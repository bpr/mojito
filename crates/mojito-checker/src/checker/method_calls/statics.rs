//! Static-method inference on plain and parameterized nominal types.

use super::*;

impl Checker {
    /// Type a static method on a parameterized built-in type. Currently only
    /// the compiler-private heap primitive
    /// `UnsafePointer[T].alloc(count: Int) -> UnsafePointer[T]` (plus
    /// `alloc_aligned` and `dangling`), reachable only from bundled
    /// standard-library sources — the audited Mojo head rejects the static
    /// allocation spelling, so user code allocates through `std.memory`.
    pub(in crate::checker) fn infer_static_method(
        &self,
        tyname: &str,
        targs: &[mojito_ast::ast::ParamArg],
        method: &str,
        args: &[Expr],
        source: Option<&str>,
    ) -> Result<Ty, TypeError> {
        if !matches!(tyname, "UnsafePointer" | "Pointer") {
            return Err(TypeError::NoSuchMethod {
                object_type: format!("{tyname}[…]"),
                method: method.to_string(),
            });
        }
        let ptr_ty = self.pointer_type(tyname, targs)?;
        match method {
            "alloc" | "alloc_aligned" => {
                // Sourceless expressions come from the stage-composed test
                // seam, which retains the primitive; every linked user file
                // carries its path and must allocate through std.memory.
                if source.is_some() && !is_bundled_stdlib_source(source) {
                    return Err(TypeError::Unsupported(format!(
                        "static UnsafePointer allocation was removed from Mojo; \
                         allocate with 'alloc(Layout[T](count=n))' from std.memory \
                         (or 'unsafe_alloc[T](n)' for a raw pointer) instead of \
                         'UnsafePointer[T].{method}'"
                    )));
                }
                let expected = if method == "alloc" { 1 } else { 2 };
                if args.len() != expected {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected,
                        got: args.len(),
                    });
                }
                for argument in args {
                    let aty = self.infer(argument)?;
                    if !coerces(&aty, &Ty::Int) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: aty.to_string(),
                            context: format!("argument to 'UnsafePointer.{method}'"),
                        });
                    }
                }
                Ok(ptr_ty)
            }
            "unsafe_dangling" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(ptr_ty)
            }
            // The pre-rename spelling is gone upstream and stays gone here.
            "dangling" => Err(TypeError::Unsupported(
                "'dangling()' was renamed in Mojo; use 'Pointer[T].unsafe_dangling()'".to_string(),
            )),
            _ => Err(TypeError::NoSuchMethod {
                object_type: ptr_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type a static method on a (possibly parametric) registered struct:
    /// `Dict[Int, Int].fromkeys(...)` (explicit `struct_targs` from a
    /// `TypeApply` receiver) or `Dict.fromkeys(keys, 0)` (empty `struct_targs`;
    /// the struct's parameters are inferred from the call's argument types).
    /// Mirrors the instance-receiver arm of [`Self::infer_method_call`]: the
    /// struct substitution applies first, method-level generics instantiate on
    /// the substituted signature, and overloads resolve through the shared
    /// scoring/selection machinery. Symbols stay template-owned
    /// (`method_lowered_name` + `self_instance_ty`); no instantiated owner is
    /// ever spelled.
    pub(super) fn infer_struct_static_method(
        &self,
        span: SourceSpan,
        sname: &str,
        struct_targs: &[mojito_ast::ast::ParamArg],
        method: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            parameterized_syntax,
            ..
        } = call;
        let info = self.structs.get(sname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
        })?;
        let receiver_spelling = || {
            if struct_targs.is_empty() {
                sname.to_string()
            } else {
                format!("{sname}[…]")
            }
        };
        let signatures = info
            .methods
            .get(method)
            .ok_or_else(|| TypeError::NoSuchMethod {
                object_type: receiver_spelling(),
                method: method.to_string(),
            })?;
        let mut matches = Vec::new();
        let mut availability_failure = None;
        let single_candidate = signatures.iter().filter(|sig| !sig.has_self).count() == 1;
        for sig in signatures.iter().filter(|sig| !sig.has_self) {
            // Solve the struct's own parameters: explicit receiver arguments
            // bind first, the remainder unifies from the argument types (`H`
            // fills from its declared default). A sole candidate propagates
            // the solver's diagnostic (`WrongTypeArgCount`,
            // `CannotInferTypeParam`, ...) instead of collapsing it into a
            // generic no-overload failure.
            let tyargs = match self.static_struct_arguments(
                sname,
                method,
                &info.decls,
                sig,
                struct_targs,
                args,
                kwargs,
            ) {
                Ok(tyargs) => tyargs,
                Err(error) if single_candidate => return Err(error),
                Err(_) => continue,
            };
            let receiver_params: Vec<Ty> = sig
                .params
                .iter()
                .map(|t| substitute_at(t, &info.decls, &tyargs))
                .collect();
            let receiver_variadic = sig
                .variadic
                .as_ref()
                .map(|ty| substitute_at(ty, &info.decls, &tyargs));
            let receiver_kw_variadic = sig
                .kw_variadic
                .as_ref()
                .map(|ty| substitute_at(ty, &info.decls, &tyargs));
            let Ok((params, variadic, kw_variadic, method_subst, mut method_arguments)) = self
                .instantiate_method_generics(
                    &format!("{sname}.{method}"),
                    sig,
                    &receiver_params,
                    receiver_variadic.as_ref(),
                    receiver_kw_variadic.as_ref(),
                    param_args,
                    args,
                    kwargs,
                )
            else {
                continue;
            };
            for (decl, argument) in info.decls.iter().zip(&tyargs) {
                method_arguments.insert(
                    decl.name().trim_start_matches('*').to_string(),
                    argument.clone(),
                );
            }
            if let Err(message) = self.method_constraint_result(sig, &method_arguments) {
                if single_candidate
                    && availability_failure.is_none()
                    && let Some(message) = message
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
                    availability_failure = Some(message.to_string());
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
                matches.push(MethodCallResolution {
                    conversion_score: scored.rank,
                    slots: scored.slots,
                    positional_overflow: scored.positional_overflow,
                    keyword_overflow: scored.keyword_overflow,
                    variadic_element: variadic.clone(),
                    keyword_element: kw_variadic.clone(),
                    conventions: sig.conventions.clone(),
                    self_convention: sig.self_convention,
                    return_type: substitute(
                        &substitute_at(&sig.ret, &info.decls, &tyargs),
                        &method_subst,
                    ),
                    result_adapter: None,
                    raises: sig.raises,
                    error: sig.error.as_ref().map(|error| {
                        Box::new(substitute(
                            &substitute_at(error, &info.decls, &tyargs),
                            &method_subst,
                        ))
                    }),
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
                });
            }
        }
        if matches.is_empty()
            && let Some(message) = availability_failure
        {
            return Err(TypeError::BadCall {
                func: format!("{sname}.{method}"),
                reason: format!("constraint failed: {message}"),
            });
        }
        let selected =
            select_method_overload(method, matches, None).map_err(|kind| TypeError::BadCall {
                func: format!("{sname}.{method}"),
                reason: match kind {
                    OverloadSelect::NoMatch => "no overload matches the supplied arguments",
                    OverloadSelect::Ambiguous => "ambiguous overloaded call",
                }
                .to_string(),
            })?;
        self.record_selected_method_conversions(method, &selected, args, kwargs)?;
        if let Some(target) = selected.lowered_name.clone() {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        if selected.raises {
            let error = selected.error.as_deref().cloned().unwrap_or(Ty::Error);
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising method '{sname}.{method}'"), error)?;
        }
        Ok(selected.return_type)
    }

    /// Solve a struct's compile-time parameters for a static-method call:
    /// bind the receiver's explicit `[…]` arguments and unify the remainder
    /// from the call's argument types against the method's *unsubstituted*
    /// parameter patterns (the same shape `resolve_use_params` serves for
    /// constructors). Slot matching mirrors `instantiate_method_generics` so
    /// keyword and variadic arguments contribute their patterns too.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn static_struct_arguments(
        &self,
        sname: &str,
        method: &str,
        decls: &[ParamDecl],
        sig: &MethodSig,
        struct_targs: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Vec<TyArg>, TypeError> {
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let matched = match_call_slots(
            &sig.names,
            &sig.required,
            sig.positional_only,
            sig.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: sig.variadic.is_some(),
                keyword: sig.kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error(&format!("{sname}.{method}")))?;
        let mut patterns = Vec::new();
        let mut actuals = Vec::new();
        for (index, slot) in matched.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            patterns.push(sig.params[index].clone());
            actuals.push(self.infer(expression)?);
        }
        if let Some(element) = sig.variadic.as_deref() {
            for position in matched.positional_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&args[position])?);
            }
        }
        if let Some(element) = sig.kw_variadic.as_deref() {
            for position in matched.keyword_overflow {
                patterns.push(element.clone());
                actuals.push(self.infer(&kwargs[position].value)?);
            }
        }
        let (_, tyargs) =
            self.resolve_use_params(sname, decls, struct_targs, &patterns, &actuals)?;
        Ok(tyargs)
    }
}
