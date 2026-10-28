//! Method calls whose receiver spells a type: static and type-level calls.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::checker::call_inference::folded_parameter_arguments;
use mojito_types::types::TyKind;

impl Checker {
    /// Type a call whose receiver spells a type rather than a value:
    /// `Self.T()`, an applied type (`Dict[Int, Int].fromkeys(...)`), a
    /// subscripted struct name, or a bare struct name. `None` leaves the call
    /// to the value-receiver families.
    pub(super) fn infer_type_receiver_call(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        // In a generic struct body, Mojo spells construction of an associated
        // type parameter as `Self.T()`. It is a type-level member call rather
        // than an instance method call, and the concrete type is reified by
        // the enclosing constructor frame at execution time.
        if let ExprKind::Identifier(name) = &object.kind
            && name == "Self"
            && let Some(ty) = self
                .self_decls
                .iter()
                .filter(|declaration| declaration.name() == method)
                .find_map(type_parameter)
            && let TyKind::Param { binder, .. } = ty.kind()
        {
            if !(param_args.is_empty() && args.is_empty() && kwargs.is_empty()) {
                return self
                    .infer_type_param_copy_construction(
                        span.clone(),
                        method,
                        &ty,
                        param_args,
                        args,
                        kwargs,
                    )
                    .map(Some);
            }
            let bound_constructs = matches!(ty.kind(), TyKind::Param { .. })
                && self.refined_bound(&ty).is_some_and(|bounds| {
                    bounds
                        .iter()
                        .any(|bound| self.bound_default_constructs(bound))
                });
            if !bound_constructs && !self.conforms_to(&ty, "Defaultable") {
                return Err(TypeError::TraitNotSatisfied {
                    param: method.to_string(),
                    ty: ty.to_string(),
                    trait_name: "Defaultable".to_string(),
                    reason: self.trait_failure_reason(&ty, "Defaultable"),
                });
            }
            self.record_at(span.clone()).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                    param: binder.clone(),
                },
            ));
            return Ok(Some(ty));
        }
        // `Self.width()`: a static method of the enclosing struct, called on
        // the instance `Self` names, which is the struct applied to its own
        // parameters (`Grid[Self.n].width()`).
        if let ExprKind::Identifier(name) = &object.kind
            && name == "Self"
            && let Some((owner, targs)) = self.self_instance_spelling()
            && self
                .structs
                .get(&owner)
                .and_then(|info| info.methods.get(method))
                .is_some_and(|sigs| sigs.iter().any(|sig| !sig.has_self))
        {
            let ty = self.infer_struct_static_method(
                span.clone(),
                &owner,
                (&targs, None),
                method,
                call,
            )?;
            self.record_static_receiver(object, &owner, &targs);
            // The receiver spells no struct name for lowering to compose
            // the symbol from.
            self.record_at(span.clone())
                .adjustment
                .get_or_insert_with(|| {
                    Box::new(
                        mojito_checked::checked::SemanticAdjustment::ResolveCallable(format!(
                            "{owner}.{method}"
                        )),
                    )
                });
            return Ok(Some(ty));
        }
        // A method on a bracket over a type name: the receiver is the type
        // its operands apply, not a value (`Dict[Int, Int].fromkeys(...)`,
        // `Box[String].filled(...)`), as upstream's `SubscriptNode` over a
        // struct type substitutes its parameters whatever the bracket's
        // shape. A struct name is never a binding. The pointer family keeps
        // its dedicated builtin path (including the removed-`alloc`
        // diagnostic); a registered struct with a matching static
        // dispatches through the struct-parameter-aware path.
        if let Some((name, targs)) = object.bracket_application()
            && self.lookup(name).is_none()
        {
            let pointer = matches!(name, "UnsafePointer" | "Pointer");
            let signatures = self
                .structs
                .get(name)
                .filter(|_| !pointer)
                .and_then(|info| info.methods.get(method));
            if signatures.is_some_and(|sigs| sigs.iter().any(|sig| !sig.has_self)) {
                let ty = self.infer_struct_static_method(
                    span.clone(),
                    name,
                    (&targs, None),
                    method,
                    call,
                )?;
                self.record_static_receiver(object, name, &targs);
                return Ok(Some(ty));
            }
            // No static of that name: an instance method called through the
            // type takes its receiver as the first argument.
            if !args.is_empty()
                && signatures.is_some_and(|sigs| sigs.iter().any(|sig| sig.has_self))
            {
                return self
                    .infer_type_receiver_instance_call(
                        span.clone(),
                        (name, &targs, Some(object.source_span())),
                        method,
                        call,
                    )
                    .map(Some);
            }
            if pointer || matches!(object.kind, ExprKind::TypeApply { .. }) {
                reject_kwargs(kwargs)?;
                return self
                    .infer_static_method(&object.source_span(), name, &targs, method, args)
                    .map(Some);
            }
        }
        if let Some(instance) = self.type_of_receiver(object) {
            return self.infer_instance_receiver_call(span, object, &instance?, method, call);
        }
        if let ExprKind::Identifier(sname) = &object.kind {
            return self.infer_struct_name_call(span, sname, method, call);
        }
        Ok(None)
    }

    /// Type a call through `object`, a receiver bound to the type `instance`
    /// rather than spelling one (`type_of(t).__len__()`), as upstream looks
    /// the method up on the type value itself: a static of the instance, or
    /// an instance method taking its receiver as the first argument. The
    /// type is never read as a value.
    fn infer_instance_receiver_call(
        &self,
        span: &SourceSpan,
        object: &Expr,
        instance: &Ty,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let no_such_method = || TypeError::NoSuchMethod {
            object_type: instance.to_string(),
            method: method.to_string(),
        };
        let (name, arguments) =
            mojito_types::types::nominal_parts(instance).ok_or_else(no_such_method)?;
        let signatures = self
            .structs
            .get(name)
            .and_then(|info| info.methods.get(method))
            .ok_or_else(no_such_method)?;
        if signatures.iter().any(|sig| !sig.has_self) {
            let ty = self.infer_struct_static_method(
                span.clone(),
                name,
                (&[], Some(instance)),
                method,
                call,
            )?;
            // Lowering names the static by, and hands the elaborator, the
            // instance the receiver denotes.
            self.record_at(object.source_span()).ty =
                Some(TyKind::Struct(name.to_string(), arguments.into_owned()).into());
            return Ok(Some(ty));
        }
        if call.args.is_empty() {
            return Err(no_such_method());
        }
        self.infer_type_receiver_instance_call(span.clone(), (name, &[], None), method, call)
            .map(Some)
    }

    /// Type `Name.method(...)` on a bare struct name: a parametric struct's
    /// static, an instance method taking its receiver as the first argument,
    /// or a non-parametric static selected among the name's overloads.
    fn infer_struct_name_call(
        &self,
        span: &SourceSpan,
        sname: &str,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            parameterized_syntax,
            ..
        } = call;
        let Some(info) = self.structs.get(sname) else {
            return Ok(None);
        };
        let Some(signatures) = info.methods.get(method) else {
            return Ok(None);
        };
        // A parametric struct's static needs the struct's own parameters
        // solved (here, inferred from the call's argument types — the
        // explicit spelling arrives as a `TypeApply` receiver above); the
        // dedicated path owns that. Non-parametric statics keep the
        // established path below.
        if !info.decls.is_empty() && signatures.iter().any(|sig| !sig.has_self) {
            return self
                .infer_struct_static_method(span.clone(), sname, (&[], None), method, call)
                .map(Some);
        }
        // The bare type name (never an expression binding) with only
        // instance methods of that name: the receiver is the first
        // argument (`Point.norm(p)`).
        if self.lookup(sname).is_none()
            && !args.is_empty()
            && signatures.iter().all(|sig| sig.has_self)
        {
            return self
                .infer_type_receiver_instance_call(span.clone(), (sname, &[], None), method, call)
                .map(Some);
        }
        let mut matches = Vec::new();
        let mut candidate_sigs: Vec<(&MethodSig, Vec<Ty>)> = Vec::new();
        let mut availability_failure = None;
        // Preserve established overload diagnostics: a retained constraint
        // message replaces `NoMatch` only when this is the sole callable shape.
        let single_candidate = signatures.iter().filter(|sig| !sig.has_self).count() == 1;
        for sig in signatures.iter().filter(|sig| !sig.has_self) {
            let Ok((
                params,
                variadic,
                kw_variadic,
                method_subst,
                method_arguments,
                bracket_attributes,
            )) = self.instantiate_method_generics(
                &format!("{sname}.{method}"),
                sig,
                &sig.params,
                sig.variadic.as_ref(),
                sig.kw_variadic.as_ref(),
                param_args,
                args,
                kwargs,
            )
            else {
                continue;
            };
            if let Err(failure) =
                self.method_constraint_result(sig, &method_arguments, &info.decls, &[])
            {
                if single_candidate
                    && availability_failure.is_none()
                    && self
                        .score_method_call(
                            sig,
                            &params,
                            variadic.as_ref(),
                            kw_variadic.as_ref(),
                            args,
                            kwargs,
                        )
                        .is_ok()
                {
                    availability_failure = Some(failure.reason());
                }
                continue;
            }
            if let Ok(scored) = self.score_method_call(
                sig,
                &params,
                variadic.as_ref(),
                kw_variadic.as_ref(),
                args,
                kwargs,
            ) {
                candidate_sigs.push((sig, params.clone()));
                matches.push(MethodCallResolution {
                    conversion_score: scored.rank,
                    simd_erasures: scored.simd_erasures,
                    slots: scored.slots,
                    positional_overflow: scored.positional_overflow,
                    keyword_overflow: scored.keyword_overflow,
                    variadic_element: variadic.clone(),
                    variadic_convention: sig.variadic_convention,
                    variadic_name: sig.variadic_name.clone(),
                    keyword_element: kw_variadic.clone(),
                    conventions: sig.conventions.clone(),
                    self_convention: sig.self_convention,
                    return_type: self.close_method_values(
                        &substitute(
                            &super::super::generics::expand_solved_packs(
                                &sig.ret,
                                &sig.decls,
                                &method_arguments,
                            ),
                            &method_subst,
                        ),
                        &sig.decls,
                        &method_arguments,
                    ),
                    result_adapter: None,
                    raises: sig.raises,
                    error: sig
                        .error
                        .as_ref()
                        .map(|error| substitute(error, &method_subst)),
                    mutates_receiver: false,
                    consumes_receiver: false,
                    lowered_name: if signatures.len() > 1 {
                        Some(method_lowered_name(
                            sname,
                            method,
                            sig,
                            self.self_instance_ty(sname).as_ref(),
                        ))
                    } else if parameterized_syntax {
                        Some(format!("{sname}.{method}"))
                    } else {
                        None
                    },
                    ref_params: sig.ref_params.clone(),
                    ref_return: sig.ref_return.clone(),
                    param_types: params,
                    param_decls: sig.decls.clone(),
                    parametric_origin_writes: sig.parametric_origin_writes.clone(),
                    instantiation: method_instantiation_arguments(sig, &method_arguments),
                    parameter_names: sig.names.clone(),
                    view_return_interior: Vec::new(),
                    view_return: Vec::new(),
                    declared_return: None,
                    declared_params: Vec::new(),
                    nested_origins: NestedOrigins::AsDeclared,
                    bracket_attributes,
                    refined_bound: None,
                });
            }
        }
        if !matches.is_empty() {
            let selected = select_method_overload(method, matches, None).map_err(|kind| {
                TypeError::BadCall {
                    func: format!("{sname}.{method}"),
                    reason: match kind {
                        OverloadSelect::NoMatch => "no overload matches the supplied arguments",
                        OverloadSelect::Ambiguous => "ambiguous overloaded call",
                    }
                    .to_string(),
                }
            })?;
            self.record_bracket_attributes(span.clone(), selected.bracket_attributes.clone());
            if parameterized_syntax {
                self.record_at(span.clone()).parameterized_method_call =
                    Some(selected.param_decls.clone());
            }
            // A generic static records its instantiation, as an instance
            // call does: the elaborator instantiates its template per
            // call.
            if let Some(arguments) = &selected.instantiation {
                let source_method = method.split('$').next().unwrap_or(method);
                let overload = selected
                    .lowered_name
                    .as_deref()
                    .and_then(mojito_symbol::symbol::overload_qualifier)
                    .map(str::to_string);
                self.record_dtype_parameter_arguments(param_args, arguments);
                self.record_at(span.clone()).method_instantiation =
                    Some(mojito_checked::checked::MethodInstantiation {
                        owner: sname.to_string(),
                        owner_arguments: Vec::new(),
                        method: source_method.to_string(),
                        parameter_names: selected.parameter_names.clone(),
                        overload,
                        arguments: arguments.clone(),
                        folded_arguments: folded_parameter_arguments(
                            &selected.param_decls,
                            param_args,
                            arguments,
                        ),
                    });
            }
            return self
                .finish_static_call(
                    span.clone(),
                    sname,
                    method,
                    selected,
                    &candidate_sigs,
                    &info.source_params,
                    &[],
                    args,
                    kwargs,
                )
                .map(Some);
        }
        if let Some(message) = availability_failure {
            return Err(TypeError::BadCall {
                func: format!("{sname}.{method}"),
                reason: message,
            });
        }
        Ok(None)
    }
}
