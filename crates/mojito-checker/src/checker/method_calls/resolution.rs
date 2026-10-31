//! Resolving a method call to one signature by receiver family, and
//! retargeting it to a minted clone.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::checker::call_inference::folded_parameter_arguments;
use mojito_types::types::TyKind;

impl Checker {
    /// Resolve the method to a concrete signature (params + return + whether
    /// it mutates `self`) for this receiver, substituting the receiver's type
    /// arguments (struct) or `Self` (a bounded type parameter's trait method).
    /// Multi-candidate failure ordering remains the ordinary overload
    /// diagnostic; `availability_failure` lets a sole shape explain that its
    /// availability predicate rejected.
    pub(super) fn resolve_receiver_method(
        &self,
        site: MethodCallSite<'_>,
        availability_failure: &mut Option<String>,
    ) -> Result<MethodSelection, TypeError> {
        match site.obj_ty.kind() {
            TyKind::Struct(sname, targs) => {
                self.resolve_struct_method(site, sname, targs, availability_failure)
            }
            TyKind::Param { .. } | TyKind::Assoc { .. } => {
                self.resolve_bound_method(site, availability_failure)
            }
            _ => match self.declared_receiver_member(site.obj_ty, site.method) {
                Some((sname, targs)) => {
                    self.resolve_struct_method(site, sname, &targs, availability_failure)
                }
                None => self.resolve_builtin_method(site),
            },
        }
    }

    /// The declared struct `ty` is an instance of, and its arguments, when
    /// the checker registered it: a struct's own, and a SIMD-valued type's
    /// as `SIMD`'s ([`mojito_types::types::nominal_parts`]).
    pub(in crate::checker) fn nominal_struct<'t>(
        &self,
        ty: &'t Ty,
    ) -> Option<(&'t str, std::borrow::Cow<'t, [TyArg]>)> {
        mojito_types::types::nominal_parts(ty).filter(|(name, _)| self.structs.contains_key(name))
    }

    /// The declaration a compiler-typed receiver's member `method` is read
    /// from before the compiler's rule: `SIMD`'s, for a `TyKind::Simd`
    /// receiver whose struct declares it, and `StringLiteral`'s, for a
    /// string literal whose type carries its value. A numeric literal
    /// receiver is no instance (`(5).__len__()` has no such attribute at
    /// the pin), and neither is the value-less `StringLiteral`.
    pub(in crate::checker) fn declared_receiver_member<'t>(
        &self,
        ty: &'t Ty,
        method: &str,
    ) -> Option<(&'t str, std::borrow::Cow<'t, [TyArg]>)> {
        if !matches!(
            ty.kind(),
            TyKind::Simd { .. } | TyKind::StringLiteral(Some(_))
        ) {
            return None;
        }
        self.nominal_struct(ty).filter(|(name, _)| {
            self.structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key(method))
        })
    }

    /// A name no signature answers: a callable-typed field invoked through
    /// the receiver.
    pub(super) fn infer_unresolved_method(
        &self,
        site: MethodCallSite<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            args,
            kwargs,
            parameterized_syntax,
            ..
        } = call;
        // A callable-typed FIELD dispatches indirectly:
        // `holder.callback(1)` loads the stored value and calls
        // through it (thin or capturing) — the field-invocation
        // channel.
        if !parameterized_syntax
            && let TyKind::Struct(sname, targs) = obj_ty.kind()
            && let Some(info) = self.structs.get(sname)
            && let Some((_, field_ty)) = info.fields.iter().find(|(fname, _)| fname == method)
        {
            let field_ty = substitute_at(field_ty, info, targs);
            if callable_contract_ty(&field_ty).is_some() {
                return self.infer_field_invocation(span.clone(), object, &field_ty, args, kwargs);
            }
        }
        Err(TypeError::NoSuchMethod {
            object_type: obj_ty.to_string(),
            method: method.to_string(),
        })
    }

    /// Record a generic method's resolved compile-time arguments, and a
    /// concrete generic-struct receiver's instantiation, as the call's
    /// checked facts.
    pub(super) fn record_method_call_instantiation(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) {
        let MethodCallSite {
            span,
            method,
            call,
            obj_ty,
            ..
        } = site;
        let param_args = call.param_args;
        if let (Some((sname, targs)), Some(arguments)) =
            (self.nominal_struct(obj_ty), &resolved.instantiation)
        {
            // On a closed instance of an ordinary generic struct the
            // instantiation is keyed by the instance: its arguments come
            // before the call's.
            let owner_arguments = self.instance_arguments(sname, &targs).unwrap_or_default();
            let overload = resolved
                .lowered_name
                .as_deref()
                .and_then(mojito_symbol::symbol::overload_qualifier)
                .map(str::to_string);
            self.record_dtype_parameter_arguments(param_args, arguments);
            self.record_at(span.clone()).method_instantiation =
                Some(mojito_checked::checked::MethodInstantiation {
                    owner: sname.to_string(),
                    owner_arguments,
                    method: method.to_string(),
                    parameter_names: resolved.parameter_names.clone(),
                    overload,
                    arguments: arguments.clone(),
                    folded_arguments: folded_parameter_arguments(
                        &resolved.param_decls,
                        param_args,
                        arguments,
                    ),
                });
        }
        if let TyKind::Struct(sname, targs) = obj_ty.kind()
            && !targs.is_empty()
        {
            self.record_struct_instantiation(sname, targs, span.source.as_deref());
        }
    }

    /// A nominal struct's declared method, substituted at the receiver's type
    /// arguments.
    fn resolve_struct_method(
        &self,
        site: MethodCallSite<'_>,
        sname: &str,
        targs: &[TyArg],
        availability_failure: &mut Option<String>,
    ) -> Result<MethodSelection, TypeError> {
        let MethodCallSite {
            object,
            method,
            call,
            ..
        } = site;
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
        let Some(sigs) = info.methods.get(method) else {
            return Ok(Ok(None));
        };
        let overloaded = sigs.len() > 1;
        let has_instance_candidate = sigs.iter().any(|sig| sig.has_self);
        let mut matches = Vec::new();
        for sig in sigs
            .iter()
            .filter(|sig| sig.has_self || !has_instance_candidate)
        {
            let receiver_params: Vec<Ty> = sig
                .params
                .iter()
                .map(|t| self.resolve_assoc_ty(&substitute_at(t, info, targs)))
                .collect();
            let receiver_variadic = sig
                .variadic
                .as_ref()
                .map(|ty| self.resolve_assoc_ty(&substitute_variadic_at(ty, info, targs)));
            let receiver_kw_variadic = sig
                .kw_variadic
                .as_ref()
                .map(|ty| self.resolve_assoc_ty(&substitute_at(ty, info, targs)));
            // A value parameter typed over the struct's pack (a handler
            // `def[index: Int](var element: Self.Ts[index])`) is judged at
            // the receiver's elements.
            let at_receiver = self.method_decls_at_receiver(sig, info, targs);
            let (params, variadic, kw_variadic, method_subst, method_arguments, bracket_attributes) =
                match self.instantiate_method_generics(
                    &format!("{sname}.{method}"),
                    at_receiver.as_ref().unwrap_or(sig),
                    &receiver_params,
                    receiver_variadic.as_ref(),
                    receiver_kw_variadic.as_ref(),
                    param_args,
                    args,
                    kwargs,
                ) {
                    Ok(instantiated) => instantiated,
                    // A runtime value where a subscript accessor takes a
                    // compile-time `Int` (`t[i]` over a runtime `i`) is the
                    // reason the call reports.
                    Err(TypeError::NotComptime(_))
                        if !overloaded
                            && matches!(method, "__getitem__" | "__getitem_param__")
                            && matches!(sig.decls.as_slice(),
                            [decl @ ParamDecl::Value { .. }]
                                if decl.value_element_ty().is_some_and(|ty| **ty == *TyKind::INT)) =>
                    {
                        availability_failure.get_or_insert_with(|| {
                            "expected a compile-time Int index, found a runtime value".to_string()
                        });
                        continue;
                    }
                    Err(failure) => {
                        if !overloaded
                            && let Some(reason) = collector_element_failure(sig, &failure)
                        {
                            availability_failure.get_or_insert(reason);
                        }
                        continue;
                    }
                };
            let instantiation = method_instantiation_arguments(sig, &method_arguments);
            if let Err(failure) =
                self.method_constraint_result(sig, &method_arguments, &info.decls, targs)
            {
                // A candidate the arguments would have
                // selected reports its failed availability
                // clause even among overloads (the
                // where-gated `set` pair of `Variant`).
                if availability_failure.is_none()
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
                    *availability_failure = Some(failure.reason());
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
                    // A pack element closes under the receiver's and the
                    // call's own arguments at once: an argument may name the
                    // caller's binder of the same identity
                    // (`result[Self.Ts.length + i]` inside a method of the
                    // struct), which the receiver's must not rewrite.
                    return_type: self.close_method_values(
                        &self.close_pack_elements(
                            substitute(
                                &super::super::generics::expand_solved_packs(
                                    &substitute_at(&sig.ret, info, targs),
                                    &sig.decls,
                                    &method_arguments,
                                ),
                                &method_subst,
                            ),
                            &[
                                (&sig.decls, &method_arguments),
                                (&info.decls, &positional_pack_arguments(&info.decls, targs)),
                            ],
                        ),
                        &sig.decls,
                        &method_arguments,
                    ),
                    result_adapter: None,
                    raises: sig.raises,
                    error: sig
                        .error
                        .as_ref()
                        .map(|error| substitute(&substitute_at(error, info, targs), &method_subst)),
                    mutates_receiver: matches!(
                        sig.self_convention,
                        Some(mojito_ast::ast::ArgConvention::Mut)
                    ),
                    consumes_receiver: matches!(
                        sig.self_convention,
                        Some(
                            mojito_ast::ast::ArgConvention::Var
                                | mojito_ast::ast::ArgConvention::Deinit
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
                    instantiation: instantiation.clone(),
                    parameter_names: sig.names.clone(),
                    view_return_interior: sig.view_return_interior.clone(),
                    view_return: sig.view_return.clone(),
                    declared_return: Some(sig.ret.clone()),
                    declared_params: sig.params.clone(),
                    nested_origins: sig.nested_origins,
                    bracket_attributes,
                    refined_bound: None,
                    param_types: params,
                    param_decls: sig.decls.clone(),
                });
            }
        }
        Ok(select_method_overload(
            method,
            matches,
            Some(matches!(object.kind, ExprKind::Transfer(_))),
        )
        .map(Some))
    }

    /// A bounded type parameter's (or associated type's) trait requirement,
    /// with `Self` substituted to the receiver.
    fn resolve_bound_method(
        &self,
        site: MethodCallSite<'_>,
        availability_failure: &mut Option<String>,
    ) -> Result<MethodSelection, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        // A `where conforms_to(T, Trait)` clause or `comptime if` the body
        // is checked under declares `Trait`'s requirements on `T` as a bound
        // does.
        let effective_bounds = self.refined_bound(obj_ty).unwrap_or_default();
        let refined_bound = self.recorded_refined_bound(obj_ty);
        let signatures = self.lookup_trait_methods(&effective_bounds, method, args.len());
        if signatures.is_empty() {
            return Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            });
        }
        // `origin_of(self)` in the result names the receiver's place; a
        // temporary receiver keeps the abstract origin.
        let receiver_origin = self
            .reference_actual(object)
            .ok()
            .map(|reference| reference.origin);
        let single_candidate = signatures.len() == 1;
        let mut matches = Vec::new();
        let mut requirements = Vec::new();
        for sig in signatures {
            let receiver_params: Vec<_> = sig
                .params
                .iter()
                .map(|ty| self.substitute_self_members(ty, obj_ty))
                .collect();
            let receiver_variadic = sig
                .variadic
                .as_ref()
                .map(|ty| self.substitute_self_members(ty, obj_ty));
            let receiver_kw_variadic = sig
                .kw_variadic
                .as_ref()
                .map(|ty| self.substitute_self_members(ty, obj_ty));
            let (params, variadic, kw_variadic, method_subst, method_arguments, bracket_attributes) =
                match self.instantiate_method_generics(
                    &format!("{obj_ty}.{method}"),
                    &sig,
                    &receiver_params,
                    receiver_variadic.as_ref(),
                    receiver_kw_variadic.as_ref(),
                    param_args,
                    args,
                    kwargs,
                ) {
                    Ok(instantiated) => instantiated,
                    Err(failure) => {
                        if single_candidate
                            && let Some(reason) = collector_element_failure(&sig, &failure)
                        {
                            availability_failure.get_or_insert(reason);
                        }
                        continue;
                    }
                };
            if let Err(failure) = self.method_constraint_result(&sig, &method_arguments, &[], &[]) {
                if single_candidate
                    && availability_failure.is_none()
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
                    *availability_failure = Some(failure.reason());
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
            let return_type = self.close_method_values(
                &substitute(
                    &self.substitute_self_members(&sig.ret, obj_ty),
                    &method_subst,
                ),
                &sig.decls,
                &method_arguments,
            );
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
                return_type: receiver_origin.as_ref().map_or_else(
                    || return_type.clone(),
                    |origin| {
                        crate::checker::origins::bind_self_origin_arguments(&return_type, origin)
                    },
                ),
                result_adapter: (method == "__next__" && sig.ref_return.is_none()).then_some(
                    mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference,
                ),
                raises: sig.raises,
                error: sig.error.as_ref().map(|error| {
                    self.close_method_values(
                        &substitute(&self.substitute_self_members(error, obj_ty), &method_subst),
                        &sig.decls,
                        &method_arguments,
                    )
                }),
                mutates_receiver: matches!(
                    sig.self_convention,
                    Some(mojito_ast::ast::ArgConvention::Mut)
                ),
                consumes_receiver: matches!(
                    sig.self_convention,
                    Some(
                        mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                ),
                // Abstract dispatch keeps `TyKind::SelfType` in `sig`, which
                // already spells `Self`; the runtime retargets the receiver
                // prefix once the concrete type is known.
                lowered_name: Some(method_lowered_name("__trait_dispatch", method, &sig, None)),
                ref_params: sig.ref_params.clone(),
                ref_return: sig.ref_return.clone(),
                param_types: params,
                param_decls: sig.decls.clone(),
                parametric_origin_writes: sig.parametric_origin_writes.clone(),
                instantiation: None,
                parameter_names: Vec::new(),
                view_return_interior: Vec::new(),
                view_return: Vec::new(),
                declared_return: None,
                declared_params: Vec::new(),
                nested_origins: NestedOrigins::AsDeclared,
                bracket_attributes,
                refined_bound: refined_bound.clone(),
            });
            requirements.push((sig, method_arguments));
        }
        let selected = select_method_overload(
            method,
            matches,
            Some(matches!(object.kind, ExprKind::Transfer(_))),
        );
        if let Ok(selected) = &selected
            && let Some((requirement, solved)) = requirements.iter().find(|(sig, _)| {
                selected.lowered_name.as_deref()
                    == Some(&method_lowered_name("__trait_dispatch", method, sig, None))
            })
        {
            self.record_bound_default_arguments(
                &bound_defaults::BoundCall {
                    span,
                    slots: &selected.slots,
                    positional: args.len(),
                    solved,
                },
                &effective_bounds,
                method,
                requirement,
            )?;
        }
        Ok(selected.map(Some))
    }

    /// The methods a built-in value answers with no callee: `copy`,
    /// `__hash__`, and the `std.math` operation dunders.
    fn resolve_builtin_method(
        &self,
        site: MethodCallSite<'_>,
    ) -> Result<MethodSelection, TypeError> {
        let MethodCallSite {
            object,
            method,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        // `x.copy()` on a built-in copyable value (a scalar, literal,
        // tuple, or variant) is `Copyable.copy` with no callee: the copy
        // is the value read itself, and MIR lowers it as one.
        if method == "copy"
            && args.is_empty()
            && kwargs.is_empty()
            && param_args.is_empty()
            && builtin_copy_is_value_read(obj_ty)
            && self.is_copyable(obj_ty)
        {
            // A place receiver copies out of its storage exactly like an
            // implicit place copy of an `ImplicitlyCopyable` value.
            if self.designates_place(object) {
                self.record_at(object.source_span()).copy_place_value_use = true;
            }
            return Ok(Ok(Some(intrinsic_resolution(
                obj_ty.clone(),
                vec![],
                vec![],
            ))));
        }
        // A `Bool`, `DType`, or `StringLiteral` leaf hashes into the
        // caller-provided hasher: the first two through its
        // `_update_with_simd` at `hash_leaf_ty`'s type, a string literal
        // through the nominal `String`'s `__hash__`. A `SIMD`'s `__hash__`
        // is its declared member. The `mut` argument is a place so its
        // updated state is committed by ordinary call lowering.
        if method == "__hash__"
            && args.len() == 1
            && kwargs.is_empty()
            && param_args.is_empty()
            && builtin_hashable_ty(obj_ty)
        {
            let hasher = self.infer(&args[0])?;
            let hasher_trait = super::super::hasher_trait();
            if !self.conforms_to(&hasher, &hasher_trait) {
                return Err(TypeError::TraitNotSatisfied {
                    param: "hasher".to_string(),
                    ty: hasher.to_string(),
                    reason: self.trait_failure_reason(&hasher, &hasher_trait),
                    trait_name: hasher_trait,
                });
            }
            self.check_place(&args[0])?;
            return Ok(Ok(Some(intrinsic_resolution(
                Ty::none(),
                vec![Some(ArgConvention::Mut)],
                vec![hasher],
            ))));
        }
        // A scalar's `std.math` operation dunders are the methods upstream's
        // `SIMD` and `IntLiteral` declare for the trait owning each; the
        // backends answer them intrinsically. A `SIMD`'s `__abs__`,
        // `__floor__`, `__ceil__`, and `__trunc__` are its declared members
        // and never reach here, so for those only `IntLiteral` does.
        if kwargs.is_empty()
            && param_args.is_empty()
            && let Some(trait_name) = math_operation_requirement(method, args.len())
            && scalar_declares_math_trait(obj_ty, trait_name)
        {
            let param_types: Vec<Ty> = args
                .iter()
                .map(|_| {
                    if method == "__round__" {
                        Ty::int()
                    } else {
                        obj_ty.clone()
                    }
                })
                .collect();
            for (arg, expected) in args.iter().zip(&param_types) {
                let found = self.builtin_args(method, 1, std::slice::from_ref(arg))?;
                if !mojito_types::types::coerces(&found[0], expected) {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found[0].to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
            }
            let return_type = if method == "__divmod__" {
                mojito_types::types::tuple_type(vec![obj_ty.clone(), obj_ty.clone()])
            } else {
                obj_ty.clone()
            };
            return Ok(Ok(Some(intrinsic_resolution(
                return_type,
                vec![None; param_types.len()],
                param_types,
            ))));
        }
        Ok(Ok(None))
    }
}

/// The resolution of a callee-less intrinsic taking `param_types`
/// positionally under `conventions`.
fn intrinsic_resolution(
    return_type: Ty,
    conventions: Vec<Option<ArgConvention>>,
    param_types: Vec<Ty>,
) -> MethodCallResolution {
    MethodCallResolution {
        conversion_score: 0,
        simd_erasures: 0,
        slots: (0..param_types.len())
            .map(mojito_ast::call::ArgSlot::Positional)
            .collect(),
        positional_overflow: vec![],
        keyword_overflow: vec![],
        variadic_element: None,
        variadic_convention: None,
        variadic_name: None,
        keyword_element: None,
        conventions,
        self_convention: None,
        return_type,
        result_adapter: None,
        raises: false,
        error: None,
        mutates_receiver: false,
        consumes_receiver: false,
        lowered_name: None,
        ref_params: vec![],
        ref_return: None,
        param_types,
        param_decls: vec![],
        parametric_origin_writes: vec![],
        instantiation: None,
        parameter_names: Vec::new(),
        view_return_interior: Vec::new(),
        view_return: Vec::new(),
        declared_return: None,
        declared_params: Vec::new(),
        nested_origins: NestedOrigins::AsDeclared,
        bracket_attributes: Vec::new(),
        refined_bound: None,
    }
}

/// The `std.math` trait whose requirement `method` with `argc` arguments
/// is. Every parameter is `Self` but `__round__`'s `ndigits: Int`.
fn math_operation_requirement(method: &str, argc: usize) -> Option<&'static str> {
    Some(match (method, argc) {
        ("__abs__", 0) => "Absable",
        ("__ceil__", 0) => "Ceilable",
        ("__floor__", 0) => "Floorable",
        ("__trunc__", 0) => "Truncable",
        ("__round__", 0 | 1) => "Roundable",
        ("__ceildiv__", 1) => "CeilDivable",
        ("__divmod__", 1) => "DivModable",
        _ => return None,
    })
}

/// Whether the compiler-modelled scalar `ty` declares the `std.math` trait
/// `trait_name`: `SIMD` all of them, `IntLiteral` only `Ceilable`,
/// `Floorable`, and `Truncable`, and `FloatLiteral` none.
fn scalar_declares_math_trait(ty: &Ty, trait_name: &str) -> bool {
    match ty.kind() {
        TyKind::IntLiteral => matches!(trait_name, "Ceilable" | "Floorable" | "Truncable"),
        TyKind::FloatLiteral => false,
        _ => mojito_types::conformance::modelled_declares(ty, trait_name, &mut |_, _| true),
    }
}

/// Upstream's reason for an argument a pack collector's element bound
/// rejects (`*args: *Ts` over `*Ts: Writable`), when `failure` is one.
fn collector_element_failure(sig: &MethodSig, failure: &TypeError) -> Option<String> {
    let TypeError::TraitNotSatisfied {
        param,
        ty,
        trait_name,
        ..
    } = failure
    else {
        return None;
    };
    let collector = sig.variadic_name.as_ref()?;
    sig.decls
        .iter()
        .any(|decl| decl.name() == param && matches!(decl, ParamDecl::Type { variadic: true, .. }))
        .then(|| {
            format!(
                "an element of '{collector}' with type '{ty}' does not conform to trait \
                 '{trait_name}'; either prove the conformance with 'conforms_to', or add \
                 conformance"
            )
        })
}
