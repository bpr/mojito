//! Origin binders: delegated receiver/field binder maps, explicit
//! origin arguments, and origin-parameter argument resolution.

use super::*;

impl Checker {
    /// See [`Checker::lower_ref_sig_resolved`]. `Ok(None)` when `member` is
    /// not a delegated-call projection at all.
    /// The composed origin-binder correspondence along a `self`-rooted field
    /// path: for each origin parameter of the FINAL field's struct, the
    /// enclosing origin-parameter index the chained field applications bound
    /// (`self.iter` typed `EntryIter[Self.o2]` maps EntryIter's binder to
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
        let Some(Ty::Struct(mut owner, _)) = self.self_ty.clone() else {
            return None;
        };
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
            let Ty::Struct(next, _) = field_ty else {
                return None;
            };
            owner = next.clone();
            current = Some(composed);
        }
        current
    }

    pub(super) fn delegated_call_ref_sig(
        &self,
        member: &Expr,
        type_params: &[mojito_ast::ast::TypeParam],
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
            let Ty::Struct(name, arguments) = &receiver_ty else {
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
            receiver_ty = substitute(field_ty, &struct_subst(&info.decls, arguments));
        }
        let Ty::Struct(callee_struct, receiver_args) = &receiver_ty else {
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
            .source_params
            .iter()
            .filter(|param| param.bounds.as_slice() == ["Origin"])
            .count();
        let mut caller_binders = type_params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.bounds.as_slice() == ["Origin"]);
        let caller_binder = match (caller_binders.next(), caller_binders.next()) {
            (Some((index, _)), None) if callee_origin_binders == 1 => {
                Some(mojito_types::origin::Origin::Param(
                    mojito_types::origin::OriginParamId(index as u32),
                ))
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
        let origin = map_delegated_sig_origin(
            &callee_ref.origin,
            receiver_args,
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
            Origin::Untracked { mutable } => Some(*mutable),
            Origin::Static => Some(false),
            Origin::Param(_) | Origin::SelfParam => None,
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
                        if let ExprKind::Identifier(name) = &place.kind
                            && name == "self"
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
            ExprKind::Identifier(name) if name == "ImmStaticOrigin" => Ok(Origin::Static),
            ExprKind::Identifier(name) if name == "ImmUntrackedOrigin" => {
                Ok(Origin::Untracked { mutable: false })
            }
            ExprKind::Identifier(name) if name == "MutUnsafeAnyOrigin" => {
                Ok(Origin::Untracked { mutable: true })
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

        // The parser may classify a bare identifier (or `Self.name`) argument
        // as a type argument; treat it as an origin name in an origin slot.
        let type_arg_name = match argument {
            ParamArg::Type(mojito_ast::ast::Type::Named(name, targs)) if targs.is_empty() => {
                Some(name)
            }
            ParamArg::Type(mojito_ast::ast::Type::SelfParam(name)) => Some(name),
            _ => None,
        };
        if let Some(name) = type_arg_name {
            if let Some((index, parameter)) = self.enclosing_origin_type_param(name) {
                let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => Mutability::Immutable,
                    _ => Mutability::Param(mojito_types::origin::OriginParamId(index)),
                };
                return Ok((
                    Origin::Param(mojito_types::origin::OriginParamId(index)),
                    Some(mutability),
                ));
            }
            return match name.as_str() {
                "ImmStaticOrigin" => Ok((Origin::Static, Some(Mutability::Immutable))),
                "ImmUntrackedOrigin" => Ok((
                    Origin::Untracked { mutable: false },
                    Some(Mutability::Immutable),
                )),
                "MutUnsafeAnyOrigin" => Ok((
                    Origin::Untracked { mutable: true },
                    Some(Mutability::Mutable),
                )),
                _ => Err(TypeError::TypeMismatch {
                    expected: "origin_of(place) or a builtin Origin value".to_string(),
                    found: format!("'{name}', which names no in-scope Origin parameter"),
                    context: "explicit origin argument".to_string(),
                }),
            };
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
        let named_param_mutability = |name: &str| match self.enclosing_origin_type_param(name) {
            Some((index, parameter)) => Some(
                match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                    Some(ExprKind::Bool(true)) => Mutability::Mutable,
                    Some(ExprKind::Bool(false)) => Mutability::Immutable,
                    _ => Mutability::Param(mojito_types::origin::OriginParamId(index)),
                },
            ),
            None => match name {
                "ImmStaticOrigin" | "ImmUntrackedOrigin" => Some(Mutability::Immutable),
                "MutUnsafeAnyOrigin" => Some(Mutability::Mutable),
                _ => None,
            },
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
        let is_origin = |parameter: &mojito_ast::ast::TypeParam| matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet");
        let applied_binder = |argument: &ParamArg| -> Option<String> {
            let binder = match argument {
                ParamArg::Type(mojito_ast::ast::Type::Named(name, targs)) if targs.is_empty() => {
                    name.as_str()
                }
                ParamArg::Type(mojito_ast::ast::Type::SelfParam(name)) => name.as_str(),
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
                let explicit: Vec<(usize, &mojito_ast::ast::TypeParam)> = info
                    .source_params
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| !parameter.infer_only)
                    .collect();
                if args.len() != explicit.len() {
                    return None;
                }
                for ((full_index, parameter), argument) in explicit.iter().zip(args) {
                    if !is_origin(parameter) {
                        continue;
                    }
                    if let Some(binder) = applied_binder(argument) {
                        out.push((*full_index as u32, binder));
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
                let explicit: Vec<&mojito_ast::ast::TypeParam> = member
                    .params
                    .iter()
                    .filter(|parameter| !parameter.infer_only)
                    .collect();
                if explicit.len() != supplied.len() {
                    return None;
                }
                let mut inner = HashMap::new();
                for (parameter, argument) in explicit.iter().zip(supplied) {
                    if !is_origin(parameter) {
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

    pub(in crate::checker) fn enclosing_origin_type_param(
        &self,
        name: &str,
    ) -> Option<(u32, &mojito_ast::ast::TypeParam)> {
        self.enclosing_type_params
            .iter()
            .enumerate()
            .find(|(_, parameter)| {
                parameter.name == name && parameter.bounds.as_slice() == ["Origin"]
            })
            .map(|(index, parameter)| (index as u32, parameter))
    }
}
