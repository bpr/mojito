//! The checked facts MIR lowering reads off a [`Node`]: its type, owner,
//! call contract, adjustments, and operand shapes as the node's own
//! attributes, and the loans, capture accesses, and slots that also need the
//! function being lowered. See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedOp;
use mojito_types::types::TyKind;

impl<'a> Node<'a> {
    /// Whether this is a name node.
    pub(super) const fn is_name(self) -> bool {
        matches!(self.op(), CheckedOp::Identifier(_))
    }

    /// Whether this is a call, invocation, or method call node.
    pub(super) const fn is_call(self) -> bool {
        matches!(
            self.op(),
            CheckedOp::Call { .. } | CheckedOp::Invoke { .. } | CheckedOp::MethodCall { .. }
        )
    }

    /// The object of a member read: `None` when this is not one.
    pub(super) fn member_object(self) -> Option<Self> {
        matches!(self.op(), CheckedOp::Member { .. })
            .then(|| self.object())
            .flatten()
    }

    /// The object and index of a subscript: `None` when this is not one.
    pub(super) fn index_operands(self) -> Option<(Self, Self)> {
        self.object().zip(self.index())
    }

    /// The operand of a transfer: `None` when this is not one.
    pub(super) fn transfer_operand(self) -> Option<Self> {
        self.sole_operand(|op| matches!(op, CheckedOp::Transfer(_)))
    }

    /// The operand of a spread: `None` when this is not one.
    pub(super) fn spread_operand(self) -> Option<Self> {
        self.sole_operand(|op| matches!(op, CheckedOp::Spread(_)))
    }

    /// The value of a walrus: `None` when this is not one.
    pub(super) fn named_value(self) -> Option<Self> {
        self.sole_operand(|op| matches!(op, CheckedOp::Named { .. }))
    }

    pub(super) fn ty(self) -> Option<Ty> {
        self.checked.ty.clone()
    }

    /// The type HIR's place for this node stores (`HirPlace::ty`).
    pub(super) fn place_ty(self) -> Option<Ty> {
        self.hir.place.as_ref().map(|place| place.ty.clone())
    }

    pub(super) fn raises(self) -> Option<Ty> {
        self.call_contract()
            .and_then(|contract| contract.raises)
            .or_else(|| self.checked.effects.raises.clone())
    }

    /// Its binding, else its place root's binding (`HirPlace::owner`).
    pub(super) fn owner(self) -> Option<mojito_types::origin::OwnerId> {
        self.checked
            .binding
            .or_else(|| self.hir.place.as_ref().map(|place| place.owner))
    }

    pub(super) fn adjustments(self) -> &'a [mojito_checked::checked::SemanticAdjustment] {
        &self.checked.adjustments
    }

    /// Whether a borrowing-view call lends this place argument to its result:
    /// an aggregate place the callee could have borrowed into the view, or a
    /// place bound to a `mut`/`ref` parameter (the checker retained its call
    /// place), whatever its type — a `ref n: Int` source of a `Pointer[Int,
    /// o]` field is lent exactly like a `ref xs: List[Int]` source.
    pub(super) fn lends_to_view(self) -> bool {
        matches!(
            self.op(),
            CheckedOp::Identifier(_) | CheckedOp::Member { .. }
        ) && (matches!(
            self.checked.ty.as_ref().map(Ty::kind),
            Some(TyKind::Struct(..))
        ) || self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RetainCallPlace
            )
        }))
    }

    /// The operand and target types of the `rebind` this node is the
    /// operand of, where the two differ.
    pub(super) fn rebind_target(self) -> Option<(Ty, Ty)> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::Rebind { operand, dest } => {
                    Some((operand.clone(), dest.clone()))
                }
                _ => None,
            })
    }

    /// Whether this is a checker-resolved `DType.<name>` value, which is a
    /// constant rather than a field of a `DType` binding.
    pub(super) fn is_dtype_constant(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::DtypeConstant { .. }
            )
        })
    }

    pub(super) fn tuple_unpack_plan(
        self,
    ) -> Option<Vec<mojito_checked::checked::CheckedTupleUnpackElement>> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::TupleUnpack { elements } => {
                    Some(elements.clone())
                }
                _ => None,
            })
    }

    pub(super) fn instantiated_callable_contract(self) -> Option<(Ty, Vec<TyArg>)> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::InstantiatedCallableContract {
                    contract,
                    arguments,
                } => Some((contract.clone(), arguments.clone())),
                _ => None,
            })
    }

    pub(super) fn call_contract(self) -> Option<mojito_checked::checked::CheckedCallContract> {
        selected_call(self.adjustments())
    }

    /// The in-place dunder contract for `place OP= rhs` on a user-defined value
    /// (`__iadd__`, …), or `None` for native scalar targets that keep the builtin
    /// `BinOp` read-modify-write. The contract is self-contained (target, raises,
    /// arguments, boundary, param decls); the place node stays an ordinary place so
    /// `lower_call_receiver` commits the `mut self` mutation through its slot.
    pub(super) fn augmented_in_place_contract(
        self,
    ) -> Option<mojito_checked::checked::CheckedCallContract> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::AugmentedInPlace {
                    call: contract,
                    ..
                } => Some((**contract).clone()),
                _ => None,
            })
    }

    pub(super) fn augmented_subscript(
        self,
    ) -> Option<mojito_checked::checked::CheckedAugmentedSubscript> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(contract) => {
                    Some((**contract).clone())
                }
                _ => None,
            })
    }

    pub(super) fn element_invocation(
        self,
    ) -> Option<mojito_checked::checked::CheckedElementInvocation> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ElementInvocation(plan) => {
                    Some((**plan).clone())
                }
                _ => None,
            })
    }

    pub(super) fn borrow_mutability(self) -> Option<bool> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::BorrowShared => Some(false),
                mojito_checked::checked::SemanticAdjustment::BorrowMutable => Some(true),
                _ => None,
            })
    }

    /// The compile-time arguments the checker solved for a call of a generic
    /// `def`, in declaration order; empty where the call recorded none.
    pub(super) fn instantiated_args(self) -> Vec<TyArg> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::InstantiatedArguments(arguments) => {
                    Some(arguments.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The places this node's reference value may point into: its interior
    /// references, its reference result's origin, and its reference type's.
    pub(super) fn reference_places(self) -> Vec<mojito_types::origin::OriginPlace> {
        fn collect(
            origin: &mojito_types::origin::Origin,
            places: &mut Vec<mojito_types::origin::OriginPlace>,
        ) {
            match origin {
                mojito_types::origin::Origin::Place(place) => places.push(place.clone()),
                mojito_types::origin::Origin::Union(members) => {
                    for member in members {
                        collect(member, places);
                    }
                }
                _ => {}
            }
        }

        let mut places = Vec::new();
        for adjustment in self.adjustments() {
            match adjustment {
                mojito_checked::checked::SemanticAdjustment::InteriorReference { origin } => {
                    places.push(origin.clone());
                }
                mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                    collect(&reference.origin, &mut places);
                }
                _ => {}
            }
        }
        if let Some(TyKind::Ref(reference)) = self.checked.ty.as_ref().map(Ty::kind) {
            collect(&reference.origin, &mut places);
        }
        places.sort();
        places.dedup();
        places
    }

    /// The [`Self::reference_places`] inside an owned interior.
    pub(super) fn interior_references(self) -> Vec<mojito_types::origin::OriginPlace> {
        self.reference_places()
            .into_iter()
            .filter(|place| {
                place.path.iter().any(|segment| {
                    matches!(
                        segment,
                        mojito_types::origin::OriginSeg::Interior(_)
                            | mojito_types::origin::OriginSeg::Subtree
                    )
                })
            })
            .collect()
    }

    /// The container a subscripted view receiver (`ys[0]`, `ps[0].name`,
    /// `self.rows[i][j]`) reads its element out of, with the interior origins
    /// the checker recorded for that element. `None` for a receiver that
    /// crosses no subscript, whose container is not a plain place, or whose
    /// subscript recorded no interior reference.
    pub(super) fn element_receiver(self) -> Option<(Self, Vec<mojito_types::origin::OriginPlace>)> {
        let mut element = None;
        let mut current = self;
        loop {
            if let Some((object, _)) = current.index_operands() {
                element = Some((object, current));
                current = object;
            } else if let Some(object) = current.member_object() {
                current = object;
            } else if current.is_name() {
                break;
            } else {
                return None;
            }
        }
        let (container, subscript) = element?;
        let interiors = subscript.interior_references();
        (!interiors.is_empty()).then_some((container, interiors))
    }

    /// The one operand of this node, when its op satisfies `is_op`.
    fn sole_operand(self, is_op: impl Fn(&CheckedOp) -> bool) -> Option<Self> {
        match self.hir.children.as_slice() {
            [_] if is_op(self.op()) => self.operand(0),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
impl<'a> Flatten<'a> {
    /// The runtime spelling of name node `node`: `None` when it is not a
    /// name.
    pub(super) fn name_spelled(&self, node: Node<'a>) -> Option<String> {
        match node.op() {
            CheckedOp::Identifier(name) => Some(self.spelling(node.checked, name)),
            _ => None,
        }
    }

    pub(super) fn expression_var(&mut self, name: &str, node: Node<'a>) -> VarId {
        if let Some(owner) = node.owner() {
            return self.binding_var(owner, name);
        }
        self.var(name)
    }

    /// Lookup-only twin of [`Self::expression_var`] for fact probes: a name
    /// with no checked owner and no existing slot (a top-level `def`
    /// referenced as a value) must NOT allocate a phantom variable — a later
    /// occurrence of the same name would then lower as a read of the
    /// never-defined slot instead of materializing the function value.
    pub(super) fn existing_expression_var(&self, name: &str, node: Node<'a>) -> Option<VarId> {
        if let Some(owner) = node.owner()
            && let Some(var) = self.owner_vars.get(&owner).copied()
        {
            return Some(var);
        }
        self.vars
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| index as VarId)
    }

    pub(super) fn nested_info(&self, node: Node<'a>) -> Option<NestedInfo> {
        node.owner()
            .and_then(|binding| self.nested.get(&binding))
            .cloned()
    }

    /// The slot call node `node`, written `name`, calls through: its
    /// binding's, when that binding is a local or parameter.
    pub(super) fn local_callee(&mut self, node: Node<'a>, name: &str) -> Option<VarId> {
        node.checked
            .binding
            .and_then(|owner| self.local_binding_var(owner, name))
    }

    /// Every owner loan carried into an aggregate expression.  An aggregate may
    /// contain more than one reference-valued field, so this must remain plural:
    /// keeping only the first borrow makes later fields dangling-capable.
    pub(super) fn aggregate_borrows(&mut self, node: Node<'a>) -> Vec<MirLoan> {
        // A view-constructor implicit conversion borrows its source place:
        // the conversion result carries the same whole-place loan the
        // explicit construction's `BorrowRefArguments` records.
        if let Some(mojito_checked::checked::SemanticAdjustment::BorrowConversionSource { mutable }) =
            node.adjustments().iter().find(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowConversionSource { .. }
                )
            })
            && matches!(
                node.op(),
                CheckedOp::Identifier(_) | CheckedOp::Member { .. }
            )
        {
            let place = self.place_of(node);
            return vec![MirLoan {
                place,
                mutable: *mutable,
                interior: None,
                shared: false,
            }];
        }
        if let Some(loans) = self.conversion_view_loans.get(&node.address()) {
            return loans.clone();
        }
        // A materialized borrow-source temporary loans its hidden owned slot:
        // the consuming aggregate's binding keeps the temporary alive exactly
        // as long as the borrower, upstream's temporary-lifetime rule. The
        // slot is lent with the capability the checker recorded, so the loan
        // permits what it would over a named local.
        let adjustments = node.adjustments();
        if let Some(owner) = mojito_checked::checked::materialized_borrow_owner(adjustments)
            && let Some(var) = self.owner_vars.get(&owner).copied()
        {
            return vec![MirLoan {
                place: MirPlace::root(var, self.var_types.get(&var).cloned()),
                mutable: mojito_checked::checked::materialized_borrow_mutability(adjustments),
                interior: None,
                shared: false,
            }];
        }
        self.aggregate_borrows_unmaterialized(node)
    }

    /// The loan an origin-bearing pointer's concrete place induces: rooted at
    /// the owner's variable, with an interior-generation tail becoming the
    /// loan's interior domain (container mutation stales it without ordinary
    /// reads conflicting). A `source` place rooted at that owner (the `ref`
    /// binding the pointer was taken through) is the executable loan place,
    /// `through` link included. An unmapped owner (no live variable) loans
    /// nothing.
    fn pointer_place_loan(
        &mut self,
        place: &mojito_types::origin::OriginPlace,
        mutable: bool,
        source: Option<MirPlace>,
        shared: bool,
    ) -> Vec<MirLoan> {
        let Some(root) = self.owner_vars.get(&place.root).copied() else {
            return Vec::new();
        };
        let mir_place = source
            .filter(|source| source.root == root)
            .unwrap_or_else(|| MirPlace::root(root, self.var_types.get(&root).cloned()));
        let interior = matches!(
            place.path.last(),
            Some(
                mojito_types::origin::OriginSeg::Interior(_)
                    | mojito_types::origin::OriginSeg::Subtree
            )
        )
        .then(|| self.mir_interior_origin(place, Some(root)))
        .flatten();
        vec![MirLoan {
            place: mir_place,
            mutable,
            interior,
            shared,
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
        seen: &mut std::collections::HashSet<mojito_types::origin::OwnerId>,
    ) {
        use mojito_ast::ast::CaptureKind;
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
                shared: false,
            });
        }
        if let Some(callable) = self.nested.get(&capture.binding).cloned() {
            for nested in &callable.captures {
                self.collect_capture_loans(nested, loans, seen);
            }
        }
    }

    pub(super) fn subscript_call_contract(
        &self,
        node: Node<'a>,
        operands: &[MirSubscriptArg],
    ) -> Option<MirSubscriptCall> {
        let contract = node.call_contract()?;
        let mut call = self.mir_subscript_call_contract(contract, operands);
        // A compile-time index built from the enclosing binders
        // (`self[Self.Ts.length - 1 - i]`) is recorded as the expression the
        // checker solved it to, which the elaborator evaluates per instance.
        if let Some((_, index)) = node.index_operands()
            && let [argument] = call.param_arg_regs.as_mut_slice()
            && argument.value.is_some()
        {
            argument.expr = match node.instantiated_args().as_slice() {
                [TyArg::Val(mojito_types::ct::CtValue::Expr(solved))] => Some(solved.clone()),
                _ => self
                    .value_binder_node(index.id())
                    .filter(|value| value.as_constant().is_none()),
            };
        }
        Some(call)
    }

    pub(super) fn mir_subscript_call_contract(
        &self,
        contract: mojito_checked::checked::CheckedCallContract,
        operands: &[MirSubscriptArg],
    ) -> MirSubscriptCall {
        let capture_accesses = contract
            .captures
            .iter()
            .filter_map(|capture| {
                let mojito_types::origin::Origin::Place(place) = &capture.origin else {
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
                value: argument
                    .operand
                    .and_then(|position| match operands.get(position) {
                        Some(MirSubscriptArg::Index(register)) => Some(*register),
                        _ => None,
                    }),
                expr: None,
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

    pub(super) fn call_capture_accesses(&self, node: Node<'a>) -> Vec<MirCaptureAccess> {
        let captures = node
            .call_contract()
            .map(|contract| contract.captures)
            .or_else(|| {
                node.adjustments()
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::CallableCaptureAccesses(
                            captures,
                        ) => Some(captures.clone()),
                        _ => None,
                    })
            });
        captures
            .map(|captures| {
                captures
                    .into_iter()
                    .filter_map(|capture| {
                        let mojito_types::origin::Origin::Place(place) = capture.origin else {
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

    /// The loans an aggregate expression carries, ignoring a materialized
    /// borrow-source slot of the expression itself: what that hidden slot
    /// must loan so the storage the temporary views outlives it. Recursion
    /// into sub-expressions goes through [`Self::aggregate_borrows`], so a
    /// materialized sub-temporary still loans its own slot.
    pub(super) fn aggregate_borrows_unmaterialized(&mut self, node: Node<'a>) -> Vec<MirLoan> {
        let borrow = node.borrow_mutability();
        if let Some(mutable) = borrow
            && let Some(name) = self.name_spelled(node)
            && let Some(var) = self.existing_expression_var(&name, node)
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
            if let Some(mut loan) = self.aliases.get(&var).cloned() {
                loan.mutable = mutable;
                return vec![loan];
            }
            // An owned variable auto-borrowed into reference storage loans
            // its own root place — the same loan an explicit `ref` binding
            // of the place would install.
            let ty = self.var_types.get(&var).cloned();
            return vec![MirLoan {
                place: MirPlace::root(var, ty),
                mutable,
                interior: None,
                shared: false,
            }];
        }
        if let Some(mutable) = borrow
            && matches!(
                node.op(),
                CheckedOp::Member { .. } | CheckedOp::Index { .. } | CheckedOp::TypeApply { .. }
            )
        {
            let place = self.place_of(node);
            let interiors = node.interior_references();
            if interiors.is_empty() {
                return vec![MirLoan {
                    place,
                    mutable,
                    interior: None,
                    shared: false,
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
                        shared: false,
                    }
                })
                .collect();
        }
        if let Some(name) = self.name_spelled(node)
            && let Some(var) = self.existing_expression_var(&name, node)
            && let Some(loans) = self.aggregate_loans.get(&var)
        {
            return loans.clone();
        }
        // A capturing closure (a nested def's name or a lambda expression)
        // flowing into storage loans its REFERENCE captures' owners: the
        // stored value retains their frame slots, so the owners must stay
        // alive (and, for `imm`, unmutated) while the storage lives. Direct
        // nested calls never consult this path, so the loan-free
        // declaration-to-call capture model is preserved; owned copy/move
        // captures are self-contained.
        if matches!(
            node.op(),
            CheckedOp::Identifier(_) | CheckedOp::Lambda { .. }
        ) && let Some(info) = self.nested_info(node)
        {
            let mut loans = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for capture in &info.captures {
                self.collect_capture_loans(capture, &mut loans, &mut seen);
            }
            return loans;
        }
        if let Some(inner) = node.transfer_operand() {
            return self.aggregate_borrows(inner);
        }
        let arguments = || {
            node.args()
                .into_iter()
                .chain(node.kwargs().into_iter().map(|(_, value)| value))
        };
        match node.op() {
            CheckedOp::Call { .. } => {
                // A view construction borrows the places its `ref [origin]`
                // parameters bound (the checker recorded the argument
                // indexes), so the stored aggregate keeps its source alive.
                if let Some(loans) = self.borrowed_ref_arguments(node) {
                    return loans;
                }
                // A checked pointer construction loans exactly its source
                // place, with the mutability the checker inferred from the
                // owner binding. A construction through a `ref` binding
                // carries a subtree origin instead: its loan is the lazy
                // generation domain rooted at the reference's owner.
                if let Some(mojito_checked::checked::SemanticAdjustment::PointerToPlace {
                    mutable,
                }) = node.adjustments().iter().find(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::PointerToPlace { .. }
                    )
                }) {
                    let source = node
                        .kwargs()
                        .first()
                        .expect("checked pointer construction has a 'to=' argument")
                        .1;
                    if let Some(TyKind::Pointer {
                        origin: mojito_types::origin::PointerOrigin::Place { place, .. },
                        ..
                    }) = node.checked.ty.as_ref().map(Ty::kind)
                        && matches!(
                            place.path.last(),
                            Some(mojito_types::origin::OriginSeg::Subtree)
                        )
                    {
                        // The pointer reborrows through the `ref` binding it
                        // was taken from, so its loan keeps that binding's
                        // place and link instead of competing with the
                        // binding's own loan.
                        let source = self.place_of(source);
                        return self.pointer_place_loan(place, *mutable, Some(source), true);
                    }
                    let place = self.place_of(source);
                    return vec![MirLoan {
                        place,
                        mutable: *mutable,
                        interior: None,
                        shared: true,
                    }];
                }
                // A free-function call returning a ref-field struct (a
                // borrowing view) lends its borrowed sources to the result,
                // mirroring the method-receiver rule below: recurse for
                // chained temporaries first, else loan each aggregate-typed
                // place argument the callee could have borrowed.
                if node.adjustments().iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. }
                    )
                }) {
                    let loans: Vec<MirLoan> = arguments()
                        .flat_map(|argument| self.aggregate_borrows(argument))
                        .collect();
                    if !loans.is_empty() {
                        return loans;
                    }
                    let loans: Vec<MirLoan> = arguments()
                        .filter(|argument| argument.lends_to_view())
                        .map(|argument| MirLoan {
                            place: self.place_of(argument),
                            mutable: false,
                            interior: None,
                            shared: false,
                        })
                        .collect();
                    if !loans.is_empty() {
                        return loans;
                    }
                }
                arguments()
                    .flat_map(|argument| self.aggregate_borrows(argument))
                    .collect()
            }
            CheckedOp::ListLit(_) | CheckedOp::TupleLit(_) => node
                .operands()
                .flat_map(|value| self.aggregate_borrows(value))
                .collect(),
            // A view-typed slice result (a Span sub-slice or a StringSpan
            // keyword slice) inherits its receiver's loans.
            CheckedOp::Slice { .. } | CheckedOp::MultiIndex { .. } => {
                if let Some(object) = node.object()
                    && node.adjustments().iter().any(|adjustment| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. }
                        )
                    })
                {
                    let loans = self.aggregate_borrows(object);
                    if !loans.is_empty() {
                        return loans;
                    }
                    // A receiver that is itself the owning place lends that
                    // place to the view.
                    if matches!(
                        object.op(),
                        CheckedOp::Identifier(_) | CheckedOp::Member { .. }
                    ) {
                        return vec![MirLoan {
                            place: self.place_of(object),
                            mutable: false,
                            interior: None,
                            shared: false,
                        }];
                    }
                }
                Vec::new()
            }
            // An `unsafe_origin_cast` result loans exactly its rebound target
            // place; an interior-generation tail becomes the loan's interior
            // domain so container mutation stales it without ordinary reads
            // conflicting.
            CheckedOp::Invoke { .. } | CheckedOp::MethodCall { .. } => {
                let method_object = match node.op() {
                    CheckedOp::MethodCall { .. } => node.object(),
                    _ => None,
                };
                // A static call binding `ref [Self.o]` arguments (the checker
                // recorded the argument indexes, as for a view construction)
                // lends those places to the returned aggregate.
                if method_object.is_some()
                    && let Some(loans) = self.borrowed_ref_arguments(node)
                {
                    return loans;
                }
                let cast = node
                    .adjustments()
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::PointerOriginCast {
                            origin,
                        } => Some(origin),
                        _ => None,
                    });
                if let Some(mojito_types::origin::PointerOrigin::Place { place, mutable }) = cast {
                    return self.pointer_place_loan(place, *mutable, None, false);
                }
                // A method whose selected contract returns an origin-bearing
                // pointer (`xs.unsafe_ptr()`) loans that rebased place as a
                // shared alias, as `Pointer(to=place)` does: copies of the
                // returned pointer coexist.
                if let Some(contract) = node.call_contract()
                    && let TyKind::Pointer {
                        origin: mojito_types::origin::PointerOrigin::Place { place, mutable },
                        ..
                    } = contract.result_ty.kind()
                {
                    return self.pointer_place_loan(&place.clone(), *mutable, None, true);
                }
                // `unsafe_offset` preserves provenance: forward the receiver's
                // loans onto the offset pointer.
                if let Some(object) = method_object
                    && node.adjustments().iter().any(|adjustment| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::PointerOffset
                        )
                    })
                {
                    return self.aggregate_borrows(object);
                }
                // A method returning a ref-field struct (a borrowing
                // view/iterator) lends its receiver to the result, exactly as
                // a view-typed slice result does: a chained temporary
                // (`Span(xs).__iter__()`) forwards only what it borrowed, and
                // a named receiver place is lent itself as well — a view
                // receiver (`sp.__iter__()`) must outlive the result, not
                // only the storage the view borrows. Its aggregate-typed
                // place arguments lend the same way (the free-function rule
                // above).
                if let (Some(named), Some(object)) = (
                    node.adjustments()
                        .iter()
                        .find_map(|adjustment| match adjustment {
                            mojito_checked::checked::SemanticAdjustment::BorrowViewResult {
                                interior,
                                ..
                            } => Some(interior),
                            _ => None,
                        }),
                    method_object,
                ) {
                    let mut loans = self.lent_to_view(object, named);
                    for argument in arguments() {
                        if argument.lends_to_view() {
                            loans.extend(self.lent_to_view(argument, &[]));
                        }
                    }
                    return loans;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The loans of the places a call binds to its `ref [origin]`
    /// parameters (the argument indexes the checker recorded), when it
    /// records any and some argument is there to lend.
    fn borrowed_ref_arguments(&mut self, node: Node<'a>) -> Option<Vec<MirLoan>> {
        let Some(mojito_checked::checked::SemanticAdjustment::BorrowRefArguments {
            arguments, ..
        }) = node.adjustments().iter().find(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowRefArguments { .. }
            )
        })
        else {
            return None;
        };
        let args = node.args();
        let loans = arguments
            .iter()
            .filter_map(|&(index, mutable)| {
                args.get(index).map(|&argument| MirLoan {
                    place: self.place_of(argument),
                    mutable,
                    interior: None,
                    shared: false,
                })
            })
            .collect::<Vec<_>>();
        (!loans.is_empty()).then_some(loans)
    }

    /// The loans a borrowing view a method returns takes on `source`, its
    /// receiver or a place argument it could have borrowed, whose callee
    /// names the interior `named` below it.
    fn lent_to_view(&mut self, source: Node<'a>, named: &[String]) -> Vec<MirLoan> {
        let mut loans = self.aggregate_borrows(source);
        // A receiver inside a container's owned interior lends the interior
        // the callee's return origin names below that element's generation
        // (`ys[0].rstrip()` lends `ys["element"]["bytes"]`), or the whole
        // subtree when it names the receiver itself: sibling element reads
        // coexist with the view, and a container mutation or a store over
        // the element stales it.
        if let Some((container, interiors)) = source.element_receiver() {
            let fallback = self.place_of(container).root;
            for mut origin in interiors {
                if named.is_empty() {
                    origin.path.push(mojito_types::origin::OriginSeg::Subtree);
                } else {
                    origin.path.extend(
                        named
                            .iter()
                            .cloned()
                            .map(mojito_types::origin::OriginSeg::Interior),
                    );
                }
                let Some(interior) = self.mir_interior_origin(&origin, Some(fallback)) else {
                    continue;
                };
                loans.push(MirLoan {
                    place: MirPlace::root(
                        interior.root,
                        self.var_types.get(&interior.root).cloned(),
                    ),
                    mutable: false,
                    interior: Some(interior),
                    shared: false,
                });
            }
            return loans;
        }
        if !matches!(
            source.op(),
            CheckedOp::Identifier(_) | CheckedOp::Member { .. }
        ) {
            return loans;
        }
        let place = self.place_of(source);
        // A local `ref` binding's slot is not an owner; its carried loan
        // already covers the referent. A reference parameter carries no loan
        // into the body, so its slot is the root the view is lent from.
        if (self.runtime_aliases.contains(&place.root) && (place.root as usize) >= self.f.n_params)
            || loans.iter().any(|loan| {
                loan.place.root == place.root && loan.place.is_whole_root() && place.is_whole_root()
            })
        {
            return loans;
        }
        let mutable = source.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowMutable
            )
        });
        let interior = field_interior(&place, named);
        loans.push(MirLoan {
            place,
            mutable,
            interior,
            shared: false,
        });
        loans
    }

    /// The place `node` denotes, lowered from the syntax rebuilt for it
    /// (R646's transitional bridge; R647 lowers the node itself).
    fn place_of(&mut self, node: Node<'a>) -> MirPlace {
        if let Some(syntax) = self.syntax(node) {
            self.place(syntax.expr())
        } else {
            self.emit(MirInstr::Unsupported(
                "a place without the syntax rebuilt for its node reached MIR".into(),
            ));
            MirPlace::root(self.var("$invalid_place"), None)
        }
    }
}

/// The interior domain a view lends from a field receiver (`h.name.strip()`
/// lends `h.name["bytes"]`): its field path followed by the callee's named
/// interior. A store over the field invalidates that domain, so the view's
/// next use is rejected as invalidated, while reads of the field coexist.
fn field_interior(place: &MirPlace, named: &[String]) -> Option<MirInteriorOrigin> {
    if named.is_empty() || place.is_whole_root() {
        return None;
    }
    let fields = place
        .steps()
        .map(|step| match step {
            Proj::Field(name) => Some(mojito_types::origin::OriginSeg::Field(name.clone())),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(MirInteriorOrigin {
        root: place.root,
        path: fields
            .into_iter()
            .chain(
                named
                    .iter()
                    .cloned()
                    .map(mojito_types::origin::OriginSeg::Interior),
            )
            .collect(),
    })
}
