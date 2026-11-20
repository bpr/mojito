//! Origin binders: delegated receiver/field binder maps, explicit
//! origin arguments, and origin-parameter argument resolution.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::LitOriginMut;

impl Checker {
    /// See [`Checker::lower_ref_sig_resolved`]. `Ok(None)` when `member` is
    /// not a delegated-call projection at all.
    /// The composed origin-binder correspondence along a `self`-rooted field
    /// path: for each origin parameter of the FINAL field's struct, the
    /// enclosing origin-parameter index the chained field applications bound
    /// (`self.iter` typed `EntryIter[Self.o2]` maps `EntryIter`'s binder to
    /// `o2`). `None` when any hop lacks a recorded application (alias-typed
    /// fields) or the path does not root at `self`.
    pub(super) fn delegated_receiver_binder_map(&self, object: &Expr) -> Option<Vec<(u32, u32)>> {
        let mut fields = Vec::new();
        let mut root = object;
        while let ExprKind::Member { object, field } = &root.kind {
            fields.push(field.clone());
            root = object;
        }
        if !matches!(&root.kind, ExprKind::Identifier(name) if name == "self") {
            return None;
        }
        fields.reverse();
        let (owner, _) = self
            .self_ty
            .as_ref()
            .and_then(|ty| ty.kind().as_non_simd_struct())?;
        let mut owner = owner.clone();
        let mut current: Option<Vec<(u32, u32)>> = None;
        for field in &fields {
            let info = self.structs.get(&owner)?;
            let hop = info.field_origin_arguments.get(field)?.clone();
            let composed: Vec<(u32, u32)> = hop
                .into_iter()
                .filter_map(|(child, owner_index)| match &current {
                    None => Some((child, owner_index)),
                    Some(map) => map
                        .iter()
                        .find(|(from, _)| *from == owner_index)
                        .map(|(_, enclosing)| (child, *enclosing)),
                })
                .collect();
            let (_, field_ty) = info.fields.iter().find(|(name, _)| name == field)?;
            let (next, _) = field_ty.kind().as_non_simd_struct()?;
            owner = next.clone();
            current = Some(composed);
        }
        current
    }

    pub(super) fn delegated_call_ref_sig(
        &self,
        member: &Expr,
        decls: &[ParamDecl],
        params: &[&FnParam],
    ) -> Result<Option<mojito_types::origin::RefSig>, TypeError> {
        use mojito_types::origin::{OriginSeg, RefSig, SigMutability, SigOrigin};

        // Strip trailing field projections down to the delegated call.
        let mut expr = member;
        while let ExprKind::Member { object, .. } = &expr.kind {
            expr = object;
        }
        let ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } = &expr.kind
        else {
            return Ok(None);
        };
        // Argument-taking delegated callees are legal (pin-attested): the
        // clause's origin depends only on the receiver walk, and the call's
        // arguments are checked at each call site as usual.
        let _ = (args, kwargs);
        // The receiver path: `self` or a parameter, projected through fields.
        let mut segs: Vec<OriginSeg> = Vec::new();
        let mut root = object.as_ref();
        while let ExprKind::Member { object, field } = &root.kind {
            segs.push(OriginSeg::Field(field.clone()));
            root = object;
        }
        segs.reverse();
        let (base, mut receiver_ty) = match &root.kind {
            ExprKind::Identifier(name) if name == "self" => {
                let Some(self_ty) = self.self_ty.clone() else {
                    return Err(TypeError::Unsupported(
                        "a delegated-call origin expression requires a 'self' receiver in scope"
                            .to_string(),
                    ));
                };
                (SigOrigin::Self_, self_ty)
            }
            ExprKind::Identifier(name) => {
                let Some(index) = params.iter().position(|param| &param.name == name) else {
                    return Ok(None);
                };
                (
                    SigOrigin::Param(index),
                    self.ty_from_anno(&params[index].ty)?,
                )
            }
            _ => {
                return Err(TypeError::Unsupported(
                    "a delegated-call origin expression must root at 'self' or a parameter"
                        .to_string(),
                ));
            }
        };
        // Walk the receiver's fields to the delegating struct type.
        for seg in &segs {
            let OriginSeg::Field(field) = seg else {
                unreachable!("receiver path collects field segments only");
            };
            let Some((name, arguments)) = receiver_ty.kind().as_non_simd_struct() else {
                return Err(TypeError::Unsupported(format!(
                    "delegated-call origin receiver field '{field}' projects a non-struct type"
                )));
            };
            let Some(info) = self.structs.get(name) else {
                return Err(TypeError::UndefinedVariable(name.clone()));
            };
            let Some((_, field_ty)) = info.fields.iter().find(|(candidate, _)| candidate == field)
            else {
                return Err(TypeError::NoSuchField {
                    object_type: name.clone(),
                    field: field.clone(),
                });
            };
            receiver_ty = substitute_at(field_ty, info, arguments);
        }
        let Some((callee_struct, receiver_args)) = receiver_ty.kind().as_non_simd_struct() else {
            return Err(TypeError::Unsupported(
                "a delegated-call origin expression requires a struct-typed receiver".to_string(),
            ));
        };
        let Some(info) = self.structs.get(callee_struct) else {
            return Err(TypeError::UndefinedVariable(callee_struct.clone()));
        };
        let candidates: Vec<_> = info
            .methods
            .get(method)
            .map(|sigs| sigs.iter().filter(|sig| sig.ref_return.is_some()).collect())
            .unwrap_or_default();
        let [callee] = candidates.as_slice() else {
            return Err(TypeError::Unsupported(format!(
                "a delegated-call origin expression requires exactly one \
                 ref-returning overload of '{callee_struct}.{method}' (its contract must \
                 already be declared)"
            )));
        };
        let callee_ref = &callee.call_ref_return().expect("filtered above");
        let parameter_rooted = matches!(base, SigOrigin::Param(_));
        let receiver = if segs.is_empty() {
            base
        } else {
            SigOrigin::Projected(Box::new(base), segs)
        };
        // Origin arguments are validated then erased from struct type
        // identity, so the receiver's type args usually cannot name the
        // binding. The adapter pattern's unambiguous case remains resolvable:
        // when both the callee and the delegating signature declare exactly
        // one origin binder, the field application can only have bound the
        // caller's binder to the callee's.
        let callee_origin_binders = info
            .origin_decls()
            .filter(|(_, decl)| !is_origin_set(decl))
            .count();
        let mut caller_binders = decls.iter().filter_map(|decl| match decl {
            ParamDecl::Origin { origin, .. } if !is_origin_set(decl) => Some(*origin),
            _ => None,
        });
        let caller_binder = match (caller_binders.next(), caller_binders.next()) {
            (Some(origin), None) if callee_origin_binders == 1 => {
                Some(mojito_types::origin::Origin::Param(origin))
            }
            _ => None,
        };
        let correspondences = self
            .delegated_receiver_binder_map(object)
            .unwrap_or_default();
        // A receiver rooted at a bare carrier PARAMETER (`ref[c.current().key]`
        // with `c: EntryCursor`) has no binder record and no enclosing binder
        // of its own to name the callee's: the returned reference is declared
        // to borrow the carrier itself. The carrier's construction-time loans
        // keep the ultimate source alive and conflict-checked transitively —
        // a conservative encoding of upstream's exact origin (Mojito
        // additionally rejects mutating the carrier while the reference
        // lives).
        let receiver_fallback = parameter_rooted.then_some(&receiver);
        let receiver_tail = info.instance_origin_bindings(receiver_args);
        let origin = map_delegated_sig_origin(
            &callee_ref.origin,
            &receiver_tail,
            &receiver,
            &correspondences,
            caller_binder.as_ref(),
            receiver_fallback,
        )?;
        let mutability = match callee_ref.mutability {
            SigMutability::Immutable => SigMutability::Immutable,
            SigMutability::Mutable => SigMutability::Mutable,
            SigMutability::BoolParam(index) => match receiver_args.get(index) {
                Some(TyArg::Val(mojito_types::ct::CtValue::Bool(value))) => {
                    if *value {
                        SigMutability::Mutable
                    } else {
                        SigMutability::Immutable
                    }
                }
                _ => SigMutability::Infer,
            },
            SigMutability::Infer => SigMutability::Infer,
        };
        Ok(Some(RefSig { origin, mutability }))
    }

    /// Whether `origin` is rooted only in storage that is writable at this
    /// site: `Some(true)`/`Some(false)` for a concrete verdict, `None` when
    /// the origin stays symbolic (a still-unresolved origin parameter).
    pub(in crate::checker) fn origin_writably_rooted(
        &self,
        origin: &mojito_types::origin::Origin,
    ) -> Option<bool> {
        use mojito_types::origin::Origin;
        match origin {
            Origin::Place(place) => Some(self.owner_is_mutable(place.root)),
            Origin::Union(members) => {
                let mut verdict = Some(true);
                for member in members {
                    match self.origin_writably_rooted(member) {
                        Some(true) => {}
                        Some(false) => return Some(false),
                        None => verdict = None,
                    }
                }
                verdict
            }
            Origin::Untracked { mutable } | Origin::UnsafeAny { mutable } => Some(*mutable),
            Origin::Static => Some(false),
            Origin::MutCast(..) => origin.static_mutability(),
            Origin::Param(_) | Origin::SelfParam | Origin::Projected(..) | Origin::Unbound => None,
        }
    }

    /// The origin a raw `!lit.origin` attribute names, and its mutability:
    /// upstream's `Origin[_mlir_origin=__mlir_attr[…]]()`, the value every
    /// named origin in `std.origin` is, and the `#lit.origin.mutcast`
    /// `Origin.unsafe_mut_cast` builds. `None` when `expr` is no bracketed
    /// `__mlir_attr`.
    pub(in crate::checker) fn lit_origin_attribute(
        &self,
        expr: &Expr,
    ) -> Option<Result<(mojito_types::origin::Origin, LitOriginMut), TypeError>> {
        self.lit_origin_attribute_in(expr, &[])
    }

    /// The origin a call of one of `std.origin`'s `Origin` methods building
    /// an origin names, and its mutability: `o.unsafe_mut_cast[m]()`, read
    /// from the method's declared result `Origin[_mlir_origin=...]` with
    /// the receiver's origin for `Self`'s and the bracket's arguments for
    /// the method's own binders. `None` when `expr` is no such call.
    pub(in crate::checker) fn origin_method_cast(
        &self,
        expr: &Expr,
    ) -> Option<Result<(mojito_types::origin::Origin, LitOriginMut), TypeError>> {
        self.origin_method_cast_in(expr, &[])
    }

    /// The origin an `Origin` conversion `X(o)` names, and its mutability,
    /// read through `Origin`'s declared `@implicit __init__(v: Origin) ->
    /// ImmOrigin[_mlir_origin=v._mlir_origin]`: the declared result origin
    /// with `v`'s origin `o`'s, as `X` (`ImmOrigin`, `Origin[mut=False]`,
    /// bare `Origin`, or an alias of one) constructs it. A callee whose
    /// `mut` the declared result does not have (`MutOrigin(o)`) matches no
    /// initializer, as at the pin. `None` when `X` is no `Origin` type.
    pub(in crate::checker) fn origin_conversion(
        &self,
        expr: &Expr,
    ) -> Option<Result<(mojito_types::origin::Origin, LitOriginMut), TypeError>> {
        use mojito_ast::ast::ParamArg;
        let operand = super::conversion_operand(expr)?;
        let ExprKind::Call {
            name, param_args, ..
        } = &expr.kind
        else {
            return None;
        };
        if self.lookup(name).is_some() {
            return None;
        }
        // The bare struct (`Origin(o)`) leaves its parameters to the
        // conversion: the struct at its own binders.
        let callee = match self.ty_from_anno(&mojito_ast::ast::Type::Named(
            name.clone(),
            param_args.clone(),
        )) {
            Ok(callee) => callee,
            Err(_) if param_args.is_empty() => self.self_instance_ty(name)?,
            Err(_) => return None,
        };
        let (head, arguments) = callee.kind().as_non_simd_struct()?;
        if head != "Origin" || !self.is_library_origin(head) {
            return None;
        }
        let info = self.structs.get(head)?;
        let mut_slot = info.params.iter().position(|decl| decl.name() == "mut")?;
        let origin_slot = info
            .params
            .iter()
            .position(|decl| decl.is_origin() && decl.name() == MLIR_ORIGIN_BINDER)?;
        let conversions: Vec<&MethodSig> = info
            .methods
            .get("__init__")?
            .iter()
            .filter(|sig| sig.implicit && crate::checker::converts_one_argument(sig))
            .collect();
        let [sig] = conversions.as_slice() else {
            return None;
        };
        let refinement = mojito_types::types::nominal_parts(sig.refined_self.as_ref()?)?.1;
        let declared_mut = match ParamDecl::source_argument(&info.params, &refinement, mut_slot)? {
            TyArg::Val(CtValue::Bool(mutable)) => *mutable,
            _ => return None,
        };
        let Some(TyArg::Origin(template)) =
            ParamDecl::source_argument(&info.params, &refinement, origin_slot)
        else {
            return None;
        };
        let own: Vec<mojito_types::origin::OriginParamId> =
            super::declared_origin_binders(&sig.declared).collect();
        let callee_mut = match ParamDecl::source_argument(&info.params, arguments, mut_slot) {
            Some(TyArg::Val(CtValue::Bool(mutable))) => Some(*mutable),
            _ => None,
        };
        Some((|| {
            if callee_mut.is_some_and(|mutable| mutable != declared_mut) {
                return Err(crate::checker::declarations::no_constructor_match(head));
            }
            let (base, _) = self.resolve_origin_param_arg(&ParamArg::Value(operand.clone()))?;
            let origin =
                instantiate_origin_method(template, &own, &base, &[]).ok_or_else(|| {
                    TypeError::Unsupported(format!(
                        "the origin '{template}' 'Origin.__init__' declares"
                    ))
                })?;
            let mutability = LitOriginMut::Known(declared_mut);
            Ok((
                self.fold_origin_mut_cast(&mojito_types::origin::Origin::mut_cast(
                    origin,
                    mutability.clone(),
                )),
                mutability,
            ))
        })())
    }

    /// The origin the declared origin value `argument` uses names, and
    /// whether it is mutable, read from its declaration as upstream's named
    /// origins are `Origin` values built from a raw attribute:
    /// `ImmStaticOrigin`, `UntrackedOrigin[mut=True]`, and an alias of one
    /// (`comptime MyO = MutUntrackedOrigin`). `None` when `argument` names
    /// no declared origin value, or names an origin binder in scope.
    pub(in crate::checker) fn named_origin_value(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Option<Result<(mojito_types::origin::Origin, bool), TypeError>> {
        self.origin_value_in(argument, &[], 0)
    }

    /// The origin the generic origin value `name` applied to `args` names
    /// (`AnyOrigin[mut=True]`), and whether it is mutable.
    pub(in crate::checker) fn origin_value_application(
        &self,
        name: &str,
        args: &[mojito_ast::ast::ParamArg],
    ) -> Result<(mojito_types::origin::Origin, bool), TypeError> {
        self.named_origin_value(&mojito_ast::ast::ParamArg::Type(
            mojito_ast::ast::Type::Named(name.to_string(), args.to_vec()),
        ))
        .unwrap_or_else(|| Err(TypeError::UnknownType(name.to_string())))
    }

    /// Record every module `comptime` whose value is an origin before any
    /// declaration reads one: an `Origin` built from a raw attribute, or a
    /// use of an origin value already recorded, here or in an imported
    /// package.
    pub(in crate::checker) fn declare_origin_values(&mut self, stmts: &[Stmt]) {
        let mut pending: Vec<(&String, OriginValueDecl)> = stmts
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Comptime {
                    name,
                    type_params,
                    value,
                    ..
                } => Some((
                    name,
                    OriginValueDecl {
                        binders: type_params
                            .iter()
                            .map(|param| param.name.clone())
                            .filter(|name| !name.starts_with('*'))
                            .collect(),
                        value: value.clone(),
                    },
                )),
                _ => None,
            })
            .collect();
        loop {
            let recorded = |value: &Expr| {
                origin_value_use(&mojito_ast::ast::ParamArg::Value(value.clone()))
                    .is_some_and(|(head, _)| self.origin_values.contains_key(head))
            };
            let (ready, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, decl)| {
                origin_construction_attribute(&decl.value).is_some()
                    || recorded(&decl.value)
                    || origin_method_receiver(&decl.value).is_some_and(recorded)
            });
            if ready.is_empty() {
                break;
            }
            for (name, decl) in ready {
                self.origin_values.insert(name.clone(), decl);
            }
            pending = rest;
        }
    }

    /// Resolve the compile-time value accepted by an `Origin` parameter at a
    /// function-value specialization site. `origin_of` observes checked places
    /// (including reference-valued places) and never evaluates at runtime.
    pub(in crate::checker) fn explicit_origin_argument(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::origin::Origin, TypeError> {
        use mojito_ast::ast::ParamArg;
        use mojito_types::origin::Origin;

        if let Some(base) = super::raw_origin_argument(argument) {
            return self.explicit_origin_argument(&base);
        }
        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => return self.explicit_origin_argument(value),
            ParamArg::Type(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "an Origin value".to_string(),
                    found: "a type".to_string(),
                    context: "explicit callable origin specialization".to_string(),
                });
            }
        };
        if let Some(read) = self.lit_origin_attribute(expression) {
            return read.map(|(origin, _)| origin);
        }
        // A conversion (`ImmOrigin(o)`) or cast (`o.unsafe_mut_cast[m]()`)
        // names `o`'s provenance; origin arguments carry no capability, so
        // the cast erases here.
        if let Some(error) = super::removed_origin_cast_from(expression) {
            return Err(error);
        }
        if let Some(read) = self
            .origin_method_cast(expression)
            .or_else(|| self.origin_conversion(expression))
        {
            return read.map(|(origin, _)| origin.uncast().clone());
        }
        // `base._get_owned_interior["tag"]` and `base._subtree` name a
        // projection of `base`'s origin, a binder's or the receiver's
        // included, as upstream's comptime members do. An origin naming no
        // tracked storage has no region to project, so it stands whole.
        if let Some((base, tag)) = super::interior_origin_syntax(expression) {
            return self
                .explicit_origin_argument(&ParamArg::Value(base.clone()))
                .map(|origin| origin.owned_interior(tag).unwrap_or(origin));
        }
        if let Some(base) = super::subtree_origin_syntax(expression) {
            return self
                .explicit_origin_argument(&ParamArg::Value(base.clone()))
                .map(|origin| origin.subtree().unwrap_or(origin));
        }
        match &expression.kind {
            ExprKind::Call {
                name,
                args,
                kwargs,
                param_args,
            } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                if args.is_empty() {
                    return Err(TypeError::Unsupported(
                        "origin_of requires at least one place".to_string(),
                    ));
                }
                args.iter()
                    .map(|place| {
                        // In an abstract signature — a trait method's return type
                        // such as `Self.IteratorType[origin_of(self)]` — there is
                        // no bound `self` place. Represent the receiver origin
                        // symbolically so the application still carries an
                        // argument; it is resolved to the concrete receiver origin
                        // at the conformance/call site and erases at runtime.
                        // A place *under* an unbound `self`
                        // (`origin_of(self.items)`, upstream's spelling when
                        // the field is what the view borrows) widens to the
                        // whole receiver: `Origin::SelfParam` carries no
                        // projection path, and a wider region only makes the
                        // resulting loan more conservative.
                        if let Some(("self", _)) = crate::checker::places::place_path(place)
                            && self.lookup_owner("self").is_none()
                        {
                            return Ok(Origin::SelfParam);
                        }
                        self.reference_actual(place)
                            .map(|reference| reference.origin)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Origin::union)
            }
            _ if let Some(read) = self.named_origin_value(argument) => {
                read.map(|(origin, _)| origin)
            }
            // A bare name (or `Self.name`) may spell an in-scope Origin
            // parameter directly, mirroring the `ref[o]` annotation channel:
            // `EntryIter[Self.K, Self.V, iterable_origin]`.
            ExprKind::Identifier(name) => self
                .enclosing_origin_type_param(name)
                .map(|(index, _)| Origin::Param(mojito_types::origin::OriginParamId(index)))
                .or_else(|| self.origin_typed_value(name).map(|(origin, _)| origin))
                .ok_or_else(|| TypeError::TypeMismatch {
                    expected: "origin_of(place) or a builtin Origin value".to_string(),
                    found: format!("'{name}', which names no in-scope Origin parameter"),
                    context: "explicit origin argument".to_string(),
                }),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(n) if n == "Self") => {
                self.enclosing_origin_type_param(field)
                    .map(|(index, _)| Origin::Param(mojito_types::origin::OriginParamId(index)))
                    .ok_or_else(|| TypeError::TypeMismatch {
                        expected: "origin_of(place) or a builtin Origin value".to_string(),
                        found: format!("'Self.{field}', which names no in-scope Origin parameter"),
                        context: "explicit origin argument".to_string(),
                    })
            }
            _ => Err(TypeError::TypeMismatch {
                expected: "origin_of(place) or a builtin Origin value".to_string(),
                found: "a runtime value".to_string(),
                context: "explicit callable origin specialization".to_string(),
            }),
        }
    }

    /// The origin a value `name` holds in its type's `_mlir_origin`
    /// binder, and its mutability when the type fixes it: upstream's read
    /// of a value's type parameter (`v._mlir_origin` over `v: Origin`).
    /// `None` for a name bound to no value of such a type.
    pub(in crate::checker) fn origin_typed_value(
        &self,
        name: &str,
    ) -> Option<(
        mojito_types::origin::Origin,
        Option<mojito_types::origin::Mutability>,
    )> {
        let ty = self.lookup(name)?;
        let (head, arguments) = ty.kind().as_non_simd_struct()?;
        let params = &self.structs.get(head)?.params;
        let slot = params
            .iter()
            .position(|decl| decl.is_origin() && decl.name() == MLIR_ORIGIN_BINDER)?;
        let Some(TyArg::Origin(origin)) = ParamDecl::source_argument(params, arguments, slot)
        else {
            return None;
        };
        let mutability = origin
            .static_mutability()
            .map(mojito_types::origin::Mutability::of)
            .or_else(|| self.bound_origin_mutability(origin));
        Some((origin.clone(), mutability))
    }

    /// Resolve an explicit origin argument together with its known mutability
    /// (`None` when the argument's mutability is symbolic). Struct
    /// applications use the mutability to validate a concrete
    /// `Origin[mut=True]` slot before erasing the argument.
    pub(in crate::checker) fn resolve_origin_param_arg(
        &self,
        argument: &mojito_ast::ast::ParamArg,
    ) -> Result<
        (
            mojito_types::origin::Origin,
            Option<mojito_types::origin::Mutability>,
        ),
        TypeError,
    > {
        use mojito_ast::ast::ParamArg;
        use mojito_types::origin::{Mutability, Origin};

        if let Some(base) = super::raw_origin_argument(argument) {
            return self.resolve_origin_param_arg(&base);
        }
        // A bare origin name spelled as a type argument is an origin name in
        // an origin slot.
        let type_arg_name = match argument {
            ParamArg::Type(mojito_ast::ast::Type::Named(name, targs)) if targs.is_empty() => {
                Some(name)
            }
            _ => None,
        };
        if let Some(name) = type_arg_name
            && let Some((index, decl)) = self.enclosing_origin_type_param(name)
        {
            let id = mojito_types::origin::OriginParamId(index);
            return Ok((
                Origin::Param(id),
                Some(super::super::generics::origin_binder_mutability(&decl, id)),
            ));
        }
        if let Some(read) = self.named_origin_value(argument) {
            return read.map(|(origin, mutable)| (origin, Some(Mutability::of(mutable))));
        }
        if let Some(name) = type_arg_name {
            return Err(TypeError::TypeMismatch {
                expected: "origin_of(place) or a builtin Origin value".to_string(),
                found: format!("'{name}', which names no in-scope Origin parameter"),
                context: "explicit origin argument".to_string(),
            });
        }
        let expression = match argument {
            ParamArg::Value(expression) => expression,
            ParamArg::Named { value, .. } => return self.resolve_origin_param_arg(value),
            ParamArg::Type(_) => {
                return Err(TypeError::TypeMismatch {
                    expected: "an Origin value".to_string(),
                    found: "a type".to_string(),
                    context: "explicit origin argument".to_string(),
                });
            }
        };
        if let Some(error) = super::removed_origin_cast_from(expression) {
            return Err(error);
        }
        if let Some(read) = self
            .lit_origin_attribute(expression)
            .or_else(|| self.origin_method_cast(expression))
            .or_else(|| self.origin_conversion(expression))
        {
            return read.map(|(origin, mutability)| {
                let mutability = match mutability {
                    LitOriginMut::Known(mutable) => Some(Mutability::of(mutable)),
                    LitOriginMut::Expr(_) => self.bound_origin_mutability(&origin),
                };
                (origin, mutability)
            });
        }
        let named_param_mutability = |name: &str| match self.enclosing_origin_type_param(name) {
            Some((index, decl)) => Some(super::super::generics::origin_binder_mutability(
                &decl,
                mojito_types::origin::OriginParamId(index),
            )),
            None => self
                .named_origin_value(&ParamArg::Value(Expr::new(
                    ExprKind::Identifier(name.to_string()),
                    expression.span,
                )))
                .and_then(Result::ok)
                .map(|(_, mutable)| Mutability::of(mutable)),
        };
        let mutability = match &expression.kind {
            ExprKind::Identifier(name) => named_param_mutability(name).or_else(|| {
                self.origin_typed_value(name)
                    .and_then(|(_, mutability)| mutability)
            }),
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(n) if n == "Self") => {
                named_param_mutability(field)
            }
            ExprKind::Call { name, args, .. } if name == "origin_of" => {
                // The union of places is immutable as soon as any constituent
                // is; a symbolic constituent leaves the union unknown.
                let mut union: Option<Mutability> = None;
                for place in args {
                    let Ok(reference) = self.reference_actual(place) else {
                        union = None;
                        break;
                    };
                    union = Some(match (union, reference.mutability) {
                        (_, Mutability::Immutable) | (Some(Mutability::Immutable), _) => {
                            Mutability::Immutable
                        }
                        (None | Some(Mutability::Mutable), other) => other,
                        (Some(symbolic @ Mutability::Param(_)), _) => symbolic,
                    });
                }
                union
            }
            _ => None,
        };
        let origin = self.explicit_origin_argument(argument)?;
        Ok((origin, mutability))
    }

    /// An in-scope `Origin`-bounded type parameter by name, with its index in
    /// the enclosing parameter list (the `OriginParamId` domain shared with
    /// `ref[o]` annotations).
    /// The origin-binder correspondences a field's SOURCE annotation binds:
    /// (field-struct origin-param index, enclosing origin-param index) pairs,
    /// both in the full-declaration-list (`OriginParamId`) domain. Origin
    /// arguments are erased from checked identity, so delegated-call origin
    /// clauses resolve binder correspondences through this record. The
    /// annotation may apply the struct directly (`EntryIter[Self.o]`) or
    /// through one of the declaring struct's own comptime aliases
    /// (`Self.dict_entry_iter`, `Self.view_t[o2]`), whose bodies are read
    /// in source form with the alias's own binders substituted by the
    /// application's arguments.
    pub(in crate::checker) fn field_origin_binder_arguments(
        &self,
        annotation: &mojito_ast::ast::SourceType,
        associated: &[mojito_ast::ast::StructComptime],
    ) -> Option<Vec<(u32, u32)>> {
        let mut binders = Vec::new();
        self.collect_field_origin_binders(annotation, associated, &HashMap::new(), &mut binders)?;
        let bindings: Vec<(u32, u32)> = binders
            .into_iter()
            .filter_map(|(full_index, binder)| {
                self.enclosing_origin_type_param(&binder)
                    .map(|(enclosing_index, _)| (full_index, enclosing_index))
            })
            .collect();
        (!bindings.is_empty()).then_some(bindings)
    }

    /// Collect `(field-struct origin-param full index, enclosing binder name)`
    /// pairs from a field annotation. `substitution` maps a parameterized
    /// alias's own binder names to the enclosing binder names its application
    /// supplied; the declaring struct's binders pass through unchanged.
    /// `None` when the annotation is not a recordable struct application.
    pub(super) fn collect_field_origin_binders(
        &self,
        annotation: &mojito_ast::ast::SourceType,
        associated: &[mojito_ast::ast::StructComptime],
        substitution: &HashMap<String, String>,
        out: &mut Vec<(u32, String)>,
    ) -> Option<()> {
        use mojito_ast::ast::{ParamArg, SourceType};
        let applied_binder = |argument: &ParamArg| -> Option<String> {
            let binder = match argument {
                ParamArg::Type(mojito_ast::ast::Type::Named(name, targs)) if targs.is_empty() => {
                    name.as_str()
                }
                ParamArg::Value(expression) => origin_binder_name(expression)?,
                _ => return None,
            };
            Some(
                substitution
                    .get(binder)
                    .cloned()
                    .unwrap_or_else(|| binder.to_string()),
            )
        };
        match annotation {
            SourceType::Named(name, args) => {
                let info = self.structs.get(name)?;
                let explicit: Vec<&ParamDecl> = info.explicit_params().collect();
                if args.len() != explicit.len() {
                    return None;
                }
                for (decl, argument) in explicit.iter().zip(args) {
                    let ParamDecl::Origin { origin, .. } = decl else {
                        continue;
                    };
                    if let Some(binder) = applied_binder(argument) {
                        out.push((origin.0, binder));
                    }
                }
                Some(())
            }
            // A monomorphic alias of the declaring struct: read its body.
            SourceType::SelfParam(alias) => {
                let member = associated
                    .iter()
                    .find(|member| member.name == *alias && member.params.is_empty())?;
                let body = super::constraints::assoc_body_source_type(&member.value).ok()?;
                self.collect_field_origin_binders(&body, associated, substitution, out)
            }
            // A parameterized alias application (`Self.view_t[o2]`): bind the
            // alias's explicit binders to the supplied arguments, then read
            // its body under that substitution.
            SourceType::IndexedProjection { base, index } => {
                let SourceType::SelfParam(alias) = base.as_ref() else {
                    return None;
                };
                let member = associated
                    .iter()
                    .find(|member| member.name == *alias && !member.params.is_empty())?;
                let supplied: Vec<&Expr> = match &index.kind {
                    ExprKind::TupleLit(items) => items.iter().collect(),
                    _ => vec![index.as_ref()],
                };
                // An associated alias's own list is classified where it is
                // read; none is stored for its members.
                let explicit: Vec<(usize, &mojito_ast::ast::TypeParam)> = member
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| !parameter.infer_only)
                    .collect();
                if explicit.len() != supplied.len() {
                    return None;
                }
                let mut inner = HashMap::new();
                for ((index, parameter), argument) in explicit.iter().zip(supplied) {
                    if self.origin_binder_at(&member.params, *index).is_none() {
                        continue;
                    }
                    if let Some(binder) = applied_binder(&ParamArg::Value(argument.clone())) {
                        inner.insert(parameter.name.clone(), binder);
                    }
                }
                let body = super::constraints::assoc_body_source_type(&member.value).ok()?;
                self.collect_field_origin_binders(&body, associated, &inner, out)
            }
            _ => None,
        }
    }

    /// The in-scope origin binder `name` (an `Origin`, never an
    /// `OriginSet`) with its index in the list that declares it, the
    /// `OriginParamId` domain of `ref[o]` annotations, classified by its
    /// annotation's type ([`Checker::origin_binder`]): the innermost
    /// function type being lowered declares its own binders over its own
    /// signature, before the enclosing declaration's
    /// ([`Checker::callable_type_params`]).
    pub(in crate::checker) fn enclosing_origin_type_param(
        &self,
        name: &str,
    ) -> Option<(u32, ParamDecl)> {
        let find = |params: &[mojito_ast::ast::TypeParam]| {
            self.origin_binder(params, name)
                .filter(|(decl, _)| !decl.is_origin_set())
                .map(|(decl, id)| (id.0, decl))
        };
        let own = self.callable_type_params.borrow().last().cloned();
        own.and_then(|own| find(&own))
            .or_else(|| find(&self.enclosing_type_params))
    }
}

/// The `Origin` construction whose raw attribute is `origin` at
/// mutability `mutable` (`Origin[_mlir_origin=__mlir_attr[`#lit.any.origin
/// : !lit.origin<true>`]]()`): the value an application of a generic origin
/// value is, located at the use's argument `at`.
pub(in crate::checker) fn origin_construction(
    origin: &mojito_types::origin::Origin,
    mutable: bool,
    at: Option<&Expr>,
) -> Result<Expr, TypeError> {
    use mojito_ast::ast::ParamArg;
    let attribute = origin.lit_attribute(mutable).ok_or_else(|| {
        TypeError::Unsupported(format!("the origin '{origin}' as an origin value"))
    })?;
    let node = |kind| Expr {
        source: at.and_then(|at| at.source.clone()),
        ..Expr::new(kind, at.map(|at| at.span).unwrap_or_default())
    };
    let attribute = node(ExprKind::Index {
        object: Box::new(node(ExprKind::Identifier("__mlir_attr".to_string()))),
        index: Box::new(node(ExprKind::Identifier(attribute))),
    });
    Ok(node(ExprKind::Call {
        name: "Origin".to_string(),
        param_args: vec![ParamArg::Named {
            name: "_mlir_origin".to_string(),
            value: Box::new(ParamArg::Value(attribute)),
        }],
        args: Vec::new(),
        kwargs: Vec::new(),
    }))
}

/// The receiver of a call of an `Origin` method with no run-time arguments
/// (`o.unsafe_mut_cast[False]()`), which may build an origin; `None` for
/// any other expression.
pub(in crate::checker) fn origin_method_receiver(value: &Expr) -> Option<&Expr> {
    match &value.kind {
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } if args.is_empty() && kwargs.is_empty() => match &callee.kind {
            ExprKind::Member { object, .. } => Some(object),
            _ => None,
        },
        _ => None,
    }
}

/// The origin a call building one reads: the receiver of an `Origin`
/// method (`o.unsafe_mut_cast[False]()`) or the operand of a conversion
/// (`ImmOrigin(o)`).
pub(in crate::checker) fn origin_call_operand(value: &Expr) -> Option<&Expr> {
    origin_method_receiver(value).or_else(|| super::conversion_operand(value))
}

impl super::OriginDecls for Checker {
    fn value(&self, expr: &Expr) -> Option<(mojito_types::origin::Origin, bool)> {
        self.named_origin_value(&mojito_ast::ast::ParamArg::Value(expr.clone()))?
            .ok()
    }

    fn conversion<'e>(&self, expr: &'e Expr) -> Option<Result<(&'e Expr, bool), TypeError>> {
        let operand = super::conversion_operand(expr)?;
        self.origin_conversion(expr).map(|read| {
            read.and_then(|(_, mutability)| match mutability {
                LitOriginMut::Known(mutable) => Ok((operand, mutable)),
                LitOriginMut::Expr(_) => Err(TypeError::Unsupported(
                    "an origin conversion whose mutability is symbolic".to_string(),
                )),
            })
        })
    }
}

impl Checker {
    /// [`Self::named_origin_value`] under `bindings`, the arguments an
    /// enclosing generic origin value's binders took.
    fn origin_value_in(
        &self,
        argument: &mojito_ast::ast::ParamArg,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
        depth: usize,
    ) -> Option<Result<(mojito_types::origin::Origin, bool), TypeError>> {
        use mojito_ast::ast::ParamArg;
        /// Alias chains deeper than this are cyclic.
        const MAX_DEPTH: usize = 32;
        let (name, args) = origin_value_use(argument)?;
        if depth > MAX_DEPTH || self.enclosing_origin_type_param(name).is_some() {
            return None;
        }
        let decl = self.origin_values.get(name)?;
        Some((|| {
            let unapplied = || TypeError::TypeMismatch {
                expected: "an Origin value".to_string(),
                found: format!(
                    "'{name}', a generator over [{}] that is not applied",
                    decl.binders.join(", ")
                ),
                context: "origin argument".to_string(),
            };
            if args.len() != decl.binders.len() {
                return Err(unapplied());
            }
            let mut own = Vec::with_capacity(args.len());
            for (position, argument) in args.iter().enumerate() {
                let (binder, value) = match argument {
                    ParamArg::Named { name, value } => (name.clone(), value.as_ref().clone()),
                    positional => (decl.binders[position].clone(), positional.clone()),
                };
                if !decl.binders.contains(&binder) {
                    return Err(unapplied());
                }
                own.push((binder, substitute_binding(value, bindings)));
            }
            if origin_method_receiver(&decl.value).is_some() {
                return self
                    .origin_method_cast_in(&decl.value, &own)
                    .unwrap_or_else(|| Err(unapplied()))
                    .and_then(closed_origin_value);
            }
            match origin_construction_attribute(&decl.value) {
                Some(attribute) => self
                    .lit_origin_attribute_in(attribute, &own)
                    .unwrap_or_else(|| Err(unapplied()))
                    .and_then(closed_origin_value),
                None => self
                    .origin_value_in(&ParamArg::Value(decl.value.clone()), &own, depth + 1)
                    .unwrap_or_else(|| Err(unapplied())),
            }
        })())
    }

    /// [`Self::origin_method_cast`] under `bindings`, the arguments an
    /// enclosing generic origin value's binders took.
    fn origin_method_cast_in(
        &self,
        expr: &Expr,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Option<Result<(mojito_types::origin::Origin, LitOriginMut), TypeError>> {
        use mojito_ast::ast::ParamArg;
        let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return None;
        };
        let ExprKind::Member { object, field } = &callee.kind else {
            return None;
        };
        if !args.is_empty() || !kwargs.is_empty() {
            return None;
        }
        let info = self
            .structs
            .get("Origin")
            .filter(|_| self.is_library_origin("Origin"))?;
        let [sig] = info.methods.get(field)?.as_slice() else {
            return None;
        };
        if sig.has_self || !sig.params.is_empty() {
            return None;
        }
        let template = match sig.ret.kind() {
            TyKind::Struct(name, arguments) if name == "Origin" => {
                arguments.iter().find_map(|argument| match argument {
                    TyArg::Origin(origin) => Some(origin),
                    _ => None,
                })?
            }
            _ => return None,
        };
        let own = info.params.iter().find_map(|decl| match decl {
            ParamDecl::Origin { origin, name, .. } if name == MLIR_ORIGIN_BINDER => Some(*origin),
            _ => None,
        })?;
        Some((|| {
            let (base, _) = self.resolve_origin_param_arg(&substitute_binding(
                ParamArg::Value((**object).clone()),
                bindings,
            ))?;
            let binders: Vec<&ParamDecl> = sig
                .declared
                .iter()
                .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
                .collect();
            let mismatch = || TypeError::TypeMismatch {
                expected: format!("{} compile-time argument(s)", binders.len()),
                found: format!("{}", param_args.len()),
                context: format!("'Origin.{field}'"),
            };
            if param_args.len() != binders.len() {
                return Err(mismatch());
            }
            let mut bound = Vec::with_capacity(binders.len());
            for (position, argument) in param_args.iter().enumerate() {
                let (binder, value) = match argument {
                    ParamArg::Named { name, value } => (name.as_str(), value.as_ref()),
                    positional => (binders[position].name(), positional),
                };
                let ParamArg::Value(value) = value else {
                    return Err(mismatch());
                };
                let read = ParamArg::Value(Expr::new(
                    ExprKind::Member {
                        object: Box::new(value.clone()),
                        field: mojito_types::ct::MLIR_VALUE_FIELD.to_string(),
                    },
                    value.span,
                ));
                bound.push((
                    binder.to_string(),
                    self.lit_origin_mutability(&read, bindings)?,
                ));
            }
            let origin =
                instantiate_origin_method(template, &[own], &base, &bound).ok_or_else(|| {
                    TypeError::Unsupported(format!(
                        "the origin '{template}' 'Origin.{field}' declares"
                    ))
                })?;
            let origin = self.fold_origin_mut_cast(&origin);
            let mutability = match origin.static_mutability() {
                Some(mutable) => LitOriginMut::Known(mutable),
                None => match &origin {
                    mojito_types::origin::Origin::MutCast(_, mutability) => mutability.clone(),
                    _ => self
                        .bound_origin_mutability(&origin)
                        .and_then(|mutability| match mutability {
                            mojito_types::origin::Mutability::Mutable => {
                                Some(LitOriginMut::Known(true))
                            }
                            mojito_types::origin::Mutability::Immutable => {
                                Some(LitOriginMut::Known(false))
                            }
                            mojito_types::origin::Mutability::Param(_) => None,
                        })
                        .or_else(|| bound.first().map(|(_, mutability)| mutability.clone()))
                        .unwrap_or(LitOriginMut::Known(false)),
                },
            };
            Ok((origin, mutability))
        })())
    }

    /// [`Self::lit_origin_attribute`] under `bindings`.
    fn lit_origin_attribute_in(
        &self,
        expr: &Expr,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Option<Result<(mojito_types::origin::Origin, LitOriginMut), TypeError>> {
        let (name, args) = expr.bracket_application()?;
        if name != "__mlir_attr" || self.lookup(name).is_some() {
            return None;
        }
        let (text, operands) = mojito_types::types::mlir_attribute_spelling(&args);
        let shape: String = text.split_whitespace().collect();
        let unsupported = || {
            TypeError::Unsupported(format!(
                "the origin attribute '{shape}' is outside the subset; only \
                 '#lit.any.origin', '#lit.origin.union<>', the static \
                 constants' origin, and '#lit.origin.mutcast' are supported"
            ))
        };
        Some((|| {
            if shape == MUTCAST_ATTRIBUTE {
                let [base, mutability] = operands.as_slice() else {
                    return Err(unsupported());
                };
                return self.origin_mut_cast_in(base, mutability, bindings);
            }
            let operand = match operands.as_slice() {
                [] => None,
                [operand] => match self.lit_origin_mutability(operand, bindings)? {
                    LitOriginMut::Known(mutable) => Some(mutable),
                    LitOriginMut::Expr(_) => {
                        return Err(TypeError::Unsupported(
                            "a raw origin attribute whose mutability is symbolic".to_string(),
                        ));
                    }
                },
                _ => return Err(unsupported()),
            };
            mojito_types::origin::Origin::from_lit_attribute(&shape, operand)
                .map(|(origin, mutable)| (origin, LitOriginMut::Known(mutable)))
                .ok_or_else(unsupported)
        })())
    }

    /// Upstream's `#lit.origin.mutcast<base> : !lit.origin<mutability>`
    /// under `bindings`: `base` a raw origin (`Self._mlir_origin`, the
    /// enclosing `Origin`'s own binder unless `Self` is bound), cast to
    /// `mutability`, folded as the pin compares casts.
    fn origin_mut_cast_in(
        &self,
        base: &mojito_ast::ast::ParamArg,
        mutability: &mojito_ast::ast::ParamArg,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Result<(mojito_types::origin::Origin, LitOriginMut), TypeError> {
        use mojito_ast::ast::ParamArg;
        use mojito_types::origin::Origin;
        let not_raw = || TypeError::TypeMismatch {
            expected: "a raw origin ('X._mlir_origin')".to_string(),
            found: "another operand".to_string(),
            context: "'#lit.origin.mutcast' attribute".to_string(),
        };
        let raw = super::raw_origin_argument(base).ok_or_else(not_raw)?;
        let reads_own_binder = matches!(&raw, ParamArg::Value(Expr {
                kind: ExprKind::Identifier(name),
                ..
            }) if name == "Self")
            && !bindings.iter().any(|(binder, _)| binder == "Self");
        let base = if reads_own_binder {
            self.enclosing_origin_type_param(MLIR_ORIGIN_BINDER)
                .map(|(index, _)| Origin::Param(mojito_types::origin::OriginParamId(index)))
                .ok_or_else(not_raw)?
        } else {
            self.resolve_origin_param_arg(&substitute_binding(raw, bindings))?
                .0
        };
        let mutability = self.lit_origin_mutability(mutability, bindings)?;
        let cast = self.fold_origin_mut_cast(&Origin::mut_cast(base, mutability.clone()));
        let mutability = cast
            .static_mutability()
            .map_or(mutability, LitOriginMut::Known);
        Ok((cast, mutability))
    }

    /// The `i1` mutability operand of a raw origin attribute
    /// (`+mut._mlir_value`, `True._mlir_value`) under `bindings`: `Known`
    /// once it folds, else the `Bool` binder read it spells.
    fn lit_origin_mutability(
        &self,
        operand: &mojito_ast::ast::ParamArg,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Result<LitOriginMut, TypeError> {
        use mojito_ast::ast::{ParamArg, PrefixOp};
        let operand = match operand {
            ParamArg::Value(Expr {
                kind: ExprKind::Prefix(PrefixOp::Pos, inner),
                ..
            }) => ParamArg::Value((**inner).clone()),
            operand => operand.clone(),
        };
        let operand = match mojito_types::types::mlir_value_operand(&operand) {
            Some(base) => match substitute_binding(base, bindings) {
                ParamArg::Value(Expr {
                    kind: ExprKind::Bool(literal),
                    ..
                }) => return Ok(LitOriginMut::Known(literal)),
                ParamArg::Type(mojito_ast::ast::Type::Named(name, args))
                    if args.is_empty() && (name == "True" || name == "False") =>
                {
                    return Ok(LitOriginMut::Known(name == "True"));
                }
                ParamArg::Value(base) => ParamArg::Value(Expr::new(
                    ExprKind::Member {
                        object: Box::new(base),
                        field: mojito_types::ct::MLIR_VALUE_FIELD.to_string(),
                    },
                    operand_span(&operand),
                )),
                _ => operand,
            },
            None => operand,
        };
        self.mlir_bool_operand(&operand)
    }
}

/// `template`, an `Origin` member's declared result origin, with each
/// origin binder of `own` (the `Origin`'s own `_mlir_origin`, or a
/// converted argument's) read as `base` and each cast mutability reading
/// one of the member's binders as `bound` binds it. `None` for a template
/// naming another binder.
fn instantiate_origin_method(
    template: &mojito_types::origin::Origin,
    own: &[mojito_types::origin::OriginParamId],
    base: &mojito_types::origin::Origin,
    bound: &[(String, LitOriginMut)],
) -> Option<mojito_types::origin::Origin> {
    use mojito_types::origin::Origin;
    match template {
        Origin::Param(id) if own.contains(id) => Some(base.clone()),
        Origin::Param(_) | Origin::SelfParam | Origin::Projected(..) | Origin::Unbound => None,
        Origin::MutCast(inner, mutability) => {
            let mutability = match mutability {
                LitOriginMut::Expr(read) => read
                    .as_decl_ref()
                    .and_then(|read| bound.iter().find(|(name, _)| **name == *read.name))
                    .map(|(_, value)| value.clone())?,
                known @ LitOriginMut::Known(_) => known.clone(),
            };
            Some(Origin::mut_cast(
                instantiate_origin_method(inner, own, base, bound)?,
                mutability,
            ))
        }
        Origin::Union(members) => members
            .iter()
            .map(|member| instantiate_origin_method(member, own, base, bound))
            .collect::<Option<Vec<_>>>()
            .map(Origin::union),
        closed => Some(closed.clone()),
    }
}

/// The shape of upstream's `#lit.origin.mutcast<base> : !lit.origin<m>`
/// attribute, whitespace removed, its operands `{}` holes.
const MUTCAST_ATTRIBUTE: &str = "#lit.origin.mutcast<{}>:!lit.origin<{}>";

/// The `Origin` binder holding its raw `!lit.origin` attribute.
const MLIR_ORIGIN_BINDER: &str = "_mlir_origin";

/// A declared origin value's origin and closed mutability: a named origin
/// is a closed value, which a symbolic cast mutability cannot give.
fn closed_origin_value(
    (origin, mutability): (mojito_types::origin::Origin, LitOriginMut),
) -> Result<(mojito_types::origin::Origin, bool), TypeError> {
    match mutability {
        LitOriginMut::Known(mutable) => Ok((origin, mutable)),
        LitOriginMut::Expr(_) => Err(TypeError::Unsupported(
            "a declared origin value whose mutability is symbolic".to_string(),
        )),
    }
}

/// The raw attribute an `Origin` construction (`Origin[_mlir_origin=
/// __mlir_attr[…]]()`) builds its value from; `None` for any other
/// expression.
fn origin_construction_attribute(value: &Expr) -> Option<&Expr> {
    use mojito_ast::ast::ParamArg;
    let ExprKind::Call {
        name,
        param_args,
        args,
        kwargs,
    } = &value.kind
    else {
        return None;
    };
    if name != "Origin" || !args.is_empty() || !kwargs.is_empty() {
        return None;
    }
    param_args.iter().find_map(|argument| match argument {
        ParamArg::Named { name, value: inner } if name == "_mlir_origin" => match inner.as_ref() {
            ParamArg::Value(attribute)
                if attribute
                    .bracket_application()
                    .is_some_and(|(head, _)| head == "__mlir_attr") =>
            {
                Some(attribute)
            }
            _ => None,
        },
        _ => None,
    })
}

/// The name and arguments of a use of a declared origin value: a bare name
/// (`ImmStaticOrigin`) or an application (`UntrackedOrigin[mut=True]`),
/// spelled as a value or as a type argument.
fn origin_value_use(
    argument: &mojito_ast::ast::ParamArg,
) -> Option<(&str, Vec<mojito_ast::ast::ParamArg>)> {
    use mojito_ast::ast::{ParamArg, Type};
    match argument {
        ParamArg::Type(Type::Named(name, args)) => Some((name, args.clone())),
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => Some((name, Vec::new())),
        ParamArg::Value(expr) => expr
            .bracket_application()
            .map(|(name, args)| (name, args.into_owned())),
        ParamArg::Named { .. } | ParamArg::Type(_) => None,
    }
}

/// `argument` with a bare name `bindings` binds replaced by its argument.
fn substitute_binding(
    argument: mojito_ast::ast::ParamArg,
    bindings: &[(String, mojito_ast::ast::ParamArg)],
) -> mojito_ast::ast::ParamArg {
    use mojito_ast::ast::{ParamArg, Type};
    let name = match &argument {
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => name,
        ParamArg::Type(Type::Named(name, args)) if args.is_empty() => name,
        _ => return argument,
    };
    bindings
        .iter()
        .find(|(binder, _)| binder == name)
        .map_or(argument, |(_, bound)| bound.clone())
}

/// The span an operand's expression covers, for one synthesized over it.
fn operand_span(operand: &mojito_ast::ast::ParamArg) -> mojito_common::token::Span {
    match operand {
        mojito_ast::ast::ParamArg::Value(expr) => expr.span,
        _ => mojito_common::token::Span::default(),
    }
}
