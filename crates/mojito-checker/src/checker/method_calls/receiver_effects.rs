//! The effects and ownership rules of a resolved method call: replayed
//! callee effects, receiver conventions, argument checks, and the reference
//! result.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl Checker {
    /// Replay the callee's loan-transfer effects against the actuals, and
    /// resolve any higher-order call-through residues against the concrete
    /// callables this call supplies.
    pub(super) fn replay_callee_effects(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
        selected_target: Option<&str>,
    ) -> Result<(), TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args, args, ..
        } = call;
        // The method's own binders, at the types this call solved for them.
        let bindings = resolved
            .instantiation
            .as_deref()
            .map(|arguments| {
                struct_subst(&ParamDecl::argument_view(&resolved.param_decls), arguments)
            })
            .unwrap_or_default();
        if let Some((struct_name, _)) = obj_ty.kind().as_non_pointer_struct() {
            let method_key = format!("{struct_name}.{method}");
            let effect_key = selected_target.unwrap_or(&method_key);
            self.apply_transfer_effects(effect_key, Some(object), args, span, &bindings)?;
            self.apply_call_through_effects(
                effect_key,
                &ParamDecl::argument_view(&resolved.param_decls),
                Some(object),
                param_args,
                args,
                span,
            )?;
        } else if matches!(obj_ty.kind(), TyKind::Param { .. }) || obj_ty.kind().is_conditional() {
            let bounds = self.refined_bound(obj_ty).unwrap_or_default();
            // Abstract trait dispatch has no concrete body: replay the union
            // of effects over every conforming implementation of the method
            // — the whole-program dispatch set. The method-name pre-filter is
            // syntactic (round-stable), and one observation per conformer
            // key keeps the two-phase pass exact even for conformers whose
            // effects commit in a later round.
            for key in self.dispatch_conformers(&bounds, method) {
                self.apply_transfer_effects(&key, Some(object), args, span, &bindings)?;
                self.apply_call_through_effects(
                    &key,
                    &ParamDecl::argument_view(&resolved.param_decls),
                    Some(object),
                    param_args,
                    args,
                    span,
                )?;
            }
        }
        Ok(())
    }

    /// A `mut self` method mutates its receiver, so the receiver must be a
    /// writable place (the mutation is written back to it): a variable, a
    /// field/index chain, or `self` in a `mut self` method.
    pub(super) fn check_mutated_receiver(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<(), TypeError> {
        let MethodCallSite {
            span, object, call, ..
        } = site;
        let MethodCallArguments {
            preserves_receiver_interiors,
            ..
        } = call;
        if resolved.mutates_receiver {
            let returned_reference = self
                .emitter
                .borrow()
                .get(&object.source_span())
                .and_then(|facts| facts.adjustment.as_deref())
                .and_then(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        Some(reference.clone())
                    }
                    _ => None,
                });
            if let Some(reference) = returned_reference {
                if !Self::require_writable_reference(reference.mutability) {
                    return Err(TypeError::ImmutableBinding(
                        "reference-returning method receiver".to_string(),
                    ));
                }
                // The method writes through a handle whose mutability is the
                // instance's: it lends mutably, as a store through it does.
                if matches!(
                    reference.mutability,
                    mojito_types::origin::Mutability::Param(_)
                ) {
                    self.record_at(object.source_span()).reference_value_use = Some(true);
                }
            } else {
                self.check_place(object)?;
            }
            if !preserves_receiver_interiors {
                self.record_interior_invalidation(span.clone(), object);
            }
        }
        Ok(())
    }

    /// Whether a write through a reference of `mutability` may stand: only
    /// a provably mutable one. A parametric one (`Origin[mut=m]`), whether
    /// over the enclosing struct's binder or the function's own, is judged
    /// in the generic body as upstream judges it, where `m` is not provably
    /// `True` however a caller binds it, and rejects.
    pub(in crate::checker) fn require_writable_reference(
        mutability: mojito_types::origin::Mutability,
    ) -> bool {
        mutability == mojito_types::origin::Mutability::Mutable
    }

    /// Record that the call at `span` returns `reference` by its `ref`
    /// result convention, as upstream's `lit.call` hands back the
    /// `!lit.ref` the expression reads through, and give the referent as
    /// the expression's type. A call carrying another checked adjustment
    /// has no slot for its result convention and is rejected.
    pub(in crate::checker) fn record_reference_result(
        &self,
        span: &SourceSpan,
        reference: mojito_types::origin::RefTy,
    ) -> Result<Ty, TypeError> {
        let occupied = self
            .fact(span, |facts| facts.adjustment.as_deref())
            .is_some_and(|adjustment| {
                !matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                )
            });
        if occupied {
            return Err(TypeError::Unsupported(
                "a reference result of a call carrying another checked adjustment".to_string(),
            ));
        }
        let referent = reference.referent.clone();
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference },
        ));
        Ok(referent)
    }

    /// A `ref self` receiver classifies as an immutable access for
    /// ownership unless the call needs the receiver's mutable capability
    /// for a reference result (whose loan carries the receiver's
    /// mutability), and then only when the receiver is provably mutable. A `ref self` method returning a
    /// value (`s[byte=a:b]`, `xs.__iter__()`) names `origin_of(self)` for
    /// the result's shared loan and cannot store into its receiver, so it
    /// reads: two live byte views of one String coexist. Receiver-aliasing
    /// exclusivity below still uses the raw declared convention.
    pub(super) fn effective_receiver_convention(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<Option<ArgConvention>, TypeError> {
        let MethodCallSite { object, .. } = site;
        Ok(
            if resolved.self_convention == Some(ArgConvention::Ref)
                && (resolved.ref_return.is_none()
                    || self.materialized_reference_actual(object)?.mutability
                        != mojito_types::origin::Mutability::Mutable)
            {
                Some(ArgConvention::Imm)
            } else {
                resolved.self_convention
            },
        )
    }

    /// The receiver a call lends, copies, or consumes: a `^` receiver of a
    /// read method, and the copy, move, and explicit-destructor rules of a
    /// consuming one.
    pub(super) fn check_consumed_receiver(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<(), TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            obj_ty,
            ..
        } = site;
        // A `^` receiver of a read method lends its place instead of moving
        // it, as a `^` into a read parameter does.
        if !resolved.consumes_receiver
            && matches!(resolved.self_convention, None | Some(ArgConvention::Imm))
            && let ExprKind::Transfer(inner) = &object.kind
        {
            for span in [object.source_span(), inner.source_span()] {
                self.record_at(span).borrowed_read_call_place = true;
            }
        }
        // A temporary receiver the method only borrows is destroyed once the
        // call returns.
        if !resolved.consumes_receiver {
            self.record_unconsumed_temporary(object);
        }
        // A `deinit self` call always consumes its receiver. Mojo may satisfy
        // that consumption by implicitly copying an `ImplicitlyCopyable` place;
        // a merely movable (or explicitly-copy-only) place still requires `^`.
        if resolved.consumes_receiver
            && (self.designates_place(object) || self.infer_reference_value(object).is_some())
        {
            if !self.is_implicitly_copyable(obj_ty) {
                let context = format!("consuming receiver of method '{method}'");
                if !self.is_copyable(obj_ty) {
                    return Err(TypeError::NonCopyable {
                        ty: obj_ty.to_string(),
                        context,
                    });
                }
                let transferable = self.is_movable(obj_ty)
                    && crate::checker::places::place_path(object)
                        .is_some_and(|(root, _)| self.is_binding_mutable(root));
                return Err(TypeError::ImplicitCopy {
                    ty: obj_ty.to_string(),
                    context,
                    transferable,
                    copyable: true,
                });
            }
            self.record_at(span.clone())
                .implicitly_copied_consuming_receiver = true;
        }
        // A `var self` receiver takes ownership by move, so a declared
        // `Movable where False` opt-out rejects it; `deinit self` is
        // consumption-for-destruction and stays legal for non-Movable values.
        // A type parameter's value is handed over where it lies, so its
        // bounds need not prove `Movable`, as at the pin.
        if resolved.consumes_receiver
            && resolved.self_convention == Some(mojito_ast::ast::ArgConvention::Var)
            && !matches!(obj_ty.kind(), TyKind::Param { .. })
            && !self.is_movable(obj_ty)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: format!("receiver of method '{method}'"),
                ty: obj_ty.to_string(),
                trait_name: "Movable".to_string(),
                reason: self
                    .trait_failure_reason(obj_ty, "Movable")
                    .or_else(|| Some("its 'Movable' conformance condition is false".to_string())),
            });
        }
        if resolved.consumes_receiver
            && let Some((name, _)) = obj_ty.kind().as_non_pointer_struct()
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.explicit_destructors.contains_key(method))
        {
            self.record_at(span.clone()).explicit_destroy_call = true;
        }
        Ok(())
    }

    /// Check the selected signature's arguments: consumption, origin
    /// solving, aliasing against each other and the receiver, and origin
    /// exclusivity. Returns the effective conventions and the solved
    /// reference result.
    pub(super) fn check_method_arguments(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<
        (
            Vec<Option<ArgConvention>>,
            Option<mojito_types::origin::RefTy>,
        ),
        TypeError,
    > {
        let MethodCallSite {
            object,
            method,
            call,
            obj_ty,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
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
            if let Some(ArgConvention::Var | ArgConvention::Deinit) =
                resolved.conventions.get(index).copied().flatten()
            {
                self.check_consuming_as(
                    expression,
                    &ty,
                    &format!("argument {} to method '{}'", index + 1, method),
                    crate::checker::traits::ConsumeKind::Forward,
                )?;
            }
        }
        self.check_consuming_collected(
            resolved.variadic_convention == Some(ArgConvention::Var),
            &resolved.positional_overflow,
            args,
            &format!("method '{method}'"),
        )?;
        if let Some(self_ref) = &resolved.self_ref {
            self.check_receiver_ref(object, self_ref).map_err(|error| {
                super::super::call_inference::name_ref_origin_mismatch(
                    error,
                    method,
                    &["self".to_string()],
                )
            })?;
        }
        let (effective_conventions, solved_return) = self
            .solve_call_origins(
                &resolved.slots,
                &resolved.conventions,
                &resolved.ref_params,
                resolved.ref_return.as_ref(),
                args,
                kwargs,
            )
            .map_err(|error| {
                super::super::call_inference::name_ref_origin_mismatch(
                    error,
                    method,
                    &resolved.parameter_names,
                )
            })?;
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
                let ty = self.infer_with_expected(
                    expression,
                    resolved
                        .param_types
                        .get(index)
                        .expect("selected method slot has a parameter type"),
                    true,
                )?;
                Ok(self.argument_is_independent_copy(convention, expression, &ty))
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        crate::checker::places::reject_transfer_into_mutable(
            method,
            &resolved.slots,
            &effective_conventions,
            args,
            kwargs,
        )?;
        self.bind_mutable_subscript_arguments(
            method,
            &resolved.slots,
            &effective_conventions,
            args,
            kwargs,
        )?;
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
        // The receiver's origin tail binds the struct's own binders in the
        // declared signature (`Self`, `RefBox[Self.o]`), whose slots fix
        // their own mutability; type parameters stay abstract there.
        let (tail_bindings, self_declared) = match obj_ty.kind() {
            struct_ty if let Some((sname, targs)) = struct_ty.as_non_pointer_struct() => {
                match self.structs.get(sname) {
                    Some(info) => (
                        {
                            let mut bindings = info.instance_substitution(targs);
                            bindings.infer_mutabilities(&|_, _| None);
                            bindings
                        },
                        TyKind::Struct(sname.clone(), info.self_arguments()).into(),
                    ),
                    None => (OriginBindings::default(), obj_ty.clone()),
                }
            }
            _ => (OriginBindings::default(), obj_ty.clone()),
        };
        let self_declared = substitute_struct_origins(&self_declared, &tail_bindings);
        let declared: Vec<Ty> = resolved
            .declared_params
            .iter()
            .map(|parameter| substitute_struct_origins(parameter, &tail_bindings))
            .collect();
        let collected = self.method_collected_arguments(resolved, args);
        self.check_argument_origin_exclusivity(
            &ExclusivityCallee {
                name: method,
                parameter_names: &resolved.parameter_names,
                declared: &declared,
                bound: &resolved.param_types,
                nested_origins: resolved.nested_origins,
                collected: Some(&collected),
                initializer: false,
            },
            Some(&ExclusivityReceiver {
                object,
                convention: resolved.self_convention,
                declared: &self_declared,
                bound: obj_ty,
            }),
            &effective_conventions,
            &resolved.slots,
            args,
            kwargs,
        )?;
        if resolved.self_convention == Some(ArgConvention::Mut) {
            self.check_mutable_receiver_carried_aliases(
                object,
                method,
                &resolved.parameter_names,
                &resolved.slots,
                args,
                kwargs,
            )?;
        }
        self.record_argument_borrows(
            &resolved.slots,
            &effective_conventions,
            resolved.gathered_arguments(),
            args,
            kwargs,
            Some((object, resolved.self_convention)),
        );
        Ok((effective_conventions, solved_return))
    }

    /// The reference a `ref`-returning method hands back, with its origin
    /// and mutability resolved against the receiver and the actuals.
    pub(super) fn method_reference_result(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
        solved_return: Option<mojito_types::origin::RefTy>,
    ) -> Result<Option<mojito_types::origin::RefTy>, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            ..
        } = site;
        let MethodCallArguments { args, kwargs, .. } = call;
        let Some(signature) = &resolved.ref_return else {
            return Ok(None);
        };
        let actual = resolved
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let expression = match slot {
                    ArgSlot::Positional(position) => &args[*position],
                    ArgSlot::Keyword(position) => &kwargs[*position].value,
                    ArgSlot::Default => return Ok(None),
                };
                match self.reference_actual(expression) {
                    Ok(reference) => Ok(Some(reference.origin)),
                    // A temporary at a read slot the result borrows lives as
                    // long as the reference, as at a free call (see
                    // `solve_call_origins_with_bool_bindings`).
                    Err(TypeError::Unsupported(message))
                        if message.contains("reference binding to a non-place expression")
                            && reads_slot(resolved, index)
                            && sig_origin_mentions_param(&signature.origin, index) =>
                    {
                        Ok(Some(mojito_types::origin::Origin::Place(
                            self.materialize_borrow_place(expression, false)?,
                        )))
                    }
                    Err(_) => Ok(None),
                }
            })
            .collect::<Result<Vec<_>, TypeError>>()?;
        // A temporary receiver of a `ref[self]`-returning method
        // (`FormatStruct(writer, "P").params(...)`) materializes as an
        // anonymous owned binding, as a temporary bound to a `ref`
        // parameter does: the result borrows that frame-local slot and
        // the temporary lives as long as the statement that chains on it.
        let self_reference = self.materialized_reference_actual(object)?;
        let declared =
            self.bind_sig_pack_origin(&signature.origin, &resolved.positional_overflow, args)?;
        let origin =
            substitute_sig_origin_with_self(&declared, &actual, Some(self_reference.origin));
        // A struct origin parameter in the return (`ref[o]`) resolves to the
        // origin the receiver's `ref[o]` field borrows, so the returned
        // reference records a loan on its ultimate source rather than an
        // abstract parameter the loan machinery would drop.
        let origin = self.resolve_receiver_origin_arguments(origin, object);
        let mutable = match signature.mutability {
            mojito_types::origin::SigMutability::Immutable => {
                mojito_types::origin::Mutability::Immutable
            }
            mojito_types::origin::SigMutability::Mutable => {
                mojito_types::origin::Mutability::Mutable
            }
            // An origin whose `mut=` is decided read-only (`ImmOrigin(o)`, the
            // static origin) makes the reference read-only, as upstream's
            // `ref[o]` takes `o`'s mutability, whatever the receiver is.
            _ if decided_read_only(&origin) => mojito_types::origin::Mutability::Immutable,
            // `ref[o]` over `o: Origin[mut=m]` has `m`'s mutability: the
            // receiver instance's `mut` argument, else what the resolved
            // origin decides (an enclosing binder's `mut`, a place's owner).
            mojito_types::origin::SigMutability::BoolParam(slot)
                if let Some(decided) = self
                    .receiver_bool_argument(object, slot)
                    .map(mojito_types::origin::Mutability::of)
                    .or_else(|| self.decided_origin_mutability(&origin)) =>
            {
                decided
            }
            _ if (self_reference.mutability == mojito_types::origin::Mutability::Mutable
                && origin_may_name_receiver(&signature.origin, resolved))
                || solved_return.is_some_and(|reference| {
                    reference.mutability == mojito_types::origin::Mutability::Mutable
                }) =>
            {
                mojito_types::origin::Mutability::Mutable
            }
            // A parametric-mut receiver's result stays symbolic: a store
            // through it rejects in the generic body
            // (`require_writable_reference`), and each instantiation
            // substitutes the binder's decided mutability.
            _ if matches!(
                self_reference.mutability,
                mojito_types::origin::Mutability::Param(_)
            ) =>
            {
                self_reference.mutability
            }
            _ => mojito_types::origin::Mutability::Immutable,
        };
        let reference = mojito_types::origin::RefTy {
            referent: resolved.return_type.clone(),
            origin,
            mutability: mutable,
            address_space: signature.address_space.clone(),
        };
        self.record_reference_result(span, reference.clone())?;
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
        Ok(Some(reference))
    }

    /// The receiver `object`'s instance argument for the struct's `Bool`
    /// binder at `slot`, counted as [`super::origins::origin_decl_mutability`]
    /// counts it (a pack's implicit list binder takes no slot), when the
    /// instance decides it.
    fn receiver_bool_argument(&self, object: &Expr, slot: usize) -> Option<bool> {
        let receiver = self.infer(object).ok()?;
        let (name, arguments) = self.nominal_struct(&receiver)?;
        let position = self
            .structs
            .get(name)?
            .params
            .iter()
            .enumerate()
            .filter(|(_, decl)| !decl.is_pack_list())
            .nth(slot)?
            .0;
        match arguments.get(position) {
            Some(TyArg::Val(mojito_types::ct::CtValue::Bool(value))) => Some(*value),
            _ => None,
        }
    }
}

/// Whether `origin`, through its projections, is decided read-only
/// ([`mojito_types::origin::Origin::static_mutability`]).
fn decided_read_only(origin: &mojito_types::origin::Origin) -> bool {
    match origin {
        mojito_types::origin::Origin::Projected(base, _) => decided_read_only(base),
        origin => origin.static_mutability() == Some(false),
    }
}

/// Whether argument slot `index` takes the read convention.
fn reads_slot(resolved: &MethodCallResolution, index: usize) -> bool {
    matches!(
        resolved.conventions.get(index),
        Some(None | Some(ArgConvention::Imm))
    )
}

/// Whether a returned reference's signature origin may borrow the receiver,
/// so the receiver's mutability can make the result mutable. A member naming
/// a read-convention argument slot, or a read `self`, is ruled out: that
/// borrow is immutable whatever the receiver is, as the pin rejects
/// `h.pick(w) = 5` and a store through `__getitem__(self) -> ref[self.a]`.
fn origin_may_name_receiver(
    origin: &mojito_types::origin::SigOrigin,
    resolved: &MethodCallResolution,
) -> bool {
    use mojito_types::origin::SigOrigin;
    match origin {
        SigOrigin::Self_ => !matches!(resolved.self_convention, None | Some(ArgConvention::Imm)),
        SigOrigin::Param(index) => !reads_slot(resolved, *index),
        SigOrigin::Projected(base, _) => origin_may_name_receiver(base, resolved),
        SigOrigin::Union(members) => members
            .iter()
            .any(|member| origin_may_name_receiver(member, resolved)),
        _ => true,
    }
}
