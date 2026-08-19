//! CheckedProgram fact accessors used by MIR lowering: checked types, owners,
//! call contracts, adjustments, capture accesses, and borrow mutability.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::*;

impl Flatten<'_> {
    pub(super) fn facts(&self, expression: &Expr) -> Option<&ExprFacts> {
        let key = expression as *const Expr as usize;
        self.active_semantics
            .iter()
            .rev()
            .find_map(|index| index.get(&key))
    }

    pub(super) fn checked_ty(&self, expression: &Expr) -> Option<Ty> {
        self.facts(expression).and_then(|facts| facts.ty.clone())
    }

    pub(super) fn checked_place_ty(&self, expression: &Expr) -> Option<Ty> {
        self.facts(expression)
            .and_then(|facts| facts.place_ty.clone())
    }

    pub(super) fn checked_raises(&self, expression: &Expr) -> Option<Ty> {
        self.checked_call_contract(expression)
            .and_then(|contract| contract.raises)
            .or_else(|| {
                self.facts(expression)
                    .and_then(|facts| facts.raises.clone())
            })
    }

    pub(super) fn checked_owner(&self, expression: &Expr) -> Option<crate::origin::OwnerId> {
        self.facts(expression).and_then(|facts| facts.owner)
    }

    pub(super) fn comprehension_bindings(
        &self,
        expression: &Expr,
    ) -> Vec<crate::checked::CheckedComprehensionBinding> {
        self.facts(expression)
            .map(|facts| facts.comprehension_bindings.clone())
            .unwrap_or_default()
    }

    pub(super) fn expression_var(&mut self, name: &str, expression: &Expr) -> VarId {
        if let Some(owner) = self.checked_owner(expression) {
            return self.binding_var(owner, name);
        }
        self.var(name)
    }

    /// Lookup-only twin of [`Self::expression_var`] for fact probes: a name
    /// with no checked owner and no existing slot (a top-level `def`
    /// referenced as a value) must NOT allocate a phantom variable — a later
    /// occurrence of the same name would then lower as a read of the
    /// never-defined slot instead of materializing the function value.
    pub(super) fn existing_expression_var(&self, name: &str, expression: &Expr) -> Option<VarId> {
        if let Some(owner) = self.checked_owner(expression)
            && let Some(var) = self.owner_vars.get(&owner).copied()
        {
            return Some(var);
        }
        self.vars
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| index as VarId)
    }

    pub(super) fn nested_info(&self, expression: &Expr) -> Option<NestedInfo> {
        self.checked_owner(expression)
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
    }
    /// Every owner loan carried into an aggregate expression.  An aggregate may
    /// contain more than one reference-valued field, so this must remain plural:
    /// keeping only the first borrow makes later fields dangling-capable.
    pub(super) fn aggregate_borrows(&mut self, expression: &Expr) -> Vec<MirLoan> {
        // A view-constructor implicit conversion borrows its source place:
        // the conversion result carries the same whole-place loan the
        // explicit construction's `BorrowRefArguments` records.
        if let Some(crate::SemanticAdjustment::BorrowConversionSource { mutable }) = self
            .checked_adjustments(expression)
            .into_iter()
            .find(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::BorrowConversionSource { .. }
                )
            })
            && matches!(
                expression.kind,
                ExprKind::Identifier(_) | ExprKind::Member { .. }
            )
        {
            let place = self.place(expression);
            return vec![MirLoan {
                place,
                mutable,
                interior: None,
            }];
        }
        let borrow = self
            .checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::BorrowShared => Some(false),
                crate::SemanticAdjustment::BorrowMutable => Some(true),
                _ => None,
            });
        if let Some(mutable) = borrow
            && let ExprKind::Identifier(name) = &expression.kind
            && let Some(var) = self.existing_expression_var(name, expression)
        {
            if let Some(loans) = self.aggregate_loans.get(&var) {
                return loans
                    .iter()
                    .cloned()
                    .map(|mut loan| {
                        loan.mutable = mutable;
                        loan
                    })
                    .collect();
            }
            return self
                .aliases
                .get(&var)
                .cloned()
                .map(|mut loan| {
                    loan.mutable = mutable;
                    vec![loan]
                })
                .unwrap_or_default();
        }
        if let Some(mutable) = borrow
            && matches!(
                expression.kind,
                ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::TypeApply { .. }
            )
        {
            let place = self.place(expression);
            let interiors = self.checked_interior_references(expression);
            if interiors.is_empty() {
                return vec![MirLoan {
                    place,
                    mutable,
                    interior: None,
                }];
            }
            return interiors
                .into_iter()
                .map(|origin| {
                    let mut place = place.clone();
                    let interior = self.direct_borrow_interior(&mut place, &origin);
                    MirLoan {
                        place,
                        mutable,
                        interior: Some(interior),
                    }
                })
                .collect();
        }
        if let ExprKind::Identifier(name) = &expression.kind {
            if let Some(var) = self.existing_expression_var(name, expression)
                && let Some(loans) = self.aggregate_loans.get(&var)
            {
                return loans.clone();
            }
            // A capturing closure flowing into storage loans its REFERENCE
            // captures' owners: the stored value retains their frame slots,
            // so the owners must stay alive (and, for `imm`, unmutated)
            // while the storage lives. Direct nested calls never consult
            // this path, so the loan-free declaration-to-call capture model
            // is preserved; owned copy/move captures are self-contained.
            if let Some(info) = self.nested_info(expression) {
                let mut loans = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for capture in &info.captures {
                    self.collect_capture_loans(capture, &mut loans, &mut seen);
                }
                return loans;
            }
        }
        match &expression.kind {
            ExprKind::Call { args, kwargs, .. } => {
                // A view construction borrows the places its `ref [origin]`
                // parameters bound (the checker recorded the argument
                // indexes), so the stored aggregate keeps its source alive.
                if let Some(crate::SemanticAdjustment::BorrowRefArguments { arguments }) = self
                    .checked_adjustments(expression)
                    .into_iter()
                    .find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::BorrowRefArguments { .. }
                        )
                    })
                {
                    let loans = arguments
                        .into_iter()
                        .filter_map(|(index, mutable)| {
                            args.get(index).map(|argument| MirLoan {
                                place: self.place(argument),
                                mutable,
                                interior: None,
                            })
                        })
                        .collect::<Vec<_>>();
                    if !loans.is_empty() {
                        return loans;
                    }
                }
                // A checked pointer construction loans exactly its source
                // place, with the mutability the checker inferred from the
                // owner binding. A construction through a `ref` binding
                // carries a subtree origin instead: its loan is the lazy
                // generation domain rooted at the reference's owner.
                if let Some(crate::SemanticAdjustment::PointerToPlace { mutable }) = self
                    .checked_adjustments(expression)
                    .into_iter()
                    .find(|adjustment| {
                        matches!(adjustment, crate::SemanticAdjustment::PointerToPlace { .. })
                    })
                {
                    if let Some(Ty::Pointer {
                        origin: crate::origin::PointerOrigin::Place { place, .. },
                        ..
                    }) = self.checked_ty(expression)
                        && matches!(place.path.last(), Some(crate::origin::OriginSeg::Subtree))
                    {
                        return self.pointer_place_loan(place.clone(), mutable);
                    }
                    let place = self.place(
                        &kwargs
                            .first()
                            .expect("checked pointer construction has a 'to=' argument")
                            .value,
                    );
                    return vec![MirLoan {
                        place,
                        mutable,
                        interior: None,
                    }];
                }
                args.iter()
                    .chain(kwargs.iter().map(|argument| &argument.value))
                    .flat_map(|argument| self.aggregate_borrows(argument))
                    .collect()
            }
            ExprKind::Transfer(inner) => self.aggregate_borrows(inner),
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => values
                .iter()
                .flat_map(|value| self.aggregate_borrows(value))
                .collect(),
            // A view-typed slice result (a Span sub-slice or a StringSpan
            // keyword slice) inherits its receiver's loans.
            ExprKind::Slice { object, .. } | ExprKind::MultiIndex { object, .. } => {
                if self
                    .checked_adjustments(expression)
                    .iter()
                    .any(|adjustment| {
                        matches!(adjustment, crate::SemanticAdjustment::BorrowViewResult)
                    })
                {
                    let loans = self.aggregate_borrows(object);
                    if !loans.is_empty() {
                        return loans;
                    }
                    // A receiver that is itself the owning place lends that
                    // place to the view.
                    if matches!(
                        object.kind,
                        ExprKind::Identifier(_) | ExprKind::Member { .. }
                    ) {
                        return vec![MirLoan {
                            place: self.place(object),
                            mutable: false,
                            interior: None,
                        }];
                    }
                }
                Vec::new()
            }
            // An `unsafe_origin_cast` result loans exactly its rebound target
            // place; an interior-generation tail becomes the loan's interior
            // domain so container mutation stales it without ordinary reads
            // conflicting.
            ExprKind::Invoke { .. } | ExprKind::MethodCall { .. } => {
                let cast =
                    self.checked_adjustments(expression)
                        .into_iter()
                        .find_map(|adjustment| match adjustment {
                            crate::SemanticAdjustment::PointerOriginCast { origin } => Some(origin),
                            _ => None,
                        });
                if let Some(crate::origin::PointerOrigin::Place { place, mutable }) = cast {
                    return self.pointer_place_loan(place, mutable);
                }
                // A method whose selected contract returns an origin-bearing
                // pointer (`xs.unsafe_ptr()`) loans that rebased place.
                if let Some(contract) = self.checked_call_contract(expression)
                    && let Ty::Pointer {
                        origin: crate::origin::PointerOrigin::Place { place, mutable },
                        ..
                    } = &contract.result_ty
                {
                    return self.pointer_place_loan(place.clone(), *mutable);
                }
                // `unsafe_offset` preserves provenance: forward the receiver's
                // loans onto the offset pointer.
                if let (true, ExprKind::MethodCall { object, .. }) = (
                    self.checked_adjustments(expression)
                        .iter()
                        .any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::PointerOffset)
                        }),
                    &expression.kind,
                ) {
                    return self.aggregate_borrows(object);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The loan an origin-bearing pointer's concrete place induces: rooted at
    /// the owner's variable, with an interior-generation tail becoming the
    /// loan's interior domain (container mutation stales it without ordinary
    /// reads conflicting). An unmapped owner (no live variable) loans nothing.
    fn pointer_place_loan(
        &mut self,
        place: crate::origin::OriginPlace,
        mutable: bool,
    ) -> Vec<MirLoan> {
        let Some(root) = self.owner_vars.get(&place.root).copied() else {
            return Vec::new();
        };
        let mir_place = MirPlace::root(root, self.var_types.get(&root).cloned());
        let interior = matches!(
            place.path.last(),
            Some(crate::origin::OriginSeg::Interior(_)) | Some(crate::origin::OriginSeg::Subtree)
        )
        .then(|| self.mir_interior_origin(&place, Some(root)))
        .flatten();
        vec![MirLoan {
            place: mir_place,
            mutable,
            interior,
        }]
    }

    /// One loan per reference capture (transitively through captured closure
    /// slots), rooted at the captured owner's variable. `imm` loans
    /// immutably; `mut`/`ref` loan mutably; copy/move environments are
    /// self-contained and stop the walk.
    fn collect_capture_loans(
        &self,
        capture: &crate::mir::NestedCapture,
        loans: &mut Vec<MirLoan>,
        seen: &mut std::collections::HashSet<crate::origin::OwnerId>,
    ) {
        use crate::ast::CaptureKind;
        if matches!(capture.kind, CaptureKind::Copy | CaptureKind::Move)
            || !seen.insert(capture.binding)
        {
            return;
        }
        if let Some(var) = self.owner_vars.get(&capture.binding).copied()
            && !loans.iter().any(|loan| loan.place.root == var)
        {
            loans.push(MirLoan {
                place: MirPlace::root(var, self.var_types.get(&var).cloned()),
                mutable: matches!(capture.kind, CaptureKind::Mut | CaptureKind::Ref),
                interior: None,
            });
        }
        if let Some(callable) = self.nested.get(&capture.binding).cloned() {
            for nested in &callable.captures {
                self.collect_capture_loans(nested, loans, seen);
            }
        }
    }

    pub(super) fn checked_adjustments(&self, expression: &Expr) -> Vec<crate::SemanticAdjustment> {
        self.facts(expression)
            .map(|facts| facts.adjustments.clone())
            .unwrap_or_default()
    }

    pub(super) fn tuple_unpack_plan(
        &self,
        expression: &Expr,
    ) -> Option<Vec<crate::checked::CheckedTupleUnpackElement>> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::TupleUnpack { elements } => Some(elements),
                _ => None,
            })
    }

    pub(super) fn instantiated_callable_contract(
        &self,
        expression: &Expr,
    ) -> Option<(Ty, Vec<TyArg>)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                } => Some((contract, arguments)),
                _ => None,
            })
    }

    pub(super) fn checked_call_contract(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedCallContract> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SelectedCall(contract) => Some(*contract),
                _ => None,
            })
    }

    /// The in-place dunder contract for `place OP= rhs` on a user-defined value
    /// (`__iadd__`, …), or `None` for native scalar targets that keep the builtin
    /// `BinOp` read-modify-write. The contract is self-contained (target, raises,
    /// arguments, boundary, param decls); the place node stays an ordinary place so
    /// `lower_call_receiver` commits the `mut self` mutation through its slot.
    pub(super) fn augmented_in_place_contract(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedCallContract> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::AugmentedInPlace(contract) => Some(*contract),
                _ => None,
            })
    }

    pub(super) fn checked_augmented_subscript(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedAugmentedSubscript> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::AugmentedSubscript(contract) => Some(*contract),
                _ => None,
            })
    }

    pub(super) fn element_invocation(
        &self,
        expression: &Expr,
    ) -> Option<crate::checked::CheckedElementInvocation> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ElementInvocation(plan) => Some(*plan),
                _ => None,
            })
    }

    pub(super) fn subscript_call_contract(
        &self,
        expression: &Expr,
        evaluated: &[(SourceSpan, Reg)],
    ) -> Option<MirSubscriptCall> {
        let contract = self.checked_call_contract(expression)?;
        Some(self.mir_subscript_call_contract(contract, evaluated))
    }

    pub(super) fn mir_subscript_call_contract(
        &self,
        contract: crate::checked::CheckedCallContract,
        evaluated: &[(SourceSpan, Reg)],
    ) -> MirSubscriptCall {
        let capture_accesses = contract
            .captures
            .iter()
            .filter_map(|capture| {
                let crate::origin::Origin::Place(place) = &capture.origin else {
                    return None;
                };
                self.owner_vars
                    .get(&place.root)
                    .copied()
                    .map(|root| MirCaptureAccess {
                        root,
                        path: place.path.clone(),
                        access: capture.access,
                    })
            })
            .collect();
        let param_arg_regs = contract
            .parameter_arguments
            .iter()
            .map(|argument| MirParamArg {
                name: argument.name.clone(),
                value: argument.value_source.as_ref().and_then(|source| {
                    evaluated.iter().find_map(|(candidate, register)| {
                        (candidate == source).then_some(*register)
                    })
                }),
            })
            .collect();
        MirSubscriptCall {
            target: contract.target,
            raises: contract.raises,
            result_ty: contract.result_ty,
            receiver_requires_place: contract.receiver_requires_place,
            receiver_convention: contract.receiver_convention,
            arguments: contract.arguments,
            capture_accesses,
            reference_result: contract.reference_result,
            param_arg_regs,
            param_decls: contract.param_decls,
        }
    }

    pub(super) fn checked_call_capture_accesses(&self, expression: &Expr) -> Vec<MirCaptureAccess> {
        let captures = self
            .checked_call_contract(expression)
            .map(|contract| contract.captures)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::CallableCaptureAccesses(captures) => {
                            Some(captures)
                        }
                        _ => None,
                    })
            });
        captures
            .map(|captures| {
                captures
                    .into_iter()
                    .filter_map(|capture| {
                        let crate::origin::Origin::Place(place) = capture.origin else {
                            return None;
                        };
                        self.owner_vars
                            .get(&place.root)
                            .copied()
                            .map(|root| MirCaptureAccess {
                                root,
                                path: place.path,
                                access: capture.access,
                            })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn checked_borrow_mutability(&self, expression: &Expr) -> Option<bool> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::BorrowShared => Some(false),
                crate::SemanticAdjustment::BorrowMutable => Some(true),
                _ => None,
            })
    }
}
