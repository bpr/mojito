//! The `infer_method_call` dispatcher.

use super::*;

impl Checker {
    /// Type a method call `object.method(args)`. On a generic struct value the
    /// method's parameter and return types are substituted at the receiver's
    /// type arguments; on a bounded type parameter (`x: T` with `T: SomeTrait`)
    /// the method is resolved from the bound trait's requirement, with `Self`
    /// substituted to `T`.
    pub(in crate::checker) fn infer_method_call(
        &self,
        span: SourceSpan,
        object: &Expr,
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
                return self.infer_struct_static_method(span, name, targs, method, call);
            }
            reject_kwargs(kwargs)?;
            return self.infer_static_method(name, targs, method, args, object.source.as_deref());
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
            && info
                .methods
                .get(method)
                .is_some_and(|sigs| sigs.iter().any(|sig| !sig.has_self))
        {
            let targ = crate::ast::ParamArg::Value((**index).clone());
            return self.infer_struct_static_method(span, sname, &[targ], method, call);
        }
        if let ExprKind::Identifier(sname) = &object.kind
            && let Some(info) = self.structs.get(sname)
            && let Some(signatures) = info.methods.get(method)
        {
            // A parametric struct's static needs the struct's own parameters
            // solved (here, inferred from the call's argument types — the
            // explicit spelling arrives as a `TypeApply` receiver above); the
            // dedicated path owns that. Non-parametric statics keep the
            // established path below.
            if !info.decls.is_empty() && signatures.iter().any(|sig| !sig.has_self) {
                return self.infer_struct_static_method(span, sname, &[], method, call);
            }
            let mut matches = Vec::new();
            let mut availability_failure = None;
            // Preserve established overload diagnostics: a retained constraint
            // message replaces `NoMatch` only when this is the sole callable shape.
            let single_candidate = signatures.iter().filter(|sig| !sig.has_self).count() == 1;
            for sig in signatures.iter().filter(|sig| !sig.has_self) {
                let (params, variadic, kw_variadic, method_subst, method_arguments) = match self
                    .instantiate_method_generics(
                        &format!("{sname}.{method}"),
                        sig,
                        &sig.params,
                        sig.variadic.as_deref(),
                        sig.kw_variadic.as_deref(),
                        param_args,
                        args,
                        kwargs,
                    ) {
                    Ok(instantiated) => instantiated,
                    Err(_) => continue,
                };
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
                        return_type: substitute(&sig.ret, &method_subst),
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
                self.record_selected_method_conversions(method, &selected, args, kwargs)?;
                if let Some(target) = selected.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(span.clone(), target);
                }
                if selected.raises {
                    let error = selected.error.as_deref().cloned().unwrap_or(Ty::Error);
                    self.record_call_effect(span.clone(), error.clone());
                    self.require_error(
                        format!("call to raising method '{sname}.{method}'"),
                        error,
                    )?;
                }
                return Ok(selected.return_type);
            }
            if let Some(message) = availability_failure {
                return Err(TypeError::BadCall {
                    func: format!("{sname}.{method}"),
                    reason: format!("constraint failed: {message}"),
                });
            }
        }
        // Parameterless Variant intrinsics (`v^.deinit_with(handler)`) arrive
        // as ordinary method calls rather than parameterized invokes.
        if let Some(result) =
            self.infer_variant_method(span.clone(), object, method, param_args, args, kwargs)
        {
            return result;
        }
        let obj_ty = self.infer(object)?;
        // A receiver borrows a reference result for the call rather than
        // reading the referent out as an owned value; a consuming receiver
        // is gated on `ImplicitlyCopyable` below once the method resolves.
        if self.infer_reference_value(object).is_some() {
            self.borrowed_reference_receivers
                .borrow_mut()
                .insert(object.source_span());
        }
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
        {
            if let Some(element) = list_element(&obj_ty) {
                reject_kwargs(kwargs)?;
                let result = self.infer_list_method(object, method, element, args)?;
                if matches!(
                    method,
                    "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
                ) {
                    self.record_interior_invalidation(span.clone(), object);
                }
                return Ok(result);
            }
            if let Some(element) = set_element(&obj_ty) {
                reject_kwargs(kwargs)?;
                return match method {
                    "add" => {
                        self.check_place(object)?;
                        let values = self.builtin_args("Set.add", 1, args)?;
                        if !coerces(&values[0], element) {
                            return Err(TypeError::TypeMismatch {
                                expected: element.to_string(),
                                found: values[0].to_string(),
                                context: "Set.add value".to_string(),
                            });
                        }
                        self.check_consuming(&args[0], &values[0], "Set.add value")?;
                        Ok(Ty::None)
                    }
                    _ => Err(TypeError::NoSuchMethod {
                        object_type: obj_ty.to_string(),
                        method: method.to_string(),
                    }),
                };
            }
            if let Some(elements) = tuple_elements(&obj_ty) {
                reject_kwargs(kwargs)?;
                let elements = elements.into_iter().cloned().collect::<Vec<_>>();
                return self.infer_tuple_method(&span, object, method, &elements, call);
            }
        }
        if let Ty::Simd { dtype, width } = &obj_ty {
            let (dtype, width) = (*dtype, *width);
            reject_kwargs(kwargs)?;
            // Compiler-known SIMD methods: `cast` converts dtypes
            // elementwise, `select` blends through a bool mask, the lane
            // reductions collapse to the canonicalized width-1 scalar
            // (`reduce_and`/`reduce_or` to `Bool`).
            return match method {
                "cast" if param_args.len() == 1 && args.is_empty() => {
                    let target = dtype_from_arg(&param_args[0])?;
                    // Bool casts are deferred: masks convert through
                    // `select`, and no numeric dtype casts to bool yet.
                    // (Not `NoSuchMethod`, which the Invoke path treats as
                    // fall-through to indirect-callable inference.)
                    if target == Dtype::Bool || dtype == Dtype::Bool {
                        return Err(TypeError::TypeMismatch {
                            expected: "a non-bool dtype cast".to_string(),
                            found: format!(
                                "cast from DType.{} to DType.{}",
                                dtype.name(),
                                target.name()
                            ),
                            context: "SIMD.cast".to_string(),
                        });
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        span,
                        crate::SemanticAdjustment::SimdCast {
                            dtype: target,
                            width,
                        },
                    );
                    Ok(simd_ty(target, width))
                }
                "select" if dtype == Dtype::Bool && args.len() == 2 => {
                    let true_case = self.infer(&args[0])?;
                    let false_case = self.infer(&args[1])?;
                    // Both cases share one dtype at the mask's width; a
                    // scalar/literal case splats, like an infix operand.
                    let payload = match (&true_case, &false_case) {
                        (
                            Ty::Simd {
                                dtype: d1,
                                width: w1,
                            },
                            Ty::Simd {
                                dtype: d2,
                                width: w2,
                            },
                        ) if d1 == d2 && w1 == w2 && *w1 == width => Some(*d1),
                        (Ty::Simd { dtype: d, width: w }, other)
                        | (other, Ty::Simd { dtype: d, width: w })
                            if *w == width && splats_to(other, *d) =>
                        {
                            Some(*d)
                        }
                        _ => None,
                    };
                    match payload {
                        Some(d) => Ok(simd_ty(d, width)),
                        None => Err(TypeError::TypeMismatch {
                            expected: format!("two width-{width} SIMD cases of one dtype"),
                            found: format!("{true_case} and {false_case}"),
                            context: "SIMD.select".to_string(),
                        }),
                    }
                }
                "shuffle" if !param_args.is_empty() && args.is_empty() => {
                    // Compile-time lane indices; the result takes the mask's
                    // width, which must itself be a valid SIMD width.
                    let mut mask = Vec::with_capacity(param_args.len());
                    for argument in param_args {
                        let crate::ast::ParamArg::Value(index) = argument else {
                            return Err(TypeError::TypeMismatch {
                                expected: "a compile-time lane index".to_string(),
                                found: "a type argument".to_string(),
                                context: "SIMD.shuffle".to_string(),
                            });
                        };
                        let value = self.eval_ct(index)?;
                        let lane = value.to_i64().unwrap_or(-1);
                        if lane < 0 || lane >= width {
                            return Err(TypeError::TypeMismatch {
                                expected: format!("a lane index below {width}"),
                                found: value.to_string(),
                                context: "SIMD.shuffle".to_string(),
                            });
                        }
                        mask.push(lane as usize);
                    }
                    let result_width = mask.len() as i64;
                    if result_width < 1 || (result_width & (result_width - 1)) != 0 {
                        return Err(TypeError::BadSimdWidth(result_width.to_string()));
                    }
                    self.operation_adjustments
                        .borrow_mut()
                        .insert(span, crate::SemanticAdjustment::SimdShuffle { mask });
                    Ok(simd_ty(dtype, result_width))
                }
                "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max"
                    if dtype != Dtype::Bool && args.is_empty() =>
                {
                    Ok(simd_ty(dtype, 1))
                }
                "reduce_and" | "reduce_or" if dtype == Dtype::Bool && args.is_empty() => {
                    Ok(Ty::Bool)
                }
                _ => Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                }),
            };
        }
        if matches!(&obj_ty, Ty::Struct(name, args) if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") && args.is_empty())
        {
            reject_kwargs(kwargs)?;
            if method != "indices" {
                return Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                });
            }
            let types = self.builtin_args("Slice.indices", 1, args)?;
            if !coerces(&types[0], &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: types[0].to_string(),
                    context: "Slice.indices length".to_string(),
                });
            }
            return Ok(self.public_tuple_type(vec![Ty::Int, Ty::Int, Ty::Int]));
        }
        // Raw-seam compatibility only: with the linked stdlib present the
        // nominal prelude `Optional` owns its full method surface below.
        if matches!(&obj_ty, Ty::Struct(name, args) if name == "Optional" && matches!(args.as_slice(), [TyArg::Ty(Ty::Int)]))
            && !self.structs.contains_key("Optional")
        {
            reject_kwargs(kwargs)?;
            return match method {
                "or_else" => {
                    let types = self.builtin_args("Optional.or_else", 1, args)?;
                    if coerces(&types[0], &Ty::Int) {
                        Ok(Ty::Int)
                    } else {
                        Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: types[0].to_string(),
                            context: "Optional.or_else default".to_string(),
                        })
                    }
                }
                _ => Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                }),
            };
        }
        if self.conforms_to(&obj_ty, "Writer") && method == "write" {
            reject_kwargs(kwargs)?;
            self.check_place(object)?;
            self.borrowed_read_call_places
                .borrow_mut()
                .extend(args.iter().map(Expr::source_span));
            self.infer_print(args)?;
            return Ok(Ty::None);
        }
        if matches!(&obj_ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Hasher"))
        {
            reject_kwargs(kwargs)?;
            match method {
                "update" => {
                    self.check_place(object)?;
                    let tys = self.builtin_args("Hasher.update", 1, args)?;
                    if !self.conforms_to(&tys[0], "Hashable") {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "T".to_string(),
                            ty: tys[0].to_string(),
                            trait_name: "Hashable".to_string(),
                            reason: self.trait_failure_reason(&tys[0], "Hashable"),
                        });
                    }
                    return Ok(Ty::None);
                }
                "_update_with_bytes" => {
                    self.check_place(object)?;
                    let tys = self.builtin_args("Hasher._update_with_bytes", 1, args)?;
                    if !matches!(&tys[0], Ty::Struct(name, _) if name.ends_with("Span")) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Span[Byte, _]".to_string(),
                            found: tys[0].to_string(),
                            context: "Hasher._update_with_bytes".to_string(),
                        });
                    }
                    return Ok(Ty::None);
                }
                "_update_with_simd" => {
                    self.check_place(object)?;
                    let tys = self.builtin_args("Hasher._update_with_simd", 1, args)?;
                    let expected = Ty::Simd {
                        dtype: Dtype::UInt64,
                        width: 1,
                    };
                    if !coerces(&tys[0], &expected) {
                        return Err(TypeError::TypeMismatch {
                            expected: expected.to_string(),
                            found: tys[0].to_string(),
                            context: "Hasher._update_with_simd".to_string(),
                        });
                    }
                    return Ok(Ty::None);
                }
                "finish" if args.is_empty() => {
                    return Ok(Ty::Simd {
                        dtype: Dtype::UInt64,
                        width: 1,
                    });
                }
                _ => {}
            }
        }
        if method == "format"
            && (obj_ty == Ty::StringLiteral
                || matches!(&obj_ty, Ty::Struct(name, args)
                    if args.is_empty() && crate::symbol::is_stdlib_string_struct(name)))
        {
            // A template receiver may be the compile-time literal or the
            // nominal String; the formatted result materializes nominally.
            reject_kwargs(kwargs)?;
            self.infer_print(args)?;
            return self.nominal_string_wrap(span);
        }
        if let Ty::Tuple(elements) = &obj_ty {
            reject_kwargs(kwargs)?;
            return self.infer_tuple_method(&span, object, method, elements, call);
        }
        // Built-in `Pointer` methods: the public unsafe_* operation
        // vocabulary. `unsafe_write(copy=v)` is the one keyword shape; every
        // other pointer method rejects kwargs inside.
        if let Ty::Pointer {
            element: elem,
            origin,
        } = &obj_ty
        {
            return self
                .infer_pointer_method(&span, method, elem, origin, param_args, args, kwargs);
        }
        // Compiler-private inline uninit storage (`MaybeUninit`'s field):
        // the write/take/destroy crossing vocabulary.
        if let Some(element) = crate::types::uninit_storage_element(&obj_ty) {
            let element = element.clone();
            reject_kwargs(kwargs)?;
            return self.infer_uninit_storage_method(&span, object, method, &element, args);
        }
        // Resolve the method to a concrete signature (params + return + whether
        // it mutates `self`) for this receiver, substituting the receiver's type
        // arguments (struct) or `Self` (a bounded type parameter's trait method).
        // Multi-candidate failure ordering remains the ordinary overload diagnostic;
        // a sole shape can safely explain that its availability predicate rejected.
        let mut availability_failure = None;
        let resolved: Result<Option<MethodCallResolution>, OverloadSelect> = match &obj_ty {
            Ty::Struct(sname, targs) => {
                let info = self.structs.get(sname).ok_or_else(|| {
                    TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
                })?;
                match info.methods.get(method) {
                    Some(sigs) => {
                        let overloaded = sigs.len() > 1;
                        let single_candidate = sigs.len() == 1;
                        let mut matches = Vec::new();
                        for sig in sigs {
                            let receiver_params: Vec<Ty> = sig
                                .params
                                .iter()
                                .map(|t| substitute_at(t, &info.decls, targs))
                                .collect();
                            let receiver_variadic = sig
                                .variadic
                                .as_ref()
                                .map(|ty| substitute_at(ty, &info.decls, targs));
                            let receiver_kw_variadic = sig
                                .kw_variadic
                                .as_ref()
                                .map(|ty| substitute_at(ty, &info.decls, targs));
                            let Ok((
                                params,
                                variadic,
                                kw_variadic,
                                method_subst,
                                mut method_arguments,
                            )) = self.instantiate_method_generics(
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
                            for (decl, argument) in info.decls.iter().zip(targs) {
                                method_arguments.insert(
                                    decl.name().trim_start_matches('*').to_string(),
                                    argument.clone(),
                                );
                            }
                            if let Err(message) =
                                self.method_constraint_result(sig, &method_arguments)
                            {
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
                                        &substitute_at(&sig.ret, &info.decls, targs),
                                        &method_subst,
                                    ),
                                    result_adapter: None,
                                    raises: sig.raises,
                                    error: sig.error.as_ref().map(|error| {
                                        Box::new(substitute(
                                            &substitute_at(error, &info.decls, targs),
                                            &method_subst,
                                        ))
                                    }),
                                    mutates_receiver: matches!(
                                        sig.self_convention,
                                        Some(crate::ast::ArgConvention::Mut)
                                    ),
                                    consumes_receiver: matches!(
                                        sig.self_convention,
                                        Some(
                                            crate::ast::ArgConvention::Var
                                                | crate::ast::ArgConvention::Deinit
                                        )
                                    ),
                                    lowered_name: if overloaded {
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
                                    parametric_origin_writes: sig.parametric_origin_writes.clone(),
                                    param_types: params,
                                    param_decls: sig.decls.clone(),
                                });
                            }
                        }
                        select_method_overload(
                            method,
                            matches,
                            Some(matches!(object.kind, ExprKind::Transfer(_))),
                        )
                        .map(Some)
                    }
                    None => Ok(None),
                }
            }
            receiver @ (Ty::Param { .. } | Ty::Assoc { .. }) => {
                let mut effective_bounds = match receiver {
                    Ty::Param { bounds, .. } => bounds.clone(),
                    Ty::Assoc { .. } => Vec::new(),
                    _ => unreachable!("receiver pattern is parameter or associated type"),
                };
                // `Copyable.copy` is proven by any route that proves
                // copyability (a refining bound, a member bound, or a
                // `conforms_to(T, Copyable)` availability assumption), not
                // only by a literal `Copyable` bound.
                if method == "copy"
                    && args.is_empty()
                    && self.is_copyable(receiver)
                    && !effective_bounds.iter().any(|bound| bound == "Copyable")
                {
                    effective_bounds.push("Copyable".to_string());
                }
                let signatures = self.lookup_trait_methods(&effective_bounds, method, args.len());
                if signatures.is_empty() {
                    return Err(TypeError::NoSuchMethod {
                        object_type: obj_ty.to_string(),
                        method: method.to_string(),
                    });
                }
                let single_candidate = signatures.len() == 1;
                let mut matches = Vec::new();
                for sig in signatures {
                    let receiver_params: Vec<_> = sig
                        .params
                        .iter()
                        .map(|ty| substitute_self(ty, &obj_ty))
                        .collect();
                    let receiver_variadic = sig
                        .variadic
                        .as_deref()
                        .map(|ty| substitute_self(ty, &obj_ty));
                    let receiver_kw_variadic = sig
                        .kw_variadic
                        .as_deref()
                        .map(|ty| substitute_self(ty, &obj_ty));
                    let Ok((params, variadic, kw_variadic, method_subst, method_arguments)) = self
                        .instantiate_method_generics(
                            &format!("{obj_ty}.{method}"),
                            &sig,
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
                    if let Err(message) = self.method_constraint_result(&sig, &method_arguments) {
                        if single_candidate
                            && availability_failure.is_none()
                            && let Some(message) = message
                            && self
                                .score_method_call(
                                    &sig,
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
                    let Ok(scored) = self.score_method_call(
                        &sig,
                        &params,
                        variadic.as_ref(),
                        kw_variadic.as_ref(),
                        args,
                        kwargs,
                    ) else {
                        continue;
                    };
                    matches.push(MethodCallResolution {
                        conversion_score: scored.rank,
                        slots: scored.slots,
                        positional_overflow: scored.positional_overflow,
                        keyword_overflow: scored.keyword_overflow,
                        variadic_element: variadic.clone(),
                        keyword_element: kw_variadic.clone(),
                        conventions: sig.conventions.clone(),
                        self_convention: sig.self_convention,
                        return_type: self.resolve_assoc_ty(&substitute(
                            &substitute_self(&sig.ret, &obj_ty),
                            &method_subst,
                        )),
                        result_adapter: (method == "__next__" && sig.ref_return.is_none())
                            .then_some(crate::checked::CheckedResultAdapter::CopyIteratorReference),
                        raises: sig.raises,
                        error: sig.error.as_ref().map(|error| {
                            Box::new(self.resolve_assoc_ty(&substitute(
                                &substitute_self(error, &obj_ty),
                                &method_subst,
                            )))
                        }),
                        mutates_receiver: matches!(
                            sig.self_convention,
                            Some(crate::ast::ArgConvention::Mut)
                        ),
                        consumes_receiver: matches!(
                            sig.self_convention,
                            Some(
                                crate::ast::ArgConvention::Var | crate::ast::ArgConvention::Deinit
                            )
                        ),
                        // Abstract dispatch keeps `Ty::SelfType` in `sig`, which
                        // already spells `Self`; the runtime retargets the receiver
                        // prefix once the concrete type is known.
                        lowered_name: Some(method_lowered_name(
                            "__trait_dispatch",
                            method,
                            &sig,
                            None,
                        )),
                        ref_params: sig.ref_params.clone(),
                        ref_return: sig.ref_return.clone(),
                        param_types: params,
                        param_decls: sig.decls.clone(),
                        parametric_origin_writes: sig.parametric_origin_writes.clone(),
                    });
                }
                select_method_overload(
                    method,
                    matches,
                    Some(matches!(object.kind, ExprKind::Transfer(_))),
                )
                .map(Some)
            }
            // `x.copy()` on a built-in copyable value (a scalar, literal,
            // tuple, or variant) is `Copyable.copy` with no callee: the copy
            // is the value read itself, and MIR lowers it as one.
            _ if method == "copy"
                && args.is_empty()
                && kwargs.is_empty()
                && param_args.is_empty()
                && builtin_copy_is_value_read(&obj_ty)
                && self.is_copyable(&obj_ty) =>
            {
                // A place receiver copies out of its storage exactly like an
                // implicit place copy of an `ImplicitlyCopyable` value.
                if is_place_expr(object) {
                    self.copy_place_value_uses
                        .borrow_mut()
                        .insert(object.source_span());
                }
                Ok(Some(MethodCallResolution {
                    conversion_score: 0,
                    slots: vec![],
                    positional_overflow: vec![],
                    keyword_overflow: vec![],
                    variadic_element: None,
                    keyword_element: None,
                    conventions: vec![],
                    self_convention: None,
                    return_type: obj_ty.clone(),
                    result_adapter: None,
                    raises: false,
                    error: None,
                    mutates_receiver: false,
                    consumes_receiver: false,
                    lowered_name: None,
                    ref_params: vec![],
                    ref_return: None,
                    param_types: vec![],
                    param_decls: vec![],
                    parametric_origin_writes: Vec::new(),
                }))
            }
            // Hashable scalar leaves contribute their normalized bits to the
            // caller-provided hasher. The `mut` argument is a place so its
            // updated state is committed by ordinary call lowering.
            _ if method == "__hash__"
                && args.len() == 1
                && kwargs.is_empty()
                && param_args.is_empty()
                && (builtin_hashable_ty(&obj_ty)
                    || matches!(&obj_ty, Ty::Variant(alternatives) if alternatives.iter().all(|alternative| self.is_hashable(alternative)))) =>
            {
                let hasher = self.infer(&args[0])?;
                if !self.conforms_to(&hasher, "Hasher") {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "hasher".to_string(),
                        ty: hasher.to_string(),
                        trait_name: "Hasher".to_string(),
                        reason: self.trait_failure_reason(&hasher, "Hasher"),
                    });
                }
                self.check_place(&args[0])?;
                Ok(Some(MethodCallResolution {
                    conversion_score: 0,
                    slots: vec![crate::call::ArgSlot::Positional(0)],
                    positional_overflow: vec![],
                    keyword_overflow: vec![],
                    variadic_element: None,
                    keyword_element: None,
                    conventions: vec![Some(ArgConvention::Mut)],
                    self_convention: None,
                    return_type: Ty::None,
                    result_adapter: None,
                    raises: false,
                    error: None,
                    mutates_receiver: false,
                    consumes_receiver: false,
                    lowered_name: None,
                    ref_params: vec![],
                    ref_return: None,
                    param_types: vec![hasher],
                    param_decls: vec![],
                    parametric_origin_writes: vec![],
                }))
            }
            // `x.__floor__()` / `x.__ceildiv__(y)` on a concrete type
            // conforming to the granting rounding trait is the same VM
            // intrinsic the abstract Floorable/Ceilable/Truncable/CeilDivable
            // dispatch uses; a monomorphized clone of the self-hosted `math`
            // generics resolves it directly (roadmap milestone 7).
            _ if math_dunder_bound(method, args.len())
                .iter()
                .any(|bound| self.conforms_to(&obj_ty, bound)) =>
            {
                let slots = if args.is_empty() {
                    vec![]
                } else {
                    let tys = self.builtin_args(method, 1, args)?;
                    if tys[0] != obj_ty {
                        return Err(TypeError::TypeMismatch {
                            expected: obj_ty.to_string(),
                            found: tys[0].to_string(),
                            context: format!("argument to '{method}'"),
                        });
                    }
                    vec![crate::call::ArgSlot::Positional(0)]
                };
                Ok(Some(MethodCallResolution {
                    conversion_score: 0,
                    conventions: vec![None; slots.len()],
                    param_types: if args.is_empty() {
                        vec![]
                    } else {
                        vec![obj_ty.clone()]
                    },
                    slots,
                    positional_overflow: vec![],
                    keyword_overflow: vec![],
                    variadic_element: None,
                    keyword_element: None,
                    self_convention: None,
                    return_type: obj_ty.clone(),
                    result_adapter: None,
                    raises: false,
                    error: None,
                    mutates_receiver: false,
                    consumes_receiver: false,
                    lowered_name: None,
                    ref_params: vec![],
                    ref_return: None,
                    param_decls: vec![],
                    parametric_origin_writes: vec![],
                }))
            }
            _ => Ok(None),
        };
        let resolved = match resolved {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                // A callable-typed FIELD dispatches indirectly:
                // `holder.callback(1)` loads the stored value and calls
                // through it (thin or capturing) — the field-invocation
                // channel.
                if !parameterized_syntax
                    && let Ty::Struct(sname, targs) = &obj_ty
                    && let Some(info) = self.structs.get(sname)
                    && let Some((_, field_ty)) =
                        info.fields.iter().find(|(fname, _)| fname == method)
                {
                    let field_ty = substitute(field_ty, &struct_subst(&info.decls, targs));
                    if callable_contract_ty(&field_ty).is_some() {
                        return self.infer_field_invocation(span, object, field_ty, args, kwargs);
                    }
                }
                return Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                });
            }
            Err(OverloadSelect::NoMatch) => {
                return Err(TypeError::BadCall {
                    func: method.to_string(),
                    reason: availability_failure.map_or_else(
                        || "no overload matches the supplied arguments".to_string(),
                        |message| format!("constraint failed: {message}"),
                    ),
                });
            }
            Err(OverloadSelect::Ambiguous) => {
                return Err(TypeError::BadCall {
                    func: method.to_string(),
                    reason: "ambiguous overloaded method call".to_string(),
                });
            }
        };
        if parameterized_syntax {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::ParameterizedMethodCall {
                    param_decls: resolved.param_decls.clone(),
                },
            );
        }
        let boundary_before = self.call_boundary_snapshot(&span, args, kwargs);
        self.record_selected_method_conversions(method, &resolved, args, kwargs)?;
        let call_error = resolved
            .raises
            .then(|| resolved.error.as_deref().cloned().unwrap_or(Ty::Error));
        if let Some(error) = &call_error {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error(format!("call to raising method '{method}'"), error.clone())?;
        }
        let selected_target = resolved.lowered_name.clone().or_else(|| match &obj_ty {
            Ty::Struct(name, _) if self.structs.contains_key(name) => {
                Some(format!("{name}.{method}"))
            }
            _ => None,
        });
        if let Some(target) = &selected_target {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target.clone());
        }
        // Replay the callee's loan-transfer effects against the actuals,
        // and resolve any higher-order call-through residues against the
        // concrete callables this call supplies.
        if let Ty::Struct(struct_name, _) = &obj_ty {
            let method_key = format!("{struct_name}.{method}");
            let effect_key = selected_target.as_deref().unwrap_or(&method_key);
            self.apply_transfer_effects(effect_key, Some(object), args, &span)?;
            self.apply_call_through_effects(
                effect_key,
                &resolved.param_decls,
                Some(object),
                param_args,
                args,
                &span,
            )?;
        } else if let Ty::Param { bounds, .. } = &obj_ty {
            // Abstract trait dispatch has no concrete body: replay the union
            // of effects over every conforming implementation of the method
            // — the whole-program dispatch set. The method-name pre-filter is
            // syntactic (round-stable), and one observation per conformer
            // key keeps the two-phase pass exact even for conformers whose
            // effects commit in a later round.
            let mut conformers: Vec<String> = self
                .structs
                .iter()
                .filter(|(_, info)| info.methods.contains_key(method))
                .filter(|(name, info)| {
                    let implementation = Ty::Struct(
                        (*name).clone(),
                        info.decls.iter().map(param_as_arg).collect(),
                    );
                    bounds
                        .iter()
                        .all(|bound| self.conforms_to(&implementation, bound))
                })
                .flat_map(|(name, info)| {
                    let signatures = &info.methods[method];
                    if signatures.len() == 1 {
                        return vec![format!("{name}.{method}")];
                    }
                    let self_ty =
                        Ty::Struct(name.clone(), info.decls.iter().map(param_as_arg).collect());
                    signatures
                        .iter()
                        .map(|signature| {
                            method_lowered_name(name, method, signature, Some(&self_ty))
                        })
                        .collect()
                })
                .collect();
            conformers.sort();
            for key in conformers {
                self.apply_transfer_effects(&key, Some(object), args, &span)?;
                self.apply_call_through_effects(
                    &key,
                    &resolved.param_decls,
                    Some(object),
                    param_args,
                    args,
                    &span,
                )?;
            }
        }
        // A `mut self` method mutates its receiver, so the receiver must be a
        // writable place (the mutation is written back to it): a variable, a
        // field/index chain, or `self` in a `mut self` method.
        if resolved.mutates_receiver {
            let returned_reference = self
                .operation_adjustments
                .borrow()
                .get(&object.source_span())
                .and_then(|adjustment| match adjustment {
                    crate::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        Some(reference.clone())
                    }
                    _ => None,
                });
            if let Some(reference) = returned_reference {
                if reference.mutability != crate::origin::Mutability::Mutable {
                    return Err(TypeError::ImmutableBinding(
                        "reference-returning method receiver".to_string(),
                    ));
                }
            } else {
                self.check_place(object)?;
            }
            if !preserves_receiver_interiors {
                self.record_interior_invalidation(span.clone(), object);
            }
        }
        // A method body writing through a parametric-mut ref field is legal
        // only for instantiations binding that origin parameter to a mutable
        // source; judge each recorded write against the receiver's concrete
        // origin arguments here, at the instantiation site.
        for id in &resolved.parametric_origin_writes {
            let origin =
                self.resolve_receiver_origin_arguments(crate::origin::Origin::Param(*id), object);
            // A resolution that only reaches the receiver's own storage found
            // no construction-time binding for the parameter — that is a
            // symbolic origin for write legality, not a mutable source.
            let verdict = if self
                .origin_place(object)
                .is_ok_and(|place| crate::checker::origins::origin_rooted_at(&origin, place.root))
            {
                None
            } else {
                self.origin_writably_rooted(&origin)
            };
            match verdict {
                Some(true) => {
                    // The call writes the borrowed storage: invalidate interior
                    // references into it, as a direct mutation would.
                    self.record_aggregate_origin_invalidation_except(span.clone(), origin, None);
                }
                Some(false) => {
                    return Err(TypeError::BadCall {
                        func: method.to_string(),
                        reason: "writes through an origin parameter bound to an immutable \
                                 source (an Origin[mut=False] instantiation)"
                            .to_string(),
                    });
                }
                None => {
                    let mut propagated = Vec::new();
                    crate::checker::origins::collect_origin_params(&origin, &mut propagated);
                    if propagated.is_empty() {
                        let enclosing = self
                            .enclosing_type_params
                            .iter()
                            .enumerate()
                            .filter(|(_, parameter)| parameter.bounds.as_slice() == ["Origin"])
                            .map(|(index, _)| crate::origin::OriginParamId(index as u32))
                            .collect::<Vec<_>>();
                        if let [id] = enclosing.as_slice() {
                            propagated.push(*id);
                        }
                    }
                    let mut frames = self.parametric_write_frames.borrow_mut();
                    let Some(frame) = frames.last_mut() else {
                        return Err(TypeError::BadCall {
                            func: method.to_string(),
                            reason: "writes through a parametric origin that is not concrete at \
                                     this call site"
                                .to_string(),
                        });
                    };
                    if propagated.is_empty() {
                        return Err(TypeError::BadCall {
                            func: method.to_string(),
                            reason: "writes through a parametric origin whose receiver binding \
                                     cannot be propagated"
                                .to_string(),
                        });
                    }
                    for id in propagated {
                        if !frame.contains(&id) {
                            frame.push(id);
                        }
                    }
                }
            }
        }
        // A `ref self` receiver whose capability is not provably mutable —
        // immutable or parametric (`Origin[mut=m]` ref fields and their
        // reborrows) — classifies as an immutable access for ownership: only a
        // proven-mutable receiver may map to a write. Receiver-aliasing
        // exclusivity below still uses the raw declared convention.
        let effective_receiver_convention = if resolved.self_convention == Some(ArgConvention::Ref)
            && self.reference_actual(object)?.mutability != crate::origin::Mutability::Mutable
        {
            Some(ArgConvention::Imm)
        } else {
            resolved.self_convention
        };
        // A `deinit self` call always consumes its receiver. Mojo may satisfy
        // that consumption by implicitly copying an `ImplicitlyCopyable` place;
        // a merely movable (or explicitly-copy-only) place still requires `^`.
        if resolved.consumes_receiver
            && (is_place_expr(object) || self.infer_reference_value(object).is_some())
        {
            if !self.is_implicitly_copyable(&obj_ty) {
                let context = format!("consuming receiver of method '{method}'");
                if !self.is_copyable(&obj_ty) {
                    return Err(TypeError::NonCopyable {
                        ty: obj_ty.to_string(),
                        context,
                    });
                }
                let transferable = self.is_movable(&obj_ty)
                    && crate::checker::places::place_path(object)
                        .is_some_and(|(root, _)| self.is_binding_mutable(root));
                return Err(TypeError::ImplicitCopy {
                    ty: obj_ty.to_string(),
                    context,
                    transferable,
                    copyable: true,
                });
            }
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span.clone());
        }
        // A `var self` receiver takes ownership by move, so a declared
        // `Movable where False` opt-out rejects it; `deinit self` is
        // consumption-for-destruction and stays legal for non-Movable values.
        if resolved.consumes_receiver
            && resolved.self_convention == Some(crate::ast::ArgConvention::Var)
            && !self.is_movable(&obj_ty)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: format!("receiver of method '{method}'"),
                ty: obj_ty.to_string(),
                trait_name: "Movable".to_string(),
                reason: self
                    .trait_failure_reason(&obj_ty, "Movable")
                    .or_else(|| Some("its 'Movable' conformance condition is false".to_string())),
            });
        }
        if resolved.consumes_receiver
            && let Ty::Struct(name, _) = &obj_ty
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.explicit_destructors.contains_key(method))
        {
            self.explicit_destroy_calls
                .borrow_mut()
                .insert(span.clone());
        }
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let ty = self.infer_with_expected(
                expression,
                resolved
                    .param_types
                    .get(index)
                    .expect("selected method slot has a parameter type"),
                true,
            )?;
            if let Some(convention @ (ArgConvention::Var | ArgConvention::Deinit)) =
                resolved.conventions.get(index).copied().flatten()
            {
                let kind = if convention == ArgConvention::Deinit {
                    crate::checker::traits::ConsumeKind::Deinit
                } else {
                    crate::checker::traits::ConsumeKind::Move
                };
                self.check_consuming_as(
                    expression,
                    &ty,
                    &format!("argument {} to method '{}'", index + 1, method),
                    kind,
                )?;
            }
        }
        let (effective_conventions, solved_return) = self.solve_call_origins(
            &resolved.slots,
            &resolved.conventions,
            &resolved.ref_params,
            resolved.ref_return.as_ref(),
            args,
            kwargs,
        )?;
        let copied_reads = resolved
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(false),
                };
                let convention = effective_conventions.get(index).copied().flatten();
                Ok(
                    !matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
                        && self.call_read_is_independent_copy(
                            &self.infer_with_expected(
                                expression,
                                resolved
                                    .param_types
                                    .get(index)
                                    .expect("selected method slot has a parameter type"),
                                true,
                            )?,
                        ),
                )
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        check_call_aliasing(
            &resolved.slots,
            &effective_conventions,
            &copied_reads,
            args,
            kwargs,
        )?;
        check_receiver_aliasing(
            object,
            resolved.self_convention,
            &resolved.slots,
            &copied_reads,
            args,
            kwargs,
        )?;
        self.borrowed_read_call_places
            .borrow_mut()
            .extend(borrowable_read_arguments(
                &resolved.slots,
                &effective_conventions,
                args,
                kwargs,
                Some((object, resolved.self_convention)),
            ));
        let reference_result = if let Some(signature) = &resolved.ref_return {
            let actual: Vec<_> = resolved
                .slots
                .iter()
                .map(|slot| match slot {
                    ArgSlot::Positional(position) => self
                        .reference_actual(&args[*position])
                        .ok()
                        .map(|reference| reference.origin),
                    ArgSlot::Keyword(position) => self
                        .reference_actual(&kwargs[*position].value)
                        .ok()
                        .map(|reference| reference.origin),
                    ArgSlot::Default => None,
                })
                .collect();
            let self_reference = self.reference_actual(object)?;
            let origin = substitute_sig_origin_with_self(
                &signature.origin,
                &actual,
                Some(self_reference.origin),
            );
            // A struct origin parameter in the return (`ref[o]`) resolves to the
            // origin the receiver's `ref[o]` field borrows, so the returned
            // reference records a loan on its ultimate source rather than an
            // abstract parameter the loan machinery would drop.
            let origin = self.resolve_receiver_origin_arguments(origin, object);
            let mutable = match signature.mutability {
                crate::origin::SigMutability::Immutable => crate::origin::Mutability::Immutable,
                crate::origin::SigMutability::Mutable => crate::origin::Mutability::Mutable,
                _ if self_reference.mutability == crate::origin::Mutability::Mutable
                    || solved_return.is_some_and(|reference| {
                        reference.mutability == crate::origin::Mutability::Mutable
                    }) =>
                {
                    crate::origin::Mutability::Mutable
                }
                // A parametric-mut receiver stays symbolic: the write legality
                // is judged per instantiation at the enclosing call site, not
                // collapsed to immutable inside the generic body.
                _ if matches!(
                    self_reference.mutability,
                    crate::origin::Mutability::Param(_)
                ) =>
                {
                    self_reference.mutability
                }
                _ => crate::origin::Mutability::Immutable,
            };
            let reference = crate::origin::RefTy {
                referent: Box::new(resolved.return_type.clone()),
                origin,
                mutability: mutable,
            };
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                crate::checked::SemanticAdjustment::ReferenceResult {
                    reference: reference.clone(),
                },
            );
            // Iterator refinement: a `ref`-returning `__next__` satisfying a
            // by-value `Self.Element` contract is read out as a checked copy
            // (`CopyIteratorReference`). The generic body sees only the
            // by-value contract, so the monomorphized re-check must not
            // demand `ImplicitlyCopyable` where upstream sees no copy at all.
            if method == "__next__" && self.is_copyable(&resolved.return_type) {
                self.copyable_reference_result_reads
                    .borrow_mut()
                    .insert(span.clone());
            }
            Some(reference)
        } else {
            None
        };

        let boundary = self.checked_call_boundary(&span, args, kwargs, &boundary_before);

        // Retain the complete selected-call payload independently of the
        // compatibility adjustment slot.  This is the authoritative handoff
        // for nominal subscripts, and lets reference results coexist with
        // descriptor and capture metadata at one source expression.
        if let Some(target) = selected_target {
            use crate::checked::{CheckedCallArgument, CheckedCallArgumentSource};
            let mut arguments = resolved
                .slots
                .iter()
                .enumerate()
                .map(|(index, slot)| CheckedCallArgument {
                    source: match slot {
                        ArgSlot::Positional(position) => {
                            CheckedCallArgumentSource::Positional(*position)
                        }
                        ArgSlot::Keyword(position) => CheckedCallArgumentSource::Keyword(*position),
                        ArgSlot::Default => CheckedCallArgumentSource::Default,
                    },
                    parameter_ty: resolved
                        .param_types
                        .get(index)
                        .cloned()
                        .unwrap_or(Ty::Error),
                    requires_place: matches!(
                        resolved.conventions.get(index).copied().flatten(),
                        Some(ArgConvention::Mut | ArgConvention::Ref)
                    ),
                    convention: effective_conventions.get(index).copied().flatten(),
                })
                .collect::<Vec<_>>();
            if let Some(element) = &resolved.variadic_element {
                arguments.extend(resolved.positional_overflow.iter().enumerate().map(
                    |(pack_index, position)| CheckedCallArgument {
                        source: CheckedCallArgumentSource::Positional(*position),
                        parameter_ty: match element {
                            Ty::RuntimePack(elements) => {
                                elements.get(pack_index).cloned().unwrap_or(Ty::Error)
                            }
                            _ => element.clone(),
                        },
                        requires_place: false,
                        convention: None,
                    },
                ));
            }
            if let Some(element) = &resolved.keyword_element {
                arguments.extend(resolved.keyword_overflow.iter().map(|position| {
                    CheckedCallArgument {
                        source: CheckedCallArgumentSource::Keyword(*position),
                        parameter_ty: element.clone(),
                        requires_place: false,
                        convention: None,
                    }
                }));
            }
            let argument_types = args
                .iter()
                .chain(kwargs.iter().map(|argument| &argument.value))
                .filter_map(|expression| {
                    self.expression_types
                        .borrow()
                        .get(&expression.source_span())
                        .cloned()
                })
                .collect::<Vec<_>>();
            let captures = self.call_capture_effects(&argument_types);
            let parameter_arguments = param_args
                .iter()
                .filter_map(|argument| {
                    let (name, argument) = match argument {
                        crate::ast::ParamArg::Named { name, value } => {
                            (Some(name.clone()), value.as_ref())
                        }
                        argument => (None, argument),
                    };
                    let value_source = match argument {
                        crate::ast::ParamArg::Type(_) => None,
                        crate::ast::ParamArg::Value(expression) => {
                            let erased = self
                                .operation_adjustments
                                .borrow()
                                .get(&expression.source_span())
                                .is_some_and(|adjustment| {
                                    matches!(
                                        adjustment,
                                        crate::checked::SemanticAdjustment::EraseCompileTimeArgument
                                            | crate::checked::SemanticAdjustment::ReifyTypeArgument {
                                                ..
                                            }
                                    )
                                });
                            if erased {
                                return None;
                            }
                            Some(expression.source_span())
                        }
                        crate::ast::ParamArg::Named { .. } => unreachable!(),
                    };
                    Some(crate::checked::CheckedCallParameterArgument { name, value_source })
                })
                .collect();
            if reference_result.is_none() && !captures.is_empty() {
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
                );
            }
            let return_type = self.rebase_self_place_pointer(resolved.return_type.clone(), object);
            // A method whose non-consuming receiver hands back a ref-field
            // struct (a borrowing view/iterator) lends the receiver to the
            // result, exactly as a view-typed subscript does: the loan keeps
            // the source alive while the view does and rejects source
            // mutation. Capture-carrying calls keep their capture adjustment;
            // reference results already carry their own loan channel.
            if reference_result.is_none()
                && captures.is_empty()
                && !resolved.consumes_receiver
                && matches!(return_type, Ty::Struct(..))
                && self.type_carries_loans(&return_type)
            {
                self.operation_adjustments
                    .borrow_mut()
                    .entry(span.clone())
                    .or_insert(crate::checked::SemanticAdjustment::BorrowViewResult);
            }
            self.selected_calls.borrow_mut().insert(
                span,
                crate::checked::CheckedCallContract {
                    target,
                    raises: call_error,
                    result_ty: reference_result
                        .clone()
                        .map(Ty::Ref)
                        .unwrap_or_else(|| return_type.clone()),
                    result_adapter: resolved.result_adapter,
                    receiver_requires_place: matches!(
                        resolved.self_convention,
                        Some(ArgConvention::Mut | ArgConvention::Ref)
                    ),
                    receiver_convention: effective_receiver_convention,
                    arguments,
                    captures,
                    reference_result: reference_result.clone(),
                    parameter_arguments,
                    param_decls: resolved.param_decls.clone(),
                    boundary,
                },
            );
        }
        Ok(reference_result
            .map(|reference| *reference.referent)
            .unwrap_or_else(|| self.rebase_self_place_pointer(resolved.return_type, object)))
    }
}
