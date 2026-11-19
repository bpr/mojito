//! Origin binders: delegated receiver/field binder maps, explicit
//! origin arguments, and origin-parameter argument resolution.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
        let callee_ref = callee.ref_return.as_ref().expect("filtered above");
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
            Origin::Static | Origin::Immutable(_) => Some(false),
            Origin::Param(_) | Origin::SelfParam | Origin::Projected(..) | Origin::Unbound => None,
        }
    }

    /// The origin a raw `!lit.origin` attribute names, and whether it is
    /// mutable: upstream's `Origin[_mlir_origin=__mlir_attr[…]]()`, the
    /// value every named origin in `std.origin` is. `None` when `expr` is
    /// no bracketed `__mlir_attr`.
    pub(in crate::checker) fn lit_origin_attribute(
        &self,
        expr: &Expr,
    ) -> Option<Result<(mojito_types::origin::Origin, bool), TypeError>> {
        self.lit_origin_attribute_in(expr, &[])
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

    /// [`Self::named_origin_value`] as a free resolver, for the signature
    /// lowerings that hold no checker.
    pub(in crate::checker) fn origin_value_of(
        &self,
        expr: &Expr,
    ) -> Option<(mojito_types::origin::Origin, bool)> {
        self.named_origin_value(&mojito_ast::ast::ParamArg::Value(expr.clone()))?
            .ok()
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
            let (ready, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, decl)| {
                origin_construction_attribute(&decl.value).is_some()
                    || origin_value_use(&mojito_ast::ast::ParamArg::Value(decl.value.clone()))
                        .is_some_and(|(head, _)| self.origin_values.contains_key(head))
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
        // `ImmOrigin(o)` names `o`'s provenance; origin arguments carry no
        // capability, so the cast erases here.
        if let Some(inner) = super::immutable_origin_cast(expression) {
            return self.explicit_origin_argument(&ParamArg::Value(inner?.clone()));
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
        if let Some(read) = self.lit_origin_attribute(expression) {
            return read.map(|(origin, mutable)| (origin, Some(Mutability::of(mutable))));
        }
        // `ImmOrigin(o)`: `o`'s provenance as an `Origin[mut=False]` value,
        // so an `Origin[mut=True]` slot rejects it and the slot's `mut`
        // binds `False`. An origin already immutable is its own cast.
        if let Some(inner) = super::immutable_origin_cast(expression) {
            let (origin, mutability) =
                self.resolve_origin_param_arg(&ParamArg::Value(inner?.clone()))?;
            let origin = if mutability == Some(Mutability::Immutable) {
                origin
            } else {
                Origin::immutable(origin)
            };
            return Ok((origin, Some(Mutability::Immutable)));
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
            ExprKind::Identifier(name) => named_param_mutability(name),
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
            match origin_construction_attribute(&decl.value) {
                Some(attribute) => self
                    .lit_origin_attribute_in(attribute, &own)
                    .unwrap_or_else(|| Err(unapplied())),
                None => self
                    .origin_value_in(&ParamArg::Value(decl.value.clone()), &own, depth + 1)
                    .unwrap_or_else(|| Err(unapplied())),
            }
        })())
    }

    /// [`Self::lit_origin_attribute`] under `bindings`.
    fn lit_origin_attribute_in(
        &self,
        expr: &Expr,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Option<Result<(mojito_types::origin::Origin, bool), TypeError>> {
        let (name, args) = expr.bracket_application()?;
        if name != "__mlir_attr" || self.lookup(name).is_some() {
            return None;
        }
        let (text, operands) = mojito_types::types::mlir_attribute_spelling(&args);
        let shape: String = text.split_whitespace().collect();
        let unsupported = || {
            TypeError::Unsupported(format!(
                "the origin attribute '{shape}' is outside the subset; only \
                 '#lit.any.origin', '#lit.origin.union<>', and the static \
                 constants' origin are supported"
            ))
        };
        Some((|| {
            let operand = match operands.as_slice() {
                [] => None,
                [operand] => Some(self.lit_origin_mutability(operand, bindings)?),
                _ => return Err(unsupported()),
            };
            mojito_types::origin::Origin::from_lit_attribute(&shape, operand)
                .ok_or_else(unsupported)
        })())
    }

    /// The `i1` mutability operand of a raw origin attribute
    /// (`+mut._mlir_value`, `True._mlir_value`) under `bindings`, once it
    /// folds: the named origins hold a closed mutability, which a symbolic
    /// one cannot give.
    fn lit_origin_mutability(
        &self,
        operand: &mojito_ast::ast::ParamArg,
        bindings: &[(String, mojito_ast::ast::ParamArg)],
    ) -> Result<bool, TypeError> {
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
                }) => return Ok(literal),
                ParamArg::Type(mojito_ast::ast::Type::Named(name, args))
                    if args.is_empty() && (name == "True" || name == "False") =>
                {
                    return Ok(name == "True");
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
        match self.mlir_bool_operand(&operand)? {
            mojito_types::types::LitOriginMut::Known(mutable) => Ok(mutable),
            mojito_types::types::LitOriginMut::Expr(_) => Err(TypeError::Unsupported(
                "a raw origin attribute whose mutability is symbolic".to_string(),
            )),
        }
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
