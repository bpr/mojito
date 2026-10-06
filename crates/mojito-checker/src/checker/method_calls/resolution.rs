//! Resolving a method call to one signature by receiver family, and
//! retargeting it to a minted clone.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
        match site.obj_ty {
            Ty::Struct(sname, targs) => {
                self.resolve_struct_method(site, sname, targs, availability_failure)
            }
            Ty::Param { .. } | Ty::Assoc { .. } => {
                self.resolve_bound_method(site, availability_failure)
            }
            _ => self.resolve_builtin_method(site),
        }
    }

    /// A name no signature answers: a callable-typed field invoked through
    /// the receiver, or the public Tuple's structural surface.
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
            && let Ty::Struct(sname, targs) = obj_ty
            && let Some(info) = self.structs.get(sname)
            && let Some((_, field_ty)) = info.fields.iter().find(|(fname, _)| fname == method)
        {
            let field_ty = substitute_at(field_ty, info, targs);
            if callable_contract_ty(&field_ty).is_some() {
                return self.infer_field_invocation(span.clone(), object, &field_ty, args, kwargs);
            }
        }
        // A public Tuple no specialization serves yet, or a member its
        // specialization was not asked to keep.
        if let Some(elements) = tuple_elements(obj_ty) {
            reject_kwargs(kwargs)?;
            let elements = elements.into_iter().cloned().collect::<Vec<_>>();
            return self.infer_tuple_member(site, &elements);
        }
        Err(TypeError::NoSuchMethod {
            object_type: obj_ty.to_string(),
            method: method.to_string(),
        })
    }

    /// Type a member call on a public Tuple of `elements` that no
    /// specialization answers: the structural surface
    /// ([`Self::infer_tuple_method`]) first, and what it does not serve from
    /// the member `std/builtin/tuple.mojo` declares, resolved against the
    /// declaration's shell with its pack bound to `elements`.
    pub(super) fn infer_tuple_member(
        &self,
        site: MethodCallSite<'_>,
        elements: &[Ty],
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            ..
        } = site;
        let shell = mojito_symbol::symbol::TUPLE_DECLARATION_SHELL;
        let structural = self.infer_tuple_method(span, object, method, elements, call);
        if !matches!(structural, Err(TypeError::NoSuchMethod { .. }))
            || !self.structs.contains_key(shell)
        {
            return structural;
        }
        // A receiver spreading a pack still open, or a list computed from
        // one, binds the declaration's pack to that spread whole.
        let pack = match elements {
            [spread]
                if mojito_types::types::pack_spread(elements).is_some()
                    || mojito_types::types::list_spread(spread).is_some() =>
            {
                TyArg::Ty(spread.clone())
            }
            _ => TyArg::Val(CtValue::Tuple(
                elements
                    .iter()
                    .cloned()
                    .map(Box::new)
                    .map(CtValue::Type)
                    .collect(),
            )),
        };
        let mut availability_failure = None;
        let selection =
            self.resolve_struct_method(site, shell, &[pack], &mut availability_failure)?;
        let Some(resolved) =
            super::mc_infer::selected_method(method, selection, availability_failure)?
        else {
            return structural;
        };
        let result = self.infer_selected_method_call(site, resolved)?;
        // The instance a member with parameters of its own asks for belongs
        // to the receiver's specialization, which the next round mints.
        if let Some(instantiation) = self.method_instantiations.borrow_mut().get_mut(span)
            && instantiation.owner == mojito_types::types::TUPLE_TYPE_NAME
            && elements
                .iter()
                .all(|element| !mojito_types::types::is_symbolic(element))
        {
            instantiation.owner = mojito_symbol::symbol::tuple_specialization_symbol(elements);
            instantiation.owner_arguments = Vec::new();
        }
        Ok(self.canonicalize_public_tuple_types(result))
    }

    /// A generic method's resolved compile-time arguments, and a concrete
    /// generic-struct receiver's instantiation, feed specialization
    /// discovery; once the elaborator has minted the clone, the call retargets
    /// to it by exact name and this returns its type.
    pub(super) fn retarget_to_method_clone(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<Option<Ty>, TypeError> {
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
            parameterized_syntax,
            preserves_receiver_interiors,
        } = call;
        if let (Ty::Struct(sname, targs), Some(arguments)) = (obj_ty, &resolved.instantiation) {
            // On a closed instance of an ordinary generic struct the request
            // (and the clone) is keyed by the instance: its arguments bake
            // before the call's.
            let owner_arguments = self.instance_arguments(sname, targs).unwrap_or_default();
            // A call already retargeted to the per-instantiation clone
            // (`name$y3:Int`) requests the clone of the source method: the
            // elaborator selects by source name and mints from the template.
            let source_method = method.split('$').next().unwrap_or(method);
            let overload = resolved
                .lowered_name
                .as_deref()
                .and_then(mojito_symbol::symbol::overload_qualifier)
                .map(str::to_string);
            self.record_dtype_parameter_arguments(param_args, arguments);
            self.method_instantiations.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::MethodInstantiation {
                    owner: sname.clone(),
                    owner_arguments: owner_arguments.clone(),
                    method: source_method.to_string(),
                    parameter_names: resolved.parameter_names.clone(),
                    overload: overload.clone(),
                    arguments: arguments.clone(),
                    inferred_values: crate::checker::call_inference::unsupplied_value_parameters(
                        &resolved.param_decls,
                        param_args,
                    ),
                },
            );
            let clone = if owner_arguments.is_empty() {
                self.specialized_method_clone(sname, method, &resolved.param_decls, arguments)
            } else {
                self.instance_call_method_clone(
                    sname,
                    targs,
                    source_method,
                    &resolved.param_decls,
                    arguments,
                )
            }
            .filter(|clone| self.clone_serves_overload(sname, clone, overload.as_deref()));
            if let Some(clone) = clone {
                return self
                    .infer_method_call(
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
                    )
                    .map(Some);
            }
        }
        // A concrete generic-struct receiver records its instantiation for
        // per-instantiation method-clone discovery and, once the elaborator
        // has appended the clone to the template, retargets to it by exact
        // name (the clone keeps the method's own parameters, so explicit
        // arguments pass through).
        if let Ty::Struct(sname, targs) = obj_ty
            && !targs.is_empty()
        {
            self.record_struct_instantiation(sname, targs, span.source.as_deref());
            if let Some(clone) = self.instance_method_clone(sname, method, targs) {
                return self
                    .infer_method_call(
                        span,
                        object,
                        &clone,
                        MethodCallArguments {
                            param_args,
                            args,
                            kwargs,
                            parameterized_syntax,
                            preserves_receiver_interiors,
                        },
                    )
                    .map(Some);
            }
        }
        Ok(None)
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
            obj_ty,
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
                .map(|t| substitute_at(t, info, targs))
                .collect();
            let receiver_variadic = sig
                .variadic
                .as_ref()
                .map(|ty| substitute_at(ty, info, targs));
            let receiver_kw_variadic = sig
                .kw_variadic
                .as_ref()
                .map(|ty| substitute_at(ty, info, targs));
            let Ok((params, variadic, kw_variadic, method_subst, method_arguments)) = self
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
            let Ok(clone_origins) = self.bind_clone_receiver_origins(
                &format!("{sname}.{method}"),
                sig,
                Some(obj_ty),
                &params,
                args,
                kwargs,
            ) else {
                continue;
            };
            let params = clone_origins.substitute_all(&params);
            let variadic = variadic.map(|element| clone_origins.substitute(&element));
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
                    return_type: clone_origins.substitute(&self.close_pack_elements(
                        self.close_method_values(
                            substitute(
                                &super::super::generics::expand_solved_packs(
                                    &substitute_at(&sig.ret, info, targs),
                                    &sig.decls,
                                    &method_arguments,
                                ),
                                &method_subst,
                            ),
                            &sig.decls,
                            &method_arguments,
                        ),
                        &[
                            (&sig.decls, &method_arguments),
                            (&info.decls, &positional_pack_arguments(&info.decls, targs)),
                        ],
                    )),
                    result_adapter: None,
                    raises: sig.raises,
                    error: sig.error.as_ref().map(|error| {
                        Box::new(substitute(
                            &substitute_at(error, info, targs),
                            &method_subst,
                        ))
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
                    declared_return: Some(
                        sig.template_ret.clone().unwrap_or_else(|| sig.ret.clone()),
                    ),
                    declared_params: clone_origins.substitute_all(&sig.params),
                    nested_origins: sig.nested_origins,
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
        let mut effective_bounds = match obj_ty {
            Ty::Param { bounds, .. } => bounds.clone(),
            _ => Vec::new(),
        };
        // `Copyable.copy` is proven by any route that proves
        // copyability (a refining bound, a member bound, or a
        // `conforms_to(T, Copyable)` availability assumption), not
        // only by a literal `Copyable` bound.
        if method == "copy"
            && args.is_empty()
            && self.is_copyable(obj_ty)
            && !effective_bounds.iter().any(|bound| bound == "Copyable")
        {
            effective_bounds.push("Copyable".to_string());
        }
        // Likewise `Hashable.__hash__`: a `where conforms_to(Self.T,
        // Hashable)` assumption proves `element.__hash__(hasher)` on
        // an otherwise unbounded `T` (upstream's container bodies).
        if method == "__hash__"
            && args.len() == 1
            && self.is_hashable(obj_ty)
            && !effective_bounds.iter().any(|bound| bound == "Hashable")
        {
            effective_bounds.push("Hashable".to_string());
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
        let mut requirements = Vec::new();
        for sig in signatures {
            let receiver_params: Vec<_> = sig
                .params
                .iter()
                .map(|ty| substitute_self(ty, obj_ty))
                .collect();
            let receiver_variadic = sig
                .variadic
                .as_deref()
                .map(|ty| substitute_self(ty, obj_ty));
            let receiver_kw_variadic = sig
                .kw_variadic
                .as_deref()
                .map(|ty| substitute_self(ty, obj_ty));
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
                return_type: self.resolve_assoc_ty(&substitute(
                    &substitute_self(&sig.ret, obj_ty),
                    &method_subst,
                )),
                result_adapter: (method == "__next__" && sig.ref_return.is_none()).then_some(
                    mojito_checked::checked::CheckedResultAdapter::CopyIteratorReference,
                ),
                raises: sig.raises,
                error: sig.error.as_ref().map(|error| {
                    Box::new(self.resolve_assoc_ty(&substitute(
                        &substitute_self(error, obj_ty),
                        &method_subst,
                    )))
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
                // Abstract dispatch keeps `Ty::SelfType` in `sig`, which
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
                    param_args,
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
    /// `__hash__`, and the float and rounding intrinsics.
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
            if is_place_expr(object) {
                self.copy_place_value_uses
                    .borrow_mut()
                    .insert(object.source_span());
            }
            return Ok(Ok(Some(intrinsic_resolution(
                obj_ty.clone(),
                vec![],
                vec![],
            ))));
        }
        // Hashable scalar leaves contribute themselves to the
        // caller-provided hasher's `_update_with_simd`, instantiated at
        // their own vector type (`-0.0` folded first, as upstream's
        // `SIMD.__hash__`). The `mut` argument is a place so its updated
        // state is committed by ordinary call lowering.
        if method == "__hash__"
            && args.len() == 1
            && kwargs.is_empty()
            && param_args.is_empty()
            && builtin_hashable_ty(obj_ty)
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
            return Ok(Ok(Some(intrinsic_resolution(
                Ty::None,
                vec![Some(ArgConvention::Mut)],
                vec![hasher],
            ))));
        }
        // A `Float64`'s fused multiply-add (`k.__fma__(step, start)`, a
        // float range element) is an intrinsic over the scalar itself;
        // sized float scalars resolve through the SIMD method table.
        if kwargs.is_empty()
            && param_args.is_empty()
            && *obj_ty == Ty::Float64
            && method == "__fma__"
            && args.len() == 2
        {
            for found in self.builtin_args(method, args.len(), args)? {
                if found != *obj_ty {
                    return Err(TypeError::TypeMismatch {
                        expected: obj_ty.to_string(),
                        found: found.to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
            }
            return Ok(Ok(Some(intrinsic_resolution(
                obj_ty.clone(),
                vec![None; args.len()],
                vec![obj_ty.clone(); args.len()],
            ))));
        }
        // `x.__floor__()` / `x.__ceildiv__(y)` on a concrete type
        // conforming to the granting rounding trait is the same VM
        // intrinsic the abstract Floorable/Ceilable/Truncable/CeilDivable
        // dispatch uses; a monomorphized clone of the self-hosted `math`
        // generics resolves it directly (roadmap milestone 7).
        if math_dunder_bound(method, args.len())
            .iter()
            .any(|bound| self.conforms_to(obj_ty, bound))
        {
            let param_types = if args.is_empty() {
                vec![]
            } else {
                let tys = self.builtin_args(method, 1, args)?;
                if tys[0] != *obj_ty {
                    return Err(TypeError::TypeMismatch {
                        expected: obj_ty.to_string(),
                        found: tys[0].to_string(),
                        context: format!("argument to '{method}'"),
                    });
                }
                vec![obj_ty.clone()]
            };
            return Ok(Ok(Some(intrinsic_resolution(
                obj_ty.clone(),
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
    }
}
