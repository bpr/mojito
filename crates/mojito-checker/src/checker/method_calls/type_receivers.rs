//! Method calls whose receiver spells a type: static and type-level calls.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
            && param_args.is_empty()
            && args.is_empty()
            && kwargs.is_empty()
            && let Some(ty) = self
                .self_decls
                .iter()
                .filter(|declaration| declaration.name() == method)
                .find_map(type_parameter)
            && let Ty::Param { binder, .. } = &ty
        {
            if !self.conforms_to(&ty, "Defaultable") && !self.conforms_to(&ty, "Hasher") {
                return Err(TypeError::TraitNotSatisfied {
                    param: method.to_string(),
                    ty: ty.to_string(),
                    trait_name: "Defaultable".to_string(),
                    reason: self.trait_failure_reason(&ty, "Defaultable"),
                });
            }
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::SemanticAdjustment::ConstructTypeParam {
                    param: binder.clone(),
                },
            );
            return Ok(Some(ty));
        }
        // A **static** method on a parameterized type — the receiver is a type,
        // not a value (`Dict[Int, Int].fromkeys(...)`). Handled before inferring
        // the object (which would reject a bare `TypeApply`). The pointer family
        // keeps its dedicated builtin path (including the removed-`alloc`
        // diagnostic); a registered struct with a matching static dispatches
        // through the struct-parameter-aware path.
        if let ExprKind::TypeApply { name, args: targs } = &object.kind {
            if !matches!(name.as_str(), "UnsafePointer" | "Pointer")
                && let Some(info) = self.structs.get(name)
                && info
                    .methods
                    .get(method)
                    .is_some_and(|sigs| sigs.iter().any(|sig| !sig.has_self))
            {
                let ty =
                    self.infer_struct_static_method(span.clone(), name, targs, method, call)?;
                self.record_static_receiver(object, name, targs);
                return Ok(Some(ty));
            }
            // No static of that name: an instance method called through the
            // type takes its receiver as the first argument.
            if !matches!(name.as_str(), "UnsafePointer" | "Pointer")
                && !args.is_empty()
                && self.structs.get(name).is_some_and(|info| {
                    info.methods
                        .get(method)
                        .is_some_and(|sigs| sigs.iter().any(|sig| sig.has_self))
                })
            {
                return self
                    .infer_type_receiver_instance_call(span.clone(), name, targs, method, call)
                    .map(Some);
            }
            reject_kwargs(kwargs)?;
            return self
                .infer_static_method(name, targs, method, args, object.source.as_deref())
                .map(Some);
        }
        // `Box[String].filled(...)`: a single non-builtin compile-time
        // argument parses as a value subscript (`Index`) — the bracket parser
        // cannot know `String` names a type. A subscript whose base names a
        // registered struct with a matching static (and no value binding —
        // struct names are never expression bindings) is that same
        // static-receiver spelling; reinterpret the index as the receiver's
        // compile-time argument.
        if let ExprKind::Index {
            object: base,
            index,
        } = &object.kind
            && let ExprKind::Identifier(sname) = &base.kind
            && self.lookup(sname).is_none()
            && let Some(info) = self.structs.get(sname)
            && let Some(sigs) = info.methods.get(method)
        {
            let targ = mojito_ast::ast::ParamArg::Value((**index).clone());
            if sigs.iter().any(|sig| !sig.has_self) {
                let targs = [targ];
                let ty =
                    self.infer_struct_static_method(span.clone(), sname, &targs, method, call)?;
                self.record_static_receiver(object, sname, &targs);
                return Ok(Some(ty));
            }
            if !args.is_empty() && sigs.iter().any(|sig| sig.has_self) {
                return self
                    .infer_type_receiver_instance_call(span.clone(), sname, &[targ], method, call)
                    .map(Some);
            }
        }
        if let ExprKind::Identifier(sname) = &object.kind {
            return self.infer_struct_name_call(span, object, sname, method, call);
        }
        Ok(None)
    }

    /// Type `Name.method(...)` on a bare struct name: a parametric struct's
    /// static, an instance method taking its receiver as the first argument,
    /// or a non-parametric static selected among the name's overloads.
    fn infer_struct_name_call(
        &self,
        span: &SourceSpan,
        object: &Expr,
        sname: &str,
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Option<Ty>, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            parameterized_syntax,
            preserves_receiver_interiors,
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
                .infer_struct_static_method(span.clone(), sname, &[], method, call)
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
                .infer_type_receiver_instance_call(span.clone(), sname, &[], method, call)
                .map(Some);
        }
        let mut matches = Vec::new();
        let mut candidate_sigs: Vec<(&MethodSig, Vec<Ty>)> = Vec::new();
        let mut availability_failure = None;
        // Preserve established overload diagnostics: a retained constraint
        // message replaces `NoMatch` only when this is the sole callable shape.
        let single_candidate = signatures.iter().filter(|sig| !sig.has_self).count() == 1;
        for sig in signatures.iter().filter(|sig| !sig.has_self) {
            let Ok((params, variadic, kw_variadic, method_subst, method_arguments)) = self
                .instantiate_method_generics(
                    &format!("{sname}.{method}"),
                    sig,
                    &sig.params,
                    sig.variadic.as_deref(),
                    sig.kw_variadic.as_deref(),
                    param_args,
                    args,
                    kwargs,
                )
            else {
                continue;
            };
            let Ok(clone_origins) = self.bind_clone_receiver_origins(
                &format!("{sname}.{method}"),
                sig,
                None,
                &params,
                args,
                kwargs,
            ) else {
                continue;
            };
            let params = clone_origins.substitute_all(&params);
            let variadic = variadic.map(|element| clone_origins.substitute(&element));
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
                    keyword_element: kw_variadic.clone(),
                    conventions: sig.conventions.clone(),
                    self_convention: sig.self_convention,
                    return_type: clone_origins.substitute(&substitute(&sig.ret, &method_subst)),
                    result_adapter: None,
                    raises: sig.raises,
                    error: sig
                        .error
                        .as_ref()
                        .map(|error| Box::new(substitute(error, &method_subst))),
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
            if parameterized_syntax {
                self.parameterized_method_calls
                    .borrow_mut()
                    .insert(span.clone(), selected.param_decls.clone());
            }
            // A generic static records its instantiation and retargets
            // to the per-call clone once minted, as an instance call
            // does: a compile-time-keyed body only folds bound.
            if let Some(arguments) = &selected.instantiation {
                let source_method = method.split('$').next().unwrap_or(method);
                let overload = selected
                    .lowered_name
                    .as_deref()
                    .and_then(mojito_symbol::symbol::overload_qualifier)
                    .map(str::to_string);
                self.method_instantiations.borrow_mut().insert(
                    span.clone(),
                    mojito_checked::checked::MethodInstantiation {
                        owner: sname.to_string(),
                        owner_arguments: Vec::new(),
                        method: source_method.to_string(),
                        parameter_names: selected.parameter_names.clone(),
                        overload: overload.clone(),
                        arguments: arguments.clone(),
                    },
                );
                if let Some(clone) = self
                    .specialized_method_clone(sname, method, &selected.param_decls, arguments)
                    .filter(|clone| {
                        self.clone_serves_overload(sname, source_method, clone, overload.as_deref())
                    })
                {
                    let ty = self.infer_method_call(
                        span,
                        object,
                        &clone,
                        MethodCallArguments {
                            param_args: &[],
                            args,
                            kwargs,
                            parameterized_syntax,
                            preserves_receiver_interiors,
                        },
                    )?;
                    self.record_static_clone_target(span.clone(), sname, &clone);
                    return Ok(Some(ty));
                }
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
