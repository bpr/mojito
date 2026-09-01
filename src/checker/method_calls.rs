//! Method-call type inference: `infer_method_call` dispatch, overload scoring,
//! call-boundary snapshots/adjustments, static- and pointer-method inference,
//! struct dunder resolution, and List/Tuple method inference. Extracted from
//! `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Type a method call `object.method(args)`. On a generic struct value the
    /// method's parameter and return types are substituted at the receiver's
    /// type arguments; on a bounded type parameter (`x: T` with `T: SomeTrait`)
    /// the method is resolved from the bound trait's requirement, with `Self`
    /// substituted to `T`.
    pub(super) fn infer_method_call(
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
        // A **static** method on a parameterized built-in type — the receiver is a
        // type, not a value (`UnsafePointer[T].alloc(n)`). Handled before inferring
        // the object (which would reject a bare `TypeApply`).
        if let ExprKind::TypeApply { name, args: targs } = &object.kind {
            reject_kwargs(kwargs)?;
            return self.infer_static_method(name, targs, method, args, object.source.as_deref());
        }
        if let ExprKind::Identifier(sname) = &object.kind
            && let Some(info) = self.structs.get(sname)
            && let Some(signatures) = info.methods.get(method)
        {
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
            && method == "update"
        {
            reject_kwargs(kwargs)?;
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
            // `x.__hash__()` on a concrete built-in hashable type (`Int`, `String`,
            // …) is an intrinsic returning `UInt` — lets a key struct combine
            // `self.field.__hash__()` values (roadmap milestone 6).
            _ if method == "__hash__"
                && args.is_empty()
                && (builtin_hashable_ty(&obj_ty)
                    || matches!(&obj_ty, Ty::Variant(alternatives) if alternatives.iter().all(|alternative| self.is_hashable(alternative)))) =>
            {
                Ok(Some(MethodCallResolution {
                    conversion_score: 0,
                    slots: vec![],
                    positional_overflow: vec![],
                    keyword_overflow: vec![],
                    variadic_element: None,
                    keyword_element: None,
                    conventions: vec![],
                    self_convention: None,
                    return_type: Ty::UInt,
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
                .is_ok_and(|place| super::origins::origin_rooted_at(&origin, place.root))
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
                    super::origins::collect_origin_params(&origin, &mut propagated);
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
                    && super::places::place_path(object)
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
                    super::traits::ConsumeKind::Deinit
                } else {
                    super::traits::ConsumeKind::Move
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
                && self.type_contains_reference(&return_type)
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

    /// Rebase a symbolic `origin_of(self)` pointer origin in a method result
    /// onto the concrete receiver: the declared interior projection is
    /// appended to the receiver's place, so the returned pointer carries the
    /// receiver's interior-generation loan. Non-pointer results and
    /// unresolvable receivers pass through unchanged.
    fn rebase_self_place_pointer(&self, ty: Ty, receiver: &Expr) -> Ty {
        use crate::origin::{Mutability, Origin, OriginSeg, PointerOrigin};
        let Ty::Pointer {
            element,
            origin: PointerOrigin::SelfPlace {
                interior, subtree, ..
            },
        } = &ty
        else {
            return ty;
        };
        let Ok(reference) = self.reference_actual(receiver) else {
            return ty;
        };
        let origin = match reference.origin {
            Origin::Place(mut place) => {
                for tag in interior {
                    place.path.push(OriginSeg::Interior(tag.clone()));
                }
                if *subtree {
                    place.path.push(OriginSeg::Subtree);
                }
                PointerOrigin::Place {
                    place,
                    mutable: matches!(reference.mutability, Mutability::Mutable),
                }
            }
            Origin::Param(id) => PointerOrigin::Param {
                id,
                mutability: reference.mutability,
                interior: interior.clone(),
                subtree: *subtree,
            },
            _ => return ty,
        };
        Ty::Pointer {
            element: element.clone(),
            origin,
        }
    }

    /// Apply the implicit conversions selected while scoring one concrete method
    /// overload. Keyword-overflow arguments are materialized into the callee's
    /// `StringDict`, so their conversions must be recorded just like conversions
    /// for ordinary parameter slots.
    pub(super) fn record_selected_method_conversions(
        &self,
        method: &str,
        resolved: &MethodCallResolution,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<(), TypeError> {
        for (index, slot) in resolved.slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            if let Some(expected) = resolved.param_types.get(index) {
                let actual = self.infer_with_expected(expression, expected, true)?;
                if !self.has_index_normalization(expression, expected)
                    && !self.record_implicit_conversion(expression, &actual, expected)?
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!("argument {} to method '{method}'", index + 1),
                    });
                }
            }
        }
        if let Some(element) = &resolved.variadic_element {
            // A specialized heterogeneous pack records each overflow argument
            // against its per-index element (mirroring the scoring pass), so
            // a literal converts where a nominal String element is expected.
            for (pack_index, &position) in resolved.positional_overflow.iter().enumerate() {
                let expected = match element {
                    Ty::RuntimePack(elements) => elements.get(pack_index).unwrap_or(element),
                    _ => element,
                };
                let expression = &args[position];
                let actual = self.infer_with_expected(expression, expected, true)?;
                if !self.record_implicit_conversion(expression, &actual, expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!("variadic argument to method '{method}'"),
                    });
                }
            }
        }
        if let Some(expected) = &resolved.keyword_element {
            for &position in &resolved.keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.record_implicit_conversion(expression, &actual, expected)? {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: format!(
                            "keyword '{}' collected by method '{method}'",
                            kwargs[position].name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn call_boundary_snapshot(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> CallBoundarySnapshot {
        let invalidations = self.interior_invalidations.borrow();
        let mut before = HashMap::new();
        for source in std::iter::once(span.clone())
            .chain(args.iter().map(Expr::source_span))
            .chain(kwargs.iter().map(|argument| argument.value.source_span()))
        {
            before
                .entry(source.clone())
                .or_insert_with(|| invalidations.get(&source).cloned().unwrap_or_default());
        }
        CallBoundarySnapshot {
            invalidations: before,
        }
    }

    /// Freeze the value adaptations and generation changes belonging to one
    /// selected call. A later call may reuse the same source occurrence (the
    /// getter/setter pair of augmented subscript assignment), so these facts must
    /// travel with the call contract rather than remain only in source-keyed maps.
    pub(super) fn checked_call_boundary(
        &self,
        span: &SourceSpan,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
        before: &CallBoundarySnapshot,
    ) -> crate::checked::CheckedCallBoundary {
        use crate::checked::{
            CheckedCallArgumentBoundary, CheckedCallArgumentSource, CheckedCallBoundary,
            CheckedCallValueAdjustment,
        };

        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        let expression_types = self.expression_types.borrow();
        let invalidations = self.interior_invalidations.borrow();
        let argument =
            |source: CheckedCallArgumentSource, expression: &Expr| -> CheckedCallArgumentBoundary {
                let value_source = expression.source_span();
                let adjustments =
                    if matches!(expression_types.get(&value_source), Some(Ty::Overload(_)))
                        && let Some(target) = overloads.get(&value_source)
                    {
                        vec![CheckedCallValueAdjustment::ResolveCallable {
                            target: target.clone(),
                        }]
                    } else if let Some(target) = implicit.get(&value_source) {
                        if crate::symbol::is_index_normalization_symbol(target) {
                            vec![CheckedCallValueAdjustment::IndexNormalization {
                                target: target.clone(),
                            }]
                        } else {
                            vec![CheckedCallValueAdjustment::ImplicitConversion {
                                target: target.clone(),
                            }]
                        }
                    } else {
                        operations
                            .get(&value_source)
                            .and_then(|adjustment| match adjustment {
                                crate::checked::SemanticAdjustment::MaterializeLiteral(target) => {
                                    Some(vec![CheckedCallValueAdjustment::MaterializeLiteral {
                                        target: Box::new(target.clone()),
                                    }])
                                }
                                _ => None,
                            })
                            .unwrap_or_default()
                    };
                let prior = before
                    .invalidations
                    .get(&value_source)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let call_invalidations = invalidations
                    .get(&value_source)
                    .into_iter()
                    .flatten()
                    .filter(|fact| !prior.contains(fact))
                    .cloned()
                    .collect();
                CheckedCallArgumentBoundary {
                    source,
                    value_source,
                    adjustments,
                    invalidations: call_invalidations,
                }
            };

        let arguments = args
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                argument(CheckedCallArgumentSource::Positional(index), expression)
            })
            .chain(kwargs.iter().enumerate().map(|(index, argument_value)| {
                argument(
                    CheckedCallArgumentSource::Keyword(index),
                    &argument_value.value,
                )
            }))
            .collect();
        let prior = before
            .invalidations
            .get(span)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let call_invalidations = invalidations
            .get(span)
            .into_iter()
            .flatten()
            .filter(|fact| !prior.contains(fact))
            .cloned()
            .collect();
        CheckedCallBoundary {
            arguments,
            invalidations: call_invalidations,
        }
    }

    pub(super) fn snapshot_value_adjustments(
        &self,
        sources: &[SourceSpan],
    ) -> Vec<ValueAdjustmentSnapshot> {
        let overloads = self.overload_targets.borrow();
        let implicit = self.implicit_conversions.borrow();
        let operations = self.operation_adjustments.borrow();
        sources
            .iter()
            .map(|source| ValueAdjustmentSnapshot {
                source: source.clone(),
                overload_target: overloads.get(source).cloned(),
                implicit_conversion: implicit.get(source).cloned(),
                operation: operations.get(source).cloned(),
            })
            .collect()
    }

    /// Put shared source operands back into their pre-call state after freezing a
    /// call boundary. Augmented subscripts then select the setter independently;
    /// neither call can overwrite the other's conversion or normalization.
    pub(super) fn restore_value_adjustments(&self, snapshots: &[ValueAdjustmentSnapshot]) {
        let mut overloads = self.overload_targets.borrow_mut();
        let mut implicit = self.implicit_conversions.borrow_mut();
        let mut operations = self.operation_adjustments.borrow_mut();
        for snapshot in snapshots {
            match &snapshot.overload_target {
                Some(target) => {
                    overloads.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    overloads.remove(&snapshot.source);
                }
            }
            match &snapshot.implicit_conversion {
                Some(target) => {
                    implicit.insert(snapshot.source.clone(), target.clone());
                }
                None => {
                    implicit.remove(&snapshot.source);
                }
            }
            match &snapshot.operation {
                Some(adjustment) => {
                    operations.insert(snapshot.source.clone(), adjustment.clone());
                }
                None => {
                    operations.remove(&snapshot.source);
                }
            }
        }
    }

    /// Remove call-local invalidations from the compatibility source tables once
    /// they have been frozen on a selected contract. Effects belonging to
    /// evaluation of the argument expression were present in the pre-call
    /// snapshot and therefore are not listed in `boundary` and remain untouched.
    pub(super) fn remove_call_boundary_invalidations(
        &self,
        site: &SourceSpan,
        boundary: &crate::checked::CheckedCallBoundary,
    ) {
        let mut invalidations = self.interior_invalidations.borrow_mut();
        let mut remove = |source: &SourceSpan, facts: &[crate::checked::InteriorInvalidation]| {
            let empty = if let Some(current) = invalidations.get_mut(source) {
                current.retain(|fact| !facts.contains(fact));
                current.is_empty()
            } else {
                false
            };
            if empty {
                invalidations.remove(source);
            }
        };
        for argument in &boundary.arguments {
            remove(&argument.value_source, &argument.invalidations);
        }
        remove(site, &boundary.invalidations);
    }

    pub(super) fn score_method_call(
        &self,
        signature: &MethodSig,
        params: &[Ty],
        variadic: Option<&Ty>,
        kw_variadic: Option<&Ty>,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<MethodCallScore, TypeError> {
        let forwarded_element = self.forwarded_kwargs_element("method", kwargs)?;
        if forwarded_element.is_some() && kw_variadic.is_none() {
            return Err(TypeError::BadCall {
                func: "method".to_string(),
                reason: "`**kwargs^` requires a callee with a `**kwargs` collector".to_string(),
            });
        }
        let keyword_names: Vec<_> = kwargs
            .iter()
            .filter(|argument| !argument.is_forwarded())
            .map(|arg| arg.name.as_str())
            .collect();
        let matched = match_call_slots(
            &signature.names,
            &signature.required,
            signature.positional_only,
            signature.keyword_only,
            args.len(),
            &keyword_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: kw_variadic.is_some(),
            },
        )
        .map_err(|error| error.into_type_error("method"))?;
        let (slots, overflow) = (matched.slots, matched.positional_overflow);
        let mut score = 0;
        for (index, slot) in slots.iter().enumerate() {
            let expression = match slot {
                ArgSlot::Positional(position) => &args[*position],
                ArgSlot::Keyword(position) => &kwargs[*position].value,
                ArgSlot::Default => continue,
            };
            let actual = self.infer_with_expected(expression, &params[index], false)?;
            if !self.has_index_normalization(expression, &params[index])
                && !self.value_coerces(&actual, &params[index])
                && (self.is_synthetic_slice_descriptor(expression)
                    || self
                        .implicit_conversion_target(&actual, &params[index])?
                        .is_none())
            {
                return Err(TypeError::TypeMismatch {
                    expected: params[index].to_string(),
                    found: actual.to_string(),
                    context: "method overload candidate".to_string(),
                });
            }
            score += conversion_count(&actual, &params[index]);
        }
        if let Some(element) = variadic {
            // A specialized heterogeneous pack (`Ty::RuntimePack`) checks each overflow
            // argument against its per-index element type with exact arity; an
            // ordinary variadic checks every argument against one element type.
            for (pack_index, &position) in overflow.iter().enumerate() {
                let expected = match element {
                    Ty::RuntimePack(elements) => {
                        elements
                            .get(pack_index)
                            .ok_or_else(|| TypeError::ArityMismatch {
                                name: "method".to_string(),
                                expected: elements.len(),
                                got: overflow.len(),
                            })?
                    }
                    _ => element,
                };
                let actual = self.infer_with_expected(&args[position], expected, false)?;
                if !coerces(&actual, expected)
                    && self
                        .implicit_conversion_target(&actual, expected)?
                        .is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: actual.to_string(),
                        context: "variadic method argument".to_string(),
                    });
                }
                score += conversion_count(&actual, expected);
            }
            if let Ty::RuntimePack(elements) = element
                && elements.len() != overflow.len()
            {
                return Err(TypeError::ArityMismatch {
                    name: "method".to_string(),
                    expected: elements.len(),
                    got: overflow.len(),
                });
            }
        }
        let keyword_overflow = matched.keyword_overflow;
        if let Some(element) = kw_variadic {
            for &position in &keyword_overflow {
                let expression = &kwargs[position].value;
                let actual = self.infer(expression)?;
                if !self.value_coerces(&actual, element)
                    && self.implicit_conversion_target(&actual, element)?.is_none()
                {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: actual.to_string(),
                        context: "keyword variadic method argument".to_string(),
                    });
                }
                self.check_consuming(
                    expression,
                    &actual,
                    &format!("keyword '{}' collected by method", kwargs[position].name),
                )?;
                score += conversion_count(&actual, element);
            }
            if let Some(actual) = forwarded_element
                && actual != *element
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("StringDict[{element}]"),
                    found: format!("StringDict[{actual}]"),
                    context: "forwarded keyword arguments to method".to_string(),
                });
            }
        }
        Ok(MethodCallScore {
            rank: overload_rank(score, variadic.is_some() || kw_variadic.is_some(), 0, false),
            slots,
            positional_overflow: overflow,
            keyword_overflow,
        })
    }

    /// Type a static method on a parameterized built-in type. Currently only
    /// the compiler-private heap primitive
    /// `UnsafePointer[T].alloc(count: Int) -> UnsafePointer[T]` (plus
    /// `alloc_aligned` and `dangling`), reachable only from bundled
    /// standard-library sources — the audited Mojo head rejects the static
    /// allocation spelling, so user code allocates through `std.memory`.
    pub(super) fn infer_static_method(
        &self,
        tyname: &str,
        targs: &[crate::ast::ParamArg],
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

    /// Type a `Pointer[T]` instance method: the public `unsafe_*` operation
    /// vocabulary (offset, write, take/deinit pointee, free) plus the
    /// deprecated `free()` bridge. Indexed load/store remain ordinary public
    /// pointer subscript syntax.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_pointer_method(
        &self,
        span: &SourceSpan,
        method: &str,
        elem: &Ty,
        origin: &crate::origin::PointerOrigin,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        // `unsafe_write` accepts the keyword `copy=` overload; every other
        // pointer method is positional-only.
        if !kwargs.is_empty() && method != "unsafe_write" {
            reject_kwargs(kwargs)?;
        }
        // `unsafe_origin_cast` takes its target origin as the sole compile-time
        // parameter argument; no other pointer method is parameterized.
        if !param_args.is_empty() && method != "unsafe_origin_cast" {
            return Err(TypeError::BadCall {
                func: format!("Pointer.{method}"),
                reason: "compile-time parameter arguments are not supported here".to_string(),
            });
        }
        match method {
            // Provenance rebind (current Mojo's `unsafe_origin_cast`
            // vocabulary): the runtime value is unchanged; only the checked
            // origin moves. The cast cannot upgrade a statically immutable
            // capability.
            "unsafe_origin_cast" => {
                let [target] = param_args else {
                    return Err(TypeError::BadCall {
                        func: "Pointer.unsafe_origin_cast".to_string(),
                        reason: "expected exactly one origin parameter argument".to_string(),
                    });
                };
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "unsafe_origin_cast".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                let target = self.pointer_origin_arg(target)?;
                // A parametric target mutability is not an upgrade: it
                // resolves from the receiver at each concrete site.
                if origin.statically_mutable() == Some(false)
                    && target.statically_mutable() == Some(true)
                {
                    return Err(TypeError::Unsupported(
                        "an origin cast cannot upgrade capability: the source Pointer \
                         origin is immutable"
                            .to_string(),
                    ));
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerOriginCast {
                        origin: target.clone(),
                    },
                );
                Ok(Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: target,
                })
            }
            "unsafe_offset" => {
                if origin.as_origin().is_some() && !origin.multi_element() {
                    return Err(TypeError::Unsupported(
                        "pointer arithmetic and comparison are not supported on an \
                         origin-bearing Pointer to a single place"
                            .to_string(),
                    ));
                }
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: "unsafe_offset".to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let offset = self.infer(&args[0])?;
                if !coerces(&offset, &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: offset.to_string(),
                        context: "argument to 'Pointer.unsafe_offset'".to_string(),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerOffset,
                );
                Ok(Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: origin.clone(),
                })
            }
            "unsafe_write" => {
                self.check_pointer_write(origin)?;
                let (value, copy) = match (args, kwargs) {
                    ([value], []) => (value, false),
                    ([], [keyword]) if keyword.name == "copy" => (&keyword.value, true),
                    _ => {
                        return Err(TypeError::BadCall {
                            func: "Pointer.unsafe_write".to_string(),
                            reason: "expected one positional value or a single 'copy=' \
                                     keyword argument"
                                .to_string(),
                        });
                    }
                };
                if copy && !self.is_copyable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "Copyable".to_string(),
                        reason: self.trait_failure_reason(elem, "Copyable"),
                    });
                }
                if copy {
                    self.copyable_reference_result_reads
                        .borrow_mut()
                        .insert(value.source_span());
                }
                let vty = self.infer(value)?;
                if !coerces(&vty, elem) {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: vty.to_string(),
                        context: "value written through 'Pointer.unsafe_write'".to_string(),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::PointerWrite {
                        element: elem.clone(),
                        copy,
                    },
                );
                Ok(Ty::None)
            }
            "unsafe_take_pointee" | "unsafe_deinit_pointee" => {
                if !matches!(
                    origin,
                    crate::origin::PointerOrigin::Untracked { mutable: true }
                ) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() requires an allocation-owning Pointer with a \
                         mutable untracked origin; an origin-bearing Pointer's \
                         pointee is owned by its checked storage"
                    )));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if method == "unsafe_deinit_pointee" && !self.is_deinitable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "Deinitable".to_string(),
                        reason: self.trait_failure_reason(elem, "Deinitable"),
                    });
                }
                let adjustment = if method == "unsafe_take_pointee" {
                    crate::checked::SemanticAdjustment::PointerStorageTake {
                        element: elem.clone(),
                    }
                } else {
                    crate::checked::SemanticAdjustment::PointerStorageDestroy {
                        element: elem.clone(),
                    }
                };
                self.operation_adjustments
                    .borrow_mut()
                    .insert(span.clone(), adjustment);
                Ok(if method == "unsafe_take_pointee" {
                    elem.clone()
                } else {
                    Ty::None
                })
            }
            "free" | "unsafe_free" => {
                if origin.as_origin().is_some() {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() is not supported on an origin-bearing Pointer; \
                         it does not own an allocation"
                    )));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: Ty::Pointer {
                    element: Box::new(elem.clone()),
                    origin: origin.clone(),
                }
                .to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type a compiler-private inline uninit-storage method
    /// (`__UninitStorage[T]`'s `unsafe_write`/`take`/`destroy`), reachable
    /// only from the bundled crossing module. `take` and `destroy` consume
    /// their receiver, so they require an explicit `^` transfer; `unsafe_write`
    /// deliberately performs no lifecycle check on (and no drop of) a
    /// previously written payload — it leaks by design, like upstream.
    fn infer_uninit_storage_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        element: &Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !self.bundled_stdlib_declaration {
            return Err(TypeError::Unsupported(format!(
                "'{}' is compiler-private storage; use MaybeUninit from std.memory",
                crate::types::UNINIT_STORAGE_TYPE_NAME
            )));
        }
        let storage_display = || format!("{}[{element}]", crate::types::UNINIT_STORAGE_TYPE_NAME);
        match method {
            "unsafe_write" => {
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let found = self.infer(&args[0])?;
                if !coerces(&found, element) {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: found.to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
                self.operation_adjustments.borrow_mut().insert(
                    span.clone(),
                    crate::checked::SemanticAdjustment::UninitStorageWrite {
                        element: element.clone(),
                    },
                );
                Ok(Ty::None)
            }
            "take" | "destroy" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if !matches!(object.kind, ExprKind::Transfer(_)) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() consumes its storage; transfer it explicitly (`storage^.{method}()`)"
                    )));
                }
                if method == "destroy" && !self.is_deinitable(element) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: element.to_string(),
                        trait_name: "Deinitable".to_string(),
                        reason: self.trait_failure_reason(element, "Deinitable"),
                    });
                }
                let adjustment = if method == "take" {
                    crate::checked::SemanticAdjustment::UninitStorageTake {
                        element: element.clone(),
                    }
                } else {
                    crate::checked::SemanticAdjustment::UninitStorageDestroy {
                        element: element.clone(),
                    }
                };
                self.operation_adjustments
                    .borrow_mut()
                    .insert(span.clone(), adjustment);
                Ok(if method == "take" {
                    element.clone()
                } else {
                    Ty::None
                })
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: storage_display(),
                method: method.to_string(),
            }),
        }
    }

    /// If `recv` is a `struct` defining dunder method `name`, type the implicit
    /// call `recv.name(args…)` — the operator / subscript / builtin dispatch that
    /// turns a user struct into a first-class value type. Checks arity and argument
    /// coercion and returns the (type-argument-substituted) result type. Returns
    /// `None` when `recv` isn't a struct or has no such method, so the caller falls
    /// back to its own operator/builtin error.
    pub(super) fn struct_dunder(
        &self,
        recv: &Ty,
        name: &str,
        args: &[&Ty],
    ) -> Option<Result<Ty, TypeError>> {
        let (info, sig, targs) = self.struct_dunder_signature(recv, name, args.len())?;
        let params: Vec<Ty> = sig
            .params
            .iter()
            .map(|t| substitute_at(t, &info.decls, targs))
            .collect();
        if params.len() != args.len() {
            return Some(Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: params.len(),
                got: args.len(),
            }));
        }
        for (arg, expected) in args.iter().zip(&params) {
            if !coerces(arg, expected) {
                return Some(Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: arg.to_string(),
                    context: format!("argument to '{name}'"),
                }));
            }
        }
        Some(Ok(substitute_at(&sig.ret, &info.decls, targs)))
    }

    /// Resolve the declaration selected by implicit dunder dispatch. Callers
    /// that need convention semantics must inspect this signature before using
    /// the type-only `struct_dunder` result.
    pub(super) fn struct_dunder_signature<'a>(
        &'a self,
        recv: &'a Ty,
        name: &str,
        arity: usize,
    ) -> Option<(&'a StructInfo, &'a MethodSig, &'a [TyArg])> {
        let Ty::Struct(sname, targs) = recv else {
            return None;
        };
        let info = self.structs.get(sname)?;
        let sig = info
            .methods
            .get(name)?
            .iter()
            .find(|sig| sig.params.len() == arity)?;
        Some((info, sig, targs))
    }

    /// Type a `List` method call. The **mutating** methods (`append`, `insert`,
    /// `remove`, `pop`, `clear`, `reverse`, `extend`) require a plain variable
    /// receiver (so they can mutate its binding in place); the **query** methods
    /// (`count`, `index`) work on any list. `remove`/`count`/`index` require an
    /// equatable element type.
    pub(super) fn infer_list_method(
        &self,
        object: &Expr,
        method: &str,
        elem: &Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let no_such = || TypeError::NoSuchMethod {
            object_type: list_type(elem.clone()).to_string(),
            method: method.to_string(),
        };
        let mutating = matches!(
            method,
            "append" | "insert" | "remove" | "pop" | "clear" | "reverse" | "extend"
        );
        // A mutating method mutates its receiver, so the receiver must be a
        // writable place (a variable or a field/index chain rooted at one) —
        // not a temporary. Reading `check_place` validates exactly that.
        if mutating && self.check_place(object).is_err() {
            return Err(TypeError::MutationRequiresVariable(method.to_string()));
        }
        // `remove`/`count`/`index` compare elements, so require an equatable type.
        if matches!(method, "remove" | "count" | "index") && !is_list_equatable(elem) {
            return Err(TypeError::TypeMismatch {
                expected: "an equatable element type".to_string(),
                found: elem.to_string(),
                context: format!("'{}'", method),
            });
        }
        // Require the argument at position `i` to coerce to the element type.
        let expect_elem = |tys: &[Ty], i: usize| -> Result<(), TypeError> {
            if coerces(&tys[i], elem) {
                Ok(())
            } else {
                Err(TypeError::TypeMismatch {
                    expected: elem.to_string(),
                    found: tys[i].to_string(),
                    context: format!("argument to '{}'", method),
                })
            }
        };
        match method {
            "append" => {
                let tys = self.builtin_args("append", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
            }
            "insert" => {
                let tys = self.builtin_args("insert", 2, args)?;
                if !coerces(&tys[0], &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: tys[0].to_string(),
                        context: "insert index".to_string(),
                    });
                }
                expect_elem(&tys, 1)?;
                Ok(Ty::None)
            }
            "remove" => {
                let tys = self.builtin_args("remove", 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::None)
            }
            "pop" => {
                // `pop()` (last) or `pop(i)` — an optional `Int` index.
                if args.len() > 1 {
                    return Err(TypeError::ArityMismatch {
                        name: "pop".into(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                if let Some(a) = args.first() {
                    let ity = self.infer(a)?;
                    if !coerces(&ity, &Ty::Int) {
                        return Err(TypeError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: ity.to_string(),
                            context: "pop index".to_string(),
                        });
                    }
                }
                Ok(elem.clone())
            }
            "clear" | "reverse" => {
                self.builtin_args(method, 0, args)?;
                Ok(Ty::None)
            }
            "extend" => {
                let tys = self.builtin_args("extend", 1, args)?;
                let expected = list_type(elem.clone());
                if tys[0] != expected {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'extend'".to_string(),
                    });
                }
                Ok(Ty::None)
            }
            "count" | "index" => {
                let tys = self.builtin_args(method, 1, args)?;
                expect_elem(&tys, 0)?;
                Ok(Ty::Int)
            }
            _ => Err(no_such()),
        }
    }

    /// Type the value-producing Tuple helpers in the current builtin surface.
    pub(super) fn infer_tuple_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elements: &[Ty],
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            parameterized_syntax,
            ..
        } = call;
        let receiver_implicitly_copyable = elements
            .iter()
            .all(|element| self.is_implicitly_copyable(element));
        match method {
            "reverse" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.reverse".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                self.builtin_args("reverse", 0, args)?;
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'reverse' must be transferred with '^'"
                                .to_string(),
                    });
                }
                Ok(nominal_tuple_type(elements.iter().rev().cloned().collect()))
            }
            "concat" => {
                if !param_args.is_empty() {
                    return Err(TypeError::WrongTypeArgCount {
                        name: "Tuple.concat".to_string(),
                        expected: 0,
                        got: param_args.len(),
                    });
                }
                let tys = self.builtin_args("concat", 1, args)?;
                let Some(other) = tuple_elements(&tys[0]) else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a Tuple".to_string(),
                        found: tys[0].to_string(),
                        context: "argument to 'concat'".to_string(),
                    });
                };
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context:
                            "consuming receiver of method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                if is_place_expr(&args[0])
                    && !other
                        .iter()
                        .all(|element| self.is_implicitly_copyable(element))
                {
                    return Err(TypeError::NonCopyable {
                        ty: tys[0].to_string(),
                        context:
                            "deinit argument 1 to method 'concat' must be transferred with '^'"
                                .to_string(),
                    });
                }
                let mut result = elements.to_vec();
                result.extend(other.into_iter().cloned());
                Ok(nominal_tuple_type(result))
            }
            "consume_elements" | "deinit_with" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: format!("Tuple.{method}"),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context: format!(
                            "consuming receiver of method '{method}' must be transferred with '^'"
                        ),
                    });
                }
                let index_decl = ParamDecl::Value {
                    name: "index".to_string(),
                    ty: Box::new(Ty::Int),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                };
                let handler = Ty::GenericFunc {
                    environment: crate::origin::CallableEnvironment::Capturing(
                        crate::origin::CaptureOriginSet::empty(),
                    ),
                    decls: vec![index_decl],
                    params: vec![Ty::Dependent(DependentType::Indexed {
                        elements: elements.to_vec(),
                        index: CtExpr::Param("index".to_string()),
                    })],
                    names: vec!["element".to_string()],
                    ret: Box::new(Ty::None),
                    required: vec![true],
                    variadic: None,
                    kw_variadic: None,
                    positional_only: None,
                    keyword_only: None,
                    raises: false,
                    error: None,
                    conventions: vec![Some(ArgConvention::Var)],
                    ref_params: Box::new(vec![None]),
                    ref_return: None,
                    transfers: Default::default(),
                };
                let method_decls = vec![ParamDecl::Value {
                    name: "elt_handler".to_string(),
                    ty: Box::new(handler),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                }];
                self.resolve_use_params(
                    &format!("Tuple.{method}"),
                    &method_decls,
                    param_args,
                    &[],
                    &[],
                )?;
                if parameterized_syntax {
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::ParameterizedMethodCall {
                            param_decls: method_decls,
                        },
                    );
                }
                Ok(Ty::None)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: nominal_tuple_type(elements.to_vec()).to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type a call through a callable-typed struct field: validate the
    /// arguments against the stored contract, replay any value-carried
    /// transfer effects, and mark the expression for indirect lowering.
    fn infer_field_invocation(
        &self,
        span: SourceSpan,
        _object: &Expr,
        callable: Ty,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let (ret, _, error, _) =
            self.infer_callable_ty(&span, "<callable>", callable.clone(), &[], args, kwargs)?;
        self.record_call_environment_effects(span.clone(), &callable, &[], args, kwargs)?;
        let carried = contract_transfer_effects(&callable);
        if !carried.is_empty() {
            self.replay_transfer_effects(carried, None, args, &span)?;
        }
        if let Some(target) = self.indirect_callable_target(&callable) {
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target);
        }
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            crate::checked::SemanticAdjustment::FieldInvocation {
                callable: callable.clone(),
            },
        );
        if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
            self.record_call_effect(span.clone(), error.clone());
            self.require_error("call through a stored callable field", error)?;
        }
        Ok(ret)
    }
}
