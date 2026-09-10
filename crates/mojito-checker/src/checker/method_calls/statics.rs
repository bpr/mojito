//! Static-method inference on plain and parameterized nominal types.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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
    /// An instance method called through its type with the receiver as the
    /// first argument (`List[Int].__len__(xs)`, `Point.norm(p)`), which
    /// current Mojo checks as the method call on that argument. The argument
    /// must be an instance of the named type (its explicit type arguments,
    /// when they resolve, must agree with the receiver's); the call then
    /// types as `xs.__len__()`, and MIR lowers it that way through
    /// [`SemanticAdjustment::ReceiverFromFirstArgument`].
    pub(in crate::checker) fn infer_type_receiver_instance_call(
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
            preserves_receiver_interiors,
        } = call;
        let Some((receiver, rest)) = args.split_first() else {
            unreachable!("type-receiver instance calls carry a receiver argument")
        };
        let info = self.structs.get(sname).ok_or_else(|| {
            TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
        })?;
        let actual = self.infer(receiver)?;
        let expected = if struct_targs.is_empty() {
            None
        } else {
            let partitioned =
                self.partition_struct_origin_args(sname, &info.source_params, struct_targs)?;
            self.resolve_use_params(sname, &info.decls, &partitioned.forwarded, &[], &[])
                .ok()
                .map(|(_, tyargs)| self.struct_instance_type(sname, tyargs))
        };
        let compatible = match (&actual, &expected) {
            (Ty::Struct(name, _), None) => name == sname,
            (Ty::Struct(name, actual_args), Some(Ty::Struct(expected_name, expected_args))) => {
                let type_args = |args: &[TyArg]| {
                    args.iter()
                        .filter_map(|arg| match arg {
                            TyArg::Ty(ty) => Some(ty.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                };
                name == expected_name && type_args(actual_args) == type_args(expected_args)
            }
            _ => false,
        };
        if !compatible {
            return Err(TypeError::TypeMismatch {
                expected: expected.map_or_else(|| sname.to_string(), |ty| ty.to_string()),
                found: actual.to_string(),
                context: format!("value passed to 'self' of '{sname}.{method}'"),
            });
        }
        let ty = self.infer_method_call(
            span.clone(),
            receiver,
            method,
            MethodCallArguments {
                param_args,
                args: rest,
                kwargs,
                parameterized_syntax,
                preserves_receiver_interiors,
            },
        )?;
        let mut adjustments = self.operation_adjustments.borrow_mut();
        let inner = adjustments.remove(&span).map(Box::new);
        adjustments.insert(
            span,
            mojito_checked::checked::SemanticAdjustment::ReceiverFromFirstArgument { inner },
        );
        Ok(ty)
    }

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
        // Origin slots partition out of the receiver application once, as
        // construction does (`Span[Int, origin_of(xs)]`, `V[origin_of(w)]`):
        // every binder below sees origin-free arguments, and the explicit
        // origins are checked against the arguments binding those slots
        // once a static is selected.
        let partitioned =
            self.partition_struct_origin_args(sname, &info.source_params, struct_targs)?;
        let forwarded: &[mojito_ast::ast::ParamArg] = &partitioned.forwarded;
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
        // The signature behind each match, recovered after selection by its
        // parameter shape (origin binding needs the declared binders).
        let mut candidate_sigs: Vec<(&MethodSig, Vec<Ty>)> = Vec::new();
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
                forwarded,
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
            let instantiation = method_instantiation_arguments(sig, &method_arguments);
            for (decl, argument) in info.decls.iter().zip(&tyargs) {
                method_arguments.insert(
                    decl.name().trim_start_matches('*').to_string(),
                    argument.clone(),
                );
            }
            if let Err(failure) = self.method_constraint_result(sig, &method_arguments) {
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
                    instantiation: instantiation.clone(),
                    parameter_names: sig.names.clone(),
                });
            }
        }
        if matches.is_empty()
            && let Some(message) = availability_failure
        {
            return Err(TypeError::BadCall {
                func: format!("{sname}.{method}"),
                reason: message,
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
        if parameterized_syntax {
            self.parameterized_method_calls
                .borrow_mut()
                .insert(span.clone(), selected.param_decls.clone());
        }
        if let Some(arguments) = &selected.instantiation {
            // An explicit receiver instance (`Box[Int].has[Bool]()`) keys the
            // request by the instance, like a method call on one.
            let receiver_targs = (!struct_targs.is_empty())
                .then(|| {
                    self.structs.get(sname).and_then(|info| {
                        self.resolve_use_params(sname, &info.decls, forwarded, &[], &[])
                            .ok()
                            .map(|(_, tyargs)| tyargs)
                    })
                })
                .flatten()
                .unwrap_or_default();
            let owner_arguments = self
                .instance_arguments(sname, &receiver_targs)
                .unwrap_or_default();
            let source_method = method.split('$').next().unwrap_or(method);
            self.method_instantiations.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::MethodInstantiation {
                    owner: sname.to_string(),
                    owner_arguments: owner_arguments.clone(),
                    method: source_method.to_string(),
                    parameter_names: selected.parameter_names.clone(),
                    arguments: arguments.clone(),
                },
            );
            let clone = if owner_arguments.is_empty() {
                self.specialized_method_clone(sname, method, &selected.param_decls, arguments)
            } else {
                self.instance_call_method_clone(
                    sname,
                    &receiver_targs,
                    source_method,
                    &selected.param_decls,
                    arguments,
                )
            };
            if let Some(clone) = clone {
                return self.infer_struct_static_method(
                    span,
                    sname,
                    struct_targs,
                    &clone,
                    MethodCallArguments {
                        param_args: &[],
                        args,
                        kwargs,
                        parameterized_syntax,
                        preserves_receiver_interiors: false,
                    },
                );
            }
        }
        // An explicit receiver instance (`Dict[String, Int].fromkeys(...)`)
        // records its instantiation and retargets to the per-instantiation
        // clone once minted; an inferred receiver keeps the erased path.
        if !struct_targs.is_empty()
            && let Ok((_, tyargs)) =
                self.resolve_use_params(sname, &info.decls, forwarded, &[], &[])
        {
            self.record_struct_instantiation(sname, &tyargs, span.source.as_deref());
            if let Some(clone) = self.instance_method_clone(sname, method, &tyargs) {
                return self.infer_struct_static_method(
                    span,
                    sname,
                    struct_targs,
                    &clone,
                    MethodCallArguments {
                        param_args,
                        args,
                        kwargs,
                        parameterized_syntax,
                        preserves_receiver_interiors: false,
                    },
                );
            }
        }
        self.finish_static_call(
            span,
            sname,
            method,
            selected,
            &candidate_sigs,
            &info.source_params,
            &partitioned.explicit_origins,
            args,
            kwargs,
        )
    }

    /// Complete a selected static call: bind the struct's origin parameters
    /// from `ref [Self.o]` arguments (a static returning `V[Self.o]` is a
    /// constructor in all but name — the explicit origins are checked against
    /// those arguments and the result keeps them lent), retain `mut`/`ref`
    /// caller places and alias-check as an instance call does (statics return
    /// before that shared tail), then record the selected conversions, symbol,
    /// and error effect.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_static_call(
        &self,
        span: SourceSpan,
        sname: &str,
        method: &str,
        selected: MethodCallResolution,
        candidate_sigs: &[(&MethodSig, Vec<Ty>)],
        source_params: &[mojito_ast::ast::TypeParam],
        explicit_origins: &[crate::checker::type_resolution::ExplicitStructOrigin],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if let Some((sig, _)) = candidate_sigs
            .iter()
            .find(|(_, params)| *params == selected.param_types)
            && (sig.origin_binders.iter().any(Option::is_some) || !explicit_origins.is_empty())
        {
            let mut bound_slots = Vec::new();
            let mut arg_tys = Vec::new();
            for (index, slot) in selected.slots.iter().enumerate() {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => continue,
                };
                bound_slots.push((index, expression, &selected.param_types[index]));
                arg_tys.push(self.infer(expression)?);
            }
            self.bind_constructor_origins(
                sname,
                method,
                source_params,
                sig,
                &bound_slots,
                &arg_tys,
                explicit_origins,
            )?;
            self.record_constructor_reference_borrows(&span, &selected.ref_params, &selected.slots);
        }
        // `mut`/`ref` arguments keep their caller places and alias-check as
        // on an instance call (statics return before that shared tail).
        let (effective_conventions, _) = self.solve_call_origins(
            &selected.slots,
            &selected.conventions,
            &selected.ref_params,
            selected.ref_return.as_ref(),
            args,
            kwargs,
        )?;
        let copied_reads = selected
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let Some(parameter) = selected.param_types.get(index) else {
                    return Ok(false);
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.call_read_is_independent_copy(
                            &self.infer_with_expected(expression, parameter, true)?,
                        ),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        crate::checker::places::check_call_aliasing(
            &selected.slots,
            &effective_conventions,
            &copied_reads,
            args,
            kwargs,
        )?;
        self.borrowed_read_call_places.borrow_mut().extend(
            crate::checker::places::borrowable_read_arguments(
                &selected.slots,
                &effective_conventions,
                args,
                kwargs,
                None,
            ),
        );
        self.record_selected_method_conversions(method, &selected, args, kwargs)?;
        if let Some(target) = selected.lowered_name.clone() {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        if selected.raises {
            let error = selected.error.as_deref().cloned().unwrap_or(Ty::Error);
            self.record_call_effect(span, error.clone());
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
