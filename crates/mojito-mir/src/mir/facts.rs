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

    /// Whether this node's value is a fresh owned temporary: a call's
    /// result, a dispatched operator's, which is its dunder's owned return,
    /// a conditional expression's, a subscript's whose selected getter
    /// returns by value, or a display's or comprehension's, which is its
    /// construction's, as upstream emits a collection literal as a
    /// constructor call. Whether its storage is a call-result slot is
    /// [`Self::indirect_result`]'s answer.
    pub(super) fn is_fresh_temporary(self) -> bool {
        self.is_call()
            || matches!(
                self.op(),
                CheckedOp::Infix(..)
                    | CheckedOp::Prefix(..)
                    | CheckedOp::IfExpr { .. }
                    | CheckedOp::ListLit(_)
                    | CheckedOp::BraceLit(_)
                    | CheckedOp::TupleLit(_)
                    | CheckedOp::Comprehension { .. }
            )
            || (matches!(
                self.op(),
                CheckedOp::Index { .. } | CheckedOp::MultiIndex { .. }
            ) && self
                .call_contract()
                .is_some_and(|contract| contract.reference_result.is_none()))
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

    /// The pointer subscript this place node spells — positional `p[i]` on
    /// a pointer-typed object, or current Mojo's keyword `p[unsafe_offset=i]`
    /// — as its pointer object and offset.
    pub(super) fn pointer_offset_subscript(self) -> Option<(Self, Self)> {
        match self.index_operands() {
            Some((object, index))
                if matches!(
                    object.ty().as_ref().map(Ty::kind),
                    Some(TyKind::Pointer { .. })
                ) =>
            {
                Some((object, index))
            }
            Some(_) => None,
            None => self.pointer_keyword_subscript(),
        }
    }

    /// The keyword pointer subscript (`p[unsafe_offset=i]`) this node is, as
    /// its object and offset; every other keyword subscript is a nominal
    /// call.
    pub(super) fn pointer_keyword_subscript(self) -> Option<(Self, Self)> {
        let CheckedOp::MultiIndex { args, .. } = self.op() else {
            return None;
        };
        match args.as_slice() {
            [mojito_checked::checked::CheckedSubscriptArg::Keyword { name, value }]
                if name == "unsafe_offset" =>
            {
                self.object().zip(self.operand_of(*value))
            }
            _ => None,
        }
    }

    /// Whether this field/dereference chain roots at a call result the
    /// checker materialized as a pointer-field holder.
    pub(super) fn rooted_at_materialized_temporary(self) -> bool {
        let mut root = self;
        while let Some(object) = root
            .member_object()
            .or_else(|| root.index_operands().map(|(object, _)| object))
            .or_else(|| root.pointer_keyword_subscript().map(|(object, _)| object))
        {
            root = object;
        }
        root.is_call()
            && mojito_checked::checked::materialized_borrow_owner(root.adjustments()).is_some()
    }

    /// The operand of a transfer: `None` when this is not one.
    pub(super) fn transfer_operand(self) -> Option<Self> {
        self.sole_operand(|op| matches!(op, CheckedOp::Transfer(_)))
    }

    /// How this `^` takes its source, as the check decided it: `None` when
    /// this is no transfer, or one the check bound to a read parameter.
    pub(super) fn transfer_verdict(self) -> Option<mojito_checked::checked::TransferVerdict> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::Transfer(verdict) => Some(*verdict),
                _ => None,
            })
    }

    /// The receiver method call `self` evaluates for `object`: an explicit
    /// destroy (`x^.finish()`) names the transfer's operand, whose place
    /// its `deinit` receiver consumes, as upstream's call consumes a
    /// `DeinitMem` operand's address; any other call names `object`.
    pub(super) fn destroyed_receiver(self, object: Self) -> Self {
        let explicit_destroy = self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ExplicitDestroy
            )
        });
        explicit_destroy
            .then(|| object.transfer_operand())
            .flatten()
            .unwrap_or(object)
    }

    /// The operand of a spread: `None` when this is not one.
    pub(super) fn spread_operand(self) -> Option<Self> {
        self.sole_operand(|op| matches!(op, CheckedOp::Spread(_)))
    }

    /// The value of a walrus: `None` when this is not one.
    pub(super) fn named_value(self) -> Option<Self> {
        match self.op() {
            CheckedOp::Named { value, .. } => self.operand_of(*value),
            _ => None,
        }
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

    /// Whether this is `v._mlir_value` on a SIMD value, whose storage is
    /// the value itself.
    pub(super) fn is_simd_storage(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::SimdStorage
            )
        })
    }

    /// Project `place`, the place of this member node's object, to the
    /// member `field`; a SIMD value's storage is the value's own place.
    pub(super) fn project_member(self, place: &mut MirPlace, field: &str) {
        if self.is_simd_storage() {
            return;
        }
        match self.place_ty().or_else(|| self.ty()) {
            Some(ty) => place.project(Proj::Field(field.to_string()), ty),
            None => place.proj.push(Proj::Field(field.to_string())),
        }
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

    pub(super) fn interior_invalidations(
        self,
    ) -> Vec<mojito_checked::checked::InteriorInvalidation> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::InvalidateInteriors {
                    invalidations,
                } => Some(invalidations),
                _ => None,
            })
            .cloned()
            .unwrap_or_default()
    }

    /// Whether this call's checked result is a borrowing view of its
    /// arguments (`BorrowViewResult`): the caller-side loans lend the place
    /// arguments to the result, so a read-convention place argument must
    /// reach the callee as the caller's storage rather than a copy — a view
    /// built over a callee-local copy would dangle once the frame unwinds.
    pub(super) fn borrows_view_result(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::BorrowViewResult { .. }
            )
        })
    }

    /// Whether this call's result borrows its read-convention place
    /// arguments — a borrowing view ([`Self::borrows_view_result`]) or a
    /// reference result — so each must reach the callee as the caller's
    /// storage, a register-passable one included: a result rooted in a
    /// callee-local copy would dangle once the frame unwinds. A memory-only
    /// read reaches it so at every call, as upstream passes it by address.
    pub(super) fn lends_read_arguments(self) -> bool {
        self.borrows_view_result() || self.reference_result().is_some()
    }

    /// Whether a checked method call writes through its receiver place. The
    /// checker's effective receiver convention already collapses a `ref self`
    /// reached through an immutable reference to `Imm`; a call without a
    /// checked contract keeps the conservative exclusive classification.
    pub(super) fn receiver_writes(self) -> bool {
        self.call_contract().is_none_or(|contract| {
            matches!(
                contract.receiver_convention,
                Some(
                    mojito_ast::ast::ArgConvention::Mut
                        | mojito_ast::ast::ArgConvention::Ref
                        | mojito_ast::ast::ArgConvention::Var
                        | mojito_ast::ast::ArgConvention::Deinit
                )
            )
        })
    }

    /// The selected callee's receiver convention, which a method call
    /// records as upstream's call carries each operand's: a `deinit` one
    /// consumes its receiver place (upstream's `DeinitMem`). A call the
    /// check recorded no contract for takes its receiver `var` when the
    /// check marked `receiver` as taken by an owned receiver.
    pub(super) fn receiver_convention(
        self,
        receiver: Node<'_>,
    ) -> Option<mojito_ast::ast::ArgConvention> {
        match self.call_contract() {
            Some(contract) => contract.receiver_convention,
            None => receiver
                .forwarded()
                .then_some(mojito_ast::ast::ArgConvention::Var),
        }
    }

    /// Whether the check marked this operand as taken by an owned
    /// parameter, receiver, or operator operand (`ForwardedArgument`).
    pub(super) fn forwarded(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ForwardedArgument
            )
        })
    }

    /// Whether a callable-value (indirect) or method call's argument list
    /// may anchor loan-carrying temporaries — the same rule as a plain
    /// function call (see `allow_argument_anchors`): a callee borrowing
    /// through checker-selected `ref` arguments or recorded transfer effects
    /// carries a temporary's loans through that channel already, so anchoring
    /// there adds a conflicting duplicate borrow. (A plain call additionally
    /// excludes constructions; a method or callable value is never one.)
    pub(super) fn call_anchors_arguments(self) -> bool {
        self.checked.call_transfers.is_empty()
            && !self.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowRefArguments { .. }
                )
            })
    }

    /// `place`, the place of this `rebind[dest](e)` node's operand, followed by a
    /// [`Proj::Rebind`] step to `dest`: the same storage, read and written at
    /// the type the rebind names, as upstream's reference overload rebinds
    /// the pointer it returns. A later projection addresses `dest`.
    pub(super) fn rebound_place(self, mut place: MirPlace) -> MirPlace {
        if let Some((_, dest)) = self.rebind_target()
            && place.ty.as_ref() != Some(&dest)
        {
            place.project(Proj::Rebind, dest);
        }
        place
    }

    pub(super) fn reference_result(self) -> Option<mojito_types::origin::RefTy> {
        // The selected-call contract is the canonical checked handoff.  In
        // particular, a free-function reference result may share its source
        // expression with another compatibility adjustment, so consulting only
        // the legacy single-operation slot can silently lose `ref[a, b] T` and
        // type a runtime handle local as ordinary `T` storage.
        self.call_contract()
            .and_then(|contract| contract.reference_result)
            .or_else(|| {
                self.adjustments()
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                            reference,
                        } => Some(reference),
                        mojito_checked::checked::SemanticAdjustment::AugmentedInPlace {
                            place_reference,
                            ..
                        } => place_reference.as_ref(),
                        _ => None,
                    })
                    .cloned()
            })
    }

    /// The parameter expression the checker recorded for this node, a
    /// compile-time query read as a runtime value.
    pub(super) fn recorded_value(self) -> Option<mojito_types::param_expr::ParamExpr> {
        recorded_param_value(&self.checked.adjustments)
    }

    /// Whether this node reads a parameter value the check recorded with no
    /// materialized borrow source of its own (a module constant), which
    /// lowers as a value wherever it stands.
    pub(super) fn constant_param_value(self) -> bool {
        constant_param_value(&self.checked.adjustments)
    }

    /// The alternative index of a checked Variant projection recorded on this
    /// node (`v[T]`, `storage[T]`), if any.
    pub(super) fn variant_projection_index(self) -> Option<mojito_types::types::VariantIndex> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::VariantProject { index, .. } => {
                    Some(index)
                }
                _ => None,
            })
            .cloned()
    }

    /// Whether this node's checked type is a pointer whose provenance
    /// designates a single checked place. Dereferencing such a pointer goes
    /// through its frame/slot handle instead of allocation arithmetic. A
    /// multi-element origin-bearing pointer (an interior-generation domain)
    /// is heap storage: it keeps allocation arithmetic and carries its loan
    /// on the binding instead.
    pub(super) fn is_origin_bearing_pointer(self) -> bool {
        origin_bearing_pointer(self.checked.ty.as_ref())
    }

    /// Whether this node is a `Pointer(to=place)` construction whose checked
    /// origin is that place, the one case where a pointer's origin also
    /// designates its address. A construction through a `ref` binding
    /// carries a subtree origin, which forgets the exact descendant the
    /// runtime handle addresses.
    pub(super) fn forms_place_address(self) -> bool {
        matches!(self.op(), CheckedOp::Call { .. })
            && self.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::PointerToPlace { .. }
                )
            })
            && !matches!(
                self.checked.ty.as_ref().map(Ty::kind),
                Some(TyKind::Pointer { origin, .. }) if origin.subtree()
            )
    }

    pub(super) fn resolved_callable(self) -> Option<String> {
        callable_target(self.adjustments())
    }

    /// What each link of this chained comparison dispatched, in link order;
    /// empty when the check recorded none.
    pub(super) fn compare_links(self) -> &'a [mojito_checked::checked::CheckedCompareLink] {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::CompareLinks(links) => {
                    Some(links.as_slice())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    pub(super) fn implicit_conversion(self) -> Option<String> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ImplicitConversion(target) => {
                    Some(target)
                }
                _ => None,
            })
            .cloned()
    }

    pub(super) fn index_normalization(self) -> Option<String> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::IndexNormalization { target } => {
                    Some(target)
                }
                _ => None,
            })
            .cloned()
    }

    /// The compile-time-index accessor call a nominal subscript resolved to
    /// (`t[0]` on a `Tuple`, `__getitem_param__[0]`), whose `ref` result
    /// into the element is the subscript's place.
    pub(super) fn element_accessor_call(
        self,
    ) -> Option<mojito_checked::checked::CheckedCallContract> {
        if !matches!(self.op(), CheckedOp::Index { .. }) {
            return None;
        }
        let contract = self.call_contract()?;
        let accessor = contract.target.ends_with(".__getitem_param__")
            || contract.target.contains(".__getitem_param__$");
        let returns_element = contract.arguments.is_empty() && contract.reference_result.is_some();
        (accessor && returns_element).then_some(contract)
    }

    pub(super) fn implicitly_copies_consuming_receiver(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ImplicitlyCopyConsumingReceiver
            )
        })
    }

    /// Whether this conditional expression's result is proven
    /// register-passable, so its `if` yields the value.
    pub(super) fn register_passable_result(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RegisterPassableResult
            )
        })
    }

    /// Whether this read argument's type is proven register-passable, so
    /// it is passed by value rather than by address.
    pub(super) fn register_passable_read(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::RegisterPassableRead
            )
        })
    }

    /// Whether this closure's storage value is converted to the closure of
    /// its body ([`mojito_checked::checked::SemanticAdjustment::ClosureCallable`]).
    pub(super) fn closure_callable(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ClosureCallable
            )
        })
    }

    /// Whether the value this node lowers to is returned through memory, as
    /// upstream's `ByRefResult` argument: an implicit conversion's when its
    /// constructor's is, a conditional expression's when the checker did
    /// not prove it register-passable, and otherwise a fresh temporary's
    /// when the checker found its selected callee's is
    /// (`IndirectResult`).
    pub(super) fn indirect_result(self) -> bool {
        use mojito_checked::checked::SemanticAdjustment;
        let recorded =
            |wanted: fn(&SemanticAdjustment) -> bool| self.adjustments().iter().any(wanted);
        if self.implicit_conversion().is_some() {
            return recorded(|adjustment| {
                matches!(adjustment, SemanticAdjustment::ConversionIndirectResult)
            });
        }
        if matches!(self.op(), CheckedOp::IfExpr { .. }) {
            return !self.register_passable_result();
        }
        self.is_fresh_temporary()
            && self.reference_result().is_none()
            && recorded(|adjustment| matches!(adjustment, SemanticAdjustment::IndirectResult))
    }

    /// Whether this conditional expression's condition is a parameter
    /// value, so it lowers as `hlcf.comptime.if`.
    pub(super) fn comptime_conditional(self) -> bool {
        self.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::ComptimeConditional
            )
        })
    }

    pub(super) fn literal_materialization(self) -> Option<Ty> {
        if !matches!(
            self.ty().as_ref().map(Ty::kind),
            Some(TyKind::IntLiteral | TyKind::FloatLiteral)
        ) {
            // Checker fact tables are still keyed by source span while HIR owns
            // stable node identities. Comptime expansion can produce several
            // nodes at one source span; only a node whose checked source type is
            // actually a literal may consume a materialization adjustment.
            return None;
        }
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(target) => {
                    Some(target)
                }
                _ => None,
            })
            .cloned()
    }

    pub(super) fn scalar_splat(self) -> Option<Ty> {
        self.adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SplatScalar(target) => Some(target),
                _ => None,
            })
            .cloned()
    }

    pub(super) fn intrinsic_index_dispatch(self) -> Option<MirIntrinsicSubscript> {
        let ty = self.ty()?;
        match ty.kind() {
            TyKind::Tuple(_) | TyKind::RuntimePack(_) => Some(MirIntrinsicSubscript::TupleStorage),
            TyKind::VariadicPack(_) => Some(MirIntrinsicSubscript::VariadicStorage),
            TyKind::Simd { .. } => Some(MirIntrinsicSubscript::Simd),
            TyKind::Pointer { .. } => Some(MirIntrinsicSubscript::Pointer),
            TyKind::ComptimeList(_) => Some(MirIntrinsicSubscript::ComptimeList),
            // `Slice.indices` has a public nominal Tuple type but crosses the
            // VM intrinsic boundary as compiler-private `Value::Tuple`
            // storage. A checker-selected public Tuple accessor carries a
            // call contract and therefore never consults this fallback.
            TyKind::Struct(..) if tuple_elements(&ty).is_some() => {
                Some(MirIntrinsicSubscript::TupleStorage)
            }
            _ => None,
        }
    }

    /// The value of this integer literal node as a host index, without
    /// materializing it as a runtime scalar; any other index expression
    /// lowers through a register.
    pub(super) fn exact_nonnegative_index(self) -> Option<usize> {
        let CheckedOp::Int(value) = self.op() else {
            return None;
        };
        value.to_u64().and_then(|value| usize::try_from(value).ok())
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
            let place = self.place(node);
            return vec![MirLoan {
                place,
                mutable: *mutable,
                interior: None,
                shared: false,
                setup: false,
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
                setup: false,
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
            setup: false,
        }]
    }

    /// One loan per reference capture (transitively through captured closure
    /// slots), rooted at the captured owner's variable. `imm` loans
    /// immutably; `mut`/`ref` loan mutably; copy/move captures live in the
    /// closure's storage and stop the walk.
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
                setup: false,
            });
        }
        if let Some(callable) = self.nested.get(&capture.binding).cloned() {
            for nested in &callable.captures {
                self.collect_capture_loans(nested, loans, seen);
            }
        }
    }

    pub(super) fn subscript_call_contract(
        &mut self,
        node: Node<'a>,
        operands: &[MirSubscriptArg],
    ) -> Option<MirSubscriptCall> {
        let contract = node.call_contract()?;
        let mut call = self.mir_subscript_call_contract(contract, operands, node.location());
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

    /// The subscript call of `contract` over `operands`, its raise edge's
    /// registers fresh at `span`.
    pub(super) fn mir_subscript_call_contract(
        &mut self,
        contract: mojito_checked::checked::CheckedCallContract,
        operands: &[MirSubscriptArg],
        span: SourceSpan,
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
        let raises = self.raises(contract.raises, span);
        MirSubscriptCall {
            target: contract.target,
            raises,
            result_ty: contract.result_ty,
            receiver_requires_place: contract.receiver_requires_place,
            receiver_convention: contract.receiver_convention,
            arguments: contract.arguments,
            capture_accesses,
            reference_result: contract.reference_result,
            param_arg_regs,
            param_decls: contract.param_decls,
            refined_bound: contract.refined_bound,
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
                setup: false,
            }];
        }
        if let Some(mutable) = borrow
            && matches!(
                node.op(),
                CheckedOp::Member { .. } | CheckedOp::Index { .. } | CheckedOp::TypeApply { .. }
            )
        {
            let place = self.place(node);
            let interiors = node.interior_references();
            if interiors.is_empty() {
                return vec![MirLoan {
                    place,
                    mutable,
                    interior: None,
                    shared: false,
                    setup: false,
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
                        setup: false,
                    }
                })
                .collect();
        }
        if let Some(name) = self.name_spelled(node)
            && let Some(var) = self.existing_expression_var(&name, node)
            && let Some(loans) = self.aggregate_loans.get(&var)
            // A value read through a reference binding is the referent's
            // copy: it carries the binding's loans only when its type can
            // hold a reference of its own.
            && !((self.aliases.contains_key(&var) || self.runtime_aliases.contains(&var))
                && !node.ty().is_some_and(|ty| carries_references(&ty)))
        {
            return loans.clone();
        }
        // A capturing closure (a nested def's name or a lambda expression)
        // flowing into storage loans its REFERENCE captures' owners: the
        // stored value retains their frame slots, so the owners must stay
        // alive (and, for `imm`, unmutated) while the storage lives. Direct
        // nested calls never consult this path, so the loan-free
        // declaration-to-call capture model is preserved; copy/move captures
        // live in the closure's storage.
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
                    ..
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
                        let source = self.place(source);
                        return self.pointer_place_loan(place, *mutable, Some(source), true);
                    }
                    let place = self.place(source);
                    return vec![MirLoan {
                        place,
                        mutable: *mutable,
                        interior: None,
                        shared: true,
                        setup: false,
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
                            place: self.place(argument),
                            mutable: false,
                            interior: None,
                            shared: false,
                            setup: false,
                        })
                        .collect();
                    if !loans.is_empty() {
                        return loans;
                    }
                }
                // A result whose type holds no reference, an `Int` from
                // `len(xs[1:3])`, keeps none of its arguments' loans.
                if !node.ty().is_none_or(|ty| carries_references(&ty)) {
                    return Vec::new();
                }
                arguments()
                    .flat_map(|argument| self.aggregate_borrows(argument))
                    .collect()
            }
            CheckedOp::ListLit(_) | CheckedOp::TupleLit(_) => node
                .operands()
                .flat_map(|value| self.aggregate_borrows(value))
                .collect(),
            // A view-typed subscript result (a Span sub-slice or a
            // StringSpan keyword slice) inherits its receiver's loans.
            CheckedOp::Index { .. } | CheckedOp::Slice { .. } | CheckedOp::MultiIndex { .. }
                if !matches!(node.op(), CheckedOp::Index { .. }) || node.borrows_view_result() =>
            {
                if let Some(object) = node.object()
                    && node.borrows_view_result()
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
                            place: self.place(object),
                            mutable: false,
                            interior: None,
                            shared: false,
                            setup: false,
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
                            ..
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
                            mojito_checked::checked::SemanticAdjustment::PointerOffset { .. }
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
                    place: self.place(argument),
                    mutable,
                    interior: None,
                    shared: false,
                    setup: false,
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
            let fallback = self.place(container).root;
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
                    setup: false,
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
        let place = self.place(source);
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
            setup: false,
        });
        loans
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

/// Whether a value of `ty` can hold a reference to storage outside itself,
/// so that its copy carries its source's loans: a reference, a pointer, a
/// callable (its captures), a type not yet bound, or an aggregate one of
/// whose arguments names an origin or such a type.
fn carries_references(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Ref(reference) => carries_references(&reference.referent),
        TyKind::Pointer { .. }
        | TyKind::Func(_)
        | TyKind::GenericFunc(_)
        | TyKind::Overload(_)
        | TyKind::Param { .. }
        | TyKind::Assoc { .. }
        | TyKind::Dependent(_)
        | TyKind::SelfType
        | TyKind::Infer => true,
        TyKind::Struct(_, args) => args.iter().any(|arg| match arg {
            mojito_types::types::TyArg::Ty(ty) => carries_references(ty),
            mojito_types::types::TyArg::Origin(_) => true,
            mojito_types::types::TyArg::Val(_) => false,
        }),
        TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => {
            items.iter().any(carries_references)
        }
        TyKind::VariadicPack(item)
        | TyKind::ComptimeList(item)
        | TyKind::PopArray { element: item, .. } => carries_references(item),
        _ => false,
    }
}
