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
            return self.infer_static_method(name, targs, method, args);
        }
        if let ExprKind::Identifier(sname) = &object.kind
            && let Some(info) = self.structs.get(sname)
            && let Some(signatures) = info.methods.get(method)
        {
            let mut matches = Vec::new();
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
                if !self.method_constraints_apply(sig, &method_arguments) {
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
                            Some(method_lowered_name(sname, method, sig))
                        } else if parameterized_syntax {
                            Some(format!("{sname}.{method}"))
                        } else {
                            None
                        },
                        ref_params: sig.ref_params.clone(),
                        ref_return: sig.ref_return.clone(),
                        param_types: params,
                        param_decls: sig.decls.clone(),
                    });
                }
            }
            if !matches.is_empty() {
                let selected =
                    select_method_overload(method, matches).map_err(|kind| TypeError::BadCall {
                        func: format!("{sname}.{method}"),
                        reason: match kind {
                            OverloadSelect::NoMatch => "no overload matches the supplied arguments",
                            OverloadSelect::Ambiguous => "ambiguous overloaded call",
                        }
                        .to_string(),
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
        }
        let obj_ty = self.infer(object)?;
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
        if matches!(&obj_ty, Ty::Struct(name, args) if name == "Optional" && matches!(args.as_slice(), [TyArg::Ty(Ty::Int)]))
        {
            reject_kwargs(kwargs)?;
            return match method {
                "is_some" if args.is_empty() => Ok(Ty::Bool),
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
        // Built-in `UnsafePointer` methods. Raw storage take/destroy are
        // checker-gated compiler-private operations; ordinary user code only
        // sees the public pointer surface.
        if let Ty::Pointer {
            element: elem,
            origin,
        } = &obj_ty
        {
            reject_kwargs(kwargs)?;
            return self.infer_pointer_method(&span, object, method, elem, origin, args);
        }
        // Resolve the method to a concrete signature (params + return + whether
        // it mutates `self`) for this receiver, substituting the receiver's type
        // arguments (struct) or `Self` (a bounded type parameter's trait method).
        let resolved: Result<Option<MethodCallResolution>, OverloadSelect> = match &obj_ty {
            Ty::Struct(sname, targs) => {
                let info = self.structs.get(sname).ok_or_else(|| {
                    TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
                })?;
                match info.methods.get(method) {
                    Some(sigs) => {
                        let overloaded = sigs.len() > 1;
                        let subst = struct_subst(&info.decls, targs);
                        let mut matches = Vec::new();
                        for sig in sigs {
                            let receiver_params: Vec<Ty> =
                                sig.params.iter().map(|t| substitute(t, &subst)).collect();
                            let receiver_variadic =
                                sig.variadic.as_ref().map(|ty| substitute(ty, &subst));
                            let receiver_kw_variadic =
                                sig.kw_variadic.as_ref().map(|ty| substitute(ty, &subst));
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
                            if !self.method_constraints_apply(sig, &method_arguments) {
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
                                        &substitute(&sig.ret, &subst),
                                        &method_subst,
                                    ),
                                    result_adapter: None,
                                    raises: sig.raises,
                                    error: sig.error.as_ref().map(|error| {
                                        Box::new(substitute(
                                            &substitute(error, &subst),
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
                                        Some(method_lowered_name(sname, method, sig))
                                    } else if parameterized_syntax {
                                        Some(format!("{sname}.{method}"))
                                    } else {
                                        None
                                    },
                                    ref_params: sig.ref_params.clone(),
                                    ref_return: sig.ref_return.clone(),
                                    param_types: params,
                                    param_decls: sig.decls.clone(),
                                });
                            }
                        }
                        select_method_overload(method, matches).map(Some)
                    }
                    None => Ok(None),
                }
            }
            Ty::Param { bounds, .. } => {
                let signatures = self.lookup_trait_methods(bounds, method, args.len());
                if signatures.is_empty() {
                    return Err(TypeError::NoSuchMethod {
                        object_type: obj_ty.to_string(),
                        method: method.to_string(),
                    });
                }
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
                    if !self.method_constraints_apply(&sig, &method_arguments) {
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
                        lowered_name: Some(method_lowered_name("__trait_dispatch", method, &sig)),
                        ref_params: sig.ref_params.clone(),
                        ref_return: sig.ref_return.clone(),
                        param_types: params,
                        param_decls: sig.decls.clone(),
                    });
                }
                select_method_overload(method, matches).map(Some)
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
                }))
            }
            _ => Ok(None),
        };
        let resolved = match resolved {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                return Err(TypeError::NoSuchMethod {
                    object_type: obj_ty.to_string(),
                    method: method.to_string(),
                });
            }
            Err(OverloadSelect::NoMatch) => {
                return Err(TypeError::BadCall {
                    func: method.to_string(),
                    reason: "no overload matches the supplied arguments".to_string(),
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
        // Replay the callee's loan-transfer effects against the actuals.
        if let Ty::Struct(struct_name, _) = &obj_ty {
            self.apply_transfer_effects(
                &format!("{struct_name}.{method}"),
                Some(object),
                args,
                &span,
            )?;
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
        let effective_receiver_convention = if resolved.self_convention == Some(ArgConvention::Ref)
            && self.reference_actual(object)?.mutability == crate::origin::Mutability::Immutable
        {
            Some(ArgConvention::Read)
        } else {
            resolved.self_convention
        };
        // A `deinit self` call always consumes its receiver. Mojo may satisfy
        // that consumption by implicitly copying an `ImplicitlyCopyable` place;
        // a merely movable (or explicitly-copy-only) place still requires `^`.
        if resolved.consumes_receiver && is_place_expr(object) {
            if !self.is_implicitly_copyable(&obj_ty) {
                return Err(TypeError::NonCopyable {
                    ty: obj_ty.to_string(),
                    context: format!(
                        "consuming receiver of method '{method}' must be transferred with '^'"
                    ),
                });
            }
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span.clone());
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
            match resolved.conventions.get(index).copied().flatten() {
                Some(ArgConvention::Deinit)
                    if is_place_expr(expression) && !self.is_implicitly_copyable(&ty) =>
                {
                    return Err(TypeError::NonCopyable {
                        ty: ty.to_string(),
                        context: format!(
                            "deinit argument {} to method '{}' must be transferred with '^'",
                            index + 1,
                            method
                        ),
                    });
                }
                Some(ArgConvention::Var | ArgConvention::Deinit) => {
                    self.check_consuming(
                        expression,
                        &ty,
                        &format!("argument {} to method '{}'", index + 1, method),
                    )?;
                }
                _ => {}
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
                        && self.is_copyable(
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
            self.selected_calls.borrow_mut().insert(
                span,
                crate::checked::CheckedCallContract {
                    target,
                    raises: call_error,
                    result_ty: reference_result
                        .clone()
                        .map(Ty::Ref)
                        .unwrap_or_else(|| resolved.return_type.clone()),
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
            .unwrap_or(resolved.return_type))
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
    /// `UnsafePointer[T].alloc(count: Int) -> UnsafePointer[T]`.
    pub(super) fn infer_static_method(
        &self,
        tyname: &str,
        targs: &[crate::ast::ParamArg],
        method: &str,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if tyname != "UnsafePointer" {
            return Err(TypeError::NoSuchMethod {
                object_type: format!("{tyname}[…]"),
                method: method.to_string(),
            });
        }
        let ptr_ty = self.pointer_type(targs)?;
        match method {
            "alloc" | "alloc_aligned" => {
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
            "dangling" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(ptr_ty)
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: ptr_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Type an `UnsafePointer[T]` instance method. `take` and `destroy` are raw
    /// initialized-slot operations reserved for the bundled self-hosted
    /// collections; indexed load/store remain ordinary public pointer syntax.
    pub(super) fn infer_pointer_method(
        &self,
        span: &SourceSpan,
        object: &Expr,
        method: &str,
        elem: &Ty,
        origin: &crate::origin::PointerOrigin,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        match method {
            "free" => {
                if origin.as_origin().is_some() {
                    return Err(TypeError::Unsupported(
                        "free() is not supported on an origin-bearing UnsafePointer; \
                         it does not own an allocation"
                            .to_string(),
                    ));
                }
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "free".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Ty::None)
            }
            "take" | "destroy" => {
                if !is_bundled_collection_source(object.source.as_deref()) {
                    return Err(TypeError::NoSuchMethod {
                        object_type: Ty::Pointer {
                            element: Box::new(elem.clone()),
                            origin: origin.clone(),
                        }
                        .to_string(),
                        method: method.to_string(),
                    });
                }
                if !matches!(origin, crate::origin::PointerOrigin::Legacy) {
                    return Err(TypeError::Unsupported(format!(
                        "{method}() is supported only on an allocation-owning \
                         UnsafePointer without an explicit origin"
                    )));
                }
                if args.len() != 1 {
                    return Err(TypeError::ArityMismatch {
                        name: method.to_string(),
                        expected: 1,
                        got: args.len(),
                    });
                }
                let index = self.infer(&args[0])?;
                if !coerces(&index, &Ty::Int) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: index.to_string(),
                        context: format!("argument to compiler-private UnsafePointer.{method}"),
                    });
                }
                if method == "destroy" && !self.is_implicitly_deletable(elem) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "T".to_string(),
                        ty: elem.to_string(),
                        trait_name: "ImplicitlyDeletable".to_string(),
                        reason: self.trait_failure_reason(elem, "ImplicitlyDeletable"),
                    });
                }
                let adjustment = if method == "take" {
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
                Ok(if method == "take" {
                    elem.clone()
                } else {
                    Ty::None
                })
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
        let Ty::Struct(sname, targs) = recv else {
            return None;
        };
        let info = self.structs.get(sname)?;
        let sig = info
            .methods
            .get(name)?
            .iter()
            .find(|sig| sig.params.len() == args.len())?;
        let subst = struct_subst(&info.decls, targs);
        let params: Vec<Ty> = sig.params.iter().map(|t| substitute(t, &subst)).collect();
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
        Some(Ok(substitute(&sig.ret, &subst)))
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
            "consume_elements" => {
                if !args.is_empty() {
                    return Err(TypeError::ArityMismatch {
                        name: "Tuple.consume_elements".to_string(),
                        expected: 0,
                        got: args.len(),
                    });
                }
                if is_place_expr(object) && !receiver_implicitly_copyable {
                    return Err(TypeError::NonCopyable {
                        ty: nominal_tuple_type(elements.to_vec()).to_string(),
                        context: "consuming receiver of method 'consume_elements' must be transferred with '^'"
                            .to_string(),
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
                    "Tuple.consume_elements",
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
}
