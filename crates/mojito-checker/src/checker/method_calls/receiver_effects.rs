//! The effects and ownership rules of a resolved method call: replayed
//! callee effects, receiver conventions, argument checks, and the reference
//! result.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
            .map(|arguments| struct_subst(&resolved.param_decls, arguments))
            .unwrap_or_default();
        if let Ty::Struct(struct_name, _) = obj_ty {
            let method_key = format!("{struct_name}.{method}");
            let effect_key = selected_target.unwrap_or(&method_key);
            self.apply_transfer_effects(effect_key, Some(object), args, span, &bindings)?;
            self.apply_call_through_effects(
                effect_key,
                &resolved.param_decls,
                Some(object),
                param_args,
                args,
                span,
            )?;
        } else if let Ty::Param { bounds, .. } = obj_ty {
            // Abstract trait dispatch has no concrete body: replay the union
            // of effects over every conforming implementation of the method
            // — the whole-program dispatch set. The method-name pre-filter is
            // syntactic (round-stable), and one observation per conformer
            // key keeps the two-phase pass exact even for conformers whose
            // effects commit in a later round.
            for key in self.dispatch_conformers(bounds, method) {
                self.apply_transfer_effects(&key, Some(object), args, span, &bindings)?;
                self.apply_call_through_effects(
                    &key,
                    &resolved.param_decls,
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
                .operation_adjustments
                .borrow()
                .get(&object.source_span())
                .and_then(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        Some(reference.clone())
                    }
                    _ => None,
                });
            if let Some(reference) = returned_reference {
                if reference.mutability != mojito_types::origin::Mutability::Mutable {
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
        Ok(())
    }

    /// A method body writing through a parametric-mut ref field is legal
    /// only for instantiations binding that origin parameter to a mutable
    /// source; judge each recorded write against the receiver's concrete
    /// origin arguments here, at the instantiation site.
    pub(super) fn judge_parametric_origin_writes(
        &self,
        site: MethodCallSite<'_>,
        resolved: &MethodCallResolution,
    ) -> Result<(), TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            ..
        } = site;
        for id in &resolved.parametric_origin_writes {
            let origin = self.resolve_receiver_origin_arguments(
                mojito_types::origin::Origin::Param(*id),
                object,
            );
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
                            .map(|(index, _)| mojito_types::origin::OriginParamId(index as u32))
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
        Ok(())
    }

    /// A `ref self` receiver classifies as an immutable access for
    /// ownership unless the call needs the receiver's mutable capability:
    /// a recorded parametric write in the body, or a reference result
    /// (whose loan carries the receiver's mutability) — and then only when
    /// the receiver is provably mutable. A `ref self` method returning a
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
                && ((resolved.parametric_origin_writes.is_empty() && resolved.ref_return.is_none())
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
            self.borrowed_read_call_places
                .borrow_mut()
                .extend([object.source_span(), inner.source_span()]);
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
            && (is_place_expr(object) || self.infer_reference_value(object).is_some())
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
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span.clone());
        }
        // A `var self` receiver takes ownership by move, so a declared
        // `Movable where False` opt-out rejects it; `deinit self` is
        // consumption-for-destruction and stays legal for non-Movable values.
        if resolved.consumes_receiver
            && resolved.self_convention == Some(mojito_ast::ast::ArgConvention::Var)
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
            && let Ty::Struct(name, _) = obj_ty
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.explicit_destructors.contains_key(method))
        {
            self.explicit_destroy_calls
                .borrow_mut()
                .insert(span.clone());
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
        self.check_consuming_collected(
            resolved.variadic_convention == Some(ArgConvention::Var),
            &resolved.positional_overflow,
            args,
            &format!("method '{method}'"),
        )?;
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
        let (tail_bindings, self_declared) = match obj_ty {
            Ty::Struct(sname, targs) => match self.structs.get(sname) {
                Some(info) => (
                    info.tail_origin_bindings(targs),
                    Ty::Struct(sname.clone(), info.self_arguments().into()),
                ),
                None => (HashMap::new(), obj_ty.clone()),
            },
            _ => (HashMap::new(), obj_ty.clone()),
        };
        let self_declared = substitute_struct_origin_tails(&self_declared, &tail_bindings);
        let declared: Vec<Ty> = resolved
            .declared_params
            .iter()
            .map(|parameter| substitute_struct_origin_tails(parameter, &tail_bindings))
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
            resolved.read_collected_arguments(),
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
        // A temporary receiver of a `ref[self]`-returning method
        // (`FormatStruct(writer, "P").params(...)`) materializes as an
        // anonymous owned binding, as a temporary bound to a `ref`
        // parameter does: the result borrows that frame-local slot and
        // the temporary lives as long as the statement that chains on it.
        let self_reference = self.materialized_reference_actual(object)?;
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
            mojito_types::origin::SigMutability::Immutable => {
                mojito_types::origin::Mutability::Immutable
            }
            mojito_types::origin::SigMutability::Mutable => {
                mojito_types::origin::Mutability::Mutable
            }
            _ if self_reference.mutability == mojito_types::origin::Mutability::Mutable
                || solved_return.is_some_and(|reference| {
                    reference.mutability == mojito_types::origin::Mutability::Mutable
                }) =>
            {
                mojito_types::origin::Mutability::Mutable
            }
            // A parametric-mut receiver stays symbolic: the write legality
            // is judged per instantiation at the enclosing call site, not
            // collapsed to immutable inside the generic body.
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
            referent: Box::new(resolved.return_type.clone()),
            origin,
            mutability: mutable,
        };
        self.operation_adjustments.borrow_mut().insert(
            span.clone(),
            mojito_checked::checked::SemanticAdjustment::ReferenceResult {
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
        Ok(Some(reference))
    }
}
