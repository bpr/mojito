//! Statement, place, subscript-assignment, `try`-region, and terminator lowering,
//! including the `lower_stmt`/`lower_instr` dispatchers.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::call_results::CValue;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CheckedOp, CheckedTarget, CheckedTargetKind, CheckedTupleUnpackElement,
};
use mojito_types::types::TyKind;

/// The state one destructuring assignment's leaves share: its place leaves
/// in order (and the stores lowered for the non-name ones), the value's
/// loans, and the source place a `ref` leaf borrows.
struct DestructureFrame<'a, 'p> {
    places: &'p [Node<'a>],
    lowered: Vec<Option<LeafStore<'a>>>,
    next: usize,
    loans: Vec<MirLoan>,
    root_place: Option<MirPlace>,
    value: Node<'a>,
}

/// A destructuring place leaf's destination, evaluated before the value as
/// upstream forms each element's lvalue first: a place written directly, or
/// a nominal subscript's setter, which the element is passed to.
enum LeafStore<'a> {
    Place(MirPlace),
    Setter(Box<SubscriptSetter<'a>>),
}

/// A nominal subscript store's operands, lowered before its value: the
/// `target` and its index argument nodes, the receiver with the place a
/// `mut self` setter writes back, the index arguments, the keyword the
/// value is passed under, and the selected `__setitem__`.
struct SubscriptSetter<'a> {
    target: Node<'a>,
    indices: Vec<Node<'a>>,
    receiver: Reg,
    receiver_place: Option<MirPlace>,
    args: Vec<MirSubscriptArg>,
    arg_places: Vec<Option<MirPlace>>,
    value_keyword: String,
    call: MirSubscriptCall,
}

/// One `receiver[arguments] OP= rhs` store's checked facts: the subscript
/// `target`, its accessor `plan`, the keyword the setter takes its value
/// under (none for a store through the getter's reference), the operator,
/// and the right-hand side.
struct AugmentedStore<'a> {
    target: Node<'a>,
    plan: mojito_checked::checked::CheckedAugmentedSubscript,
    value_keyword: Option<String>,
    op: InfixOp,
    rhs: Node<'a>,
}

/// An augmented store's right-hand side as its operator step consumes it:
/// the lowered `expression`'s argument registers and retained places.
struct AugmentedOperand<'a> {
    expression: Node<'a>,
    args: Vec<Reg>,
    places: Vec<Option<MirPlace>>,
}

impl<'a> Flatten<'a> {
    /// Lower one HIR instruction into `self.cur`.
    pub(super) fn lower_instr(&mut self, i: &'a HirInstr) {
        // Bracket each HIR instruction for temporary-argument anchors (see
        // `lower_stmt`, whose statements arrive through this dispatcher too):
        // hidden `$arg_loan_r` slots stay live until the consuming call has
        // returned — at the latest the instruction's end — via a `KeepAlive`
        // use; an assignment flushes them before its store.
        let saved = std::mem::take(&mut self.pending_argument_anchors);
        self.lower_instr_dispatch(i);
        self.flush_argument_anchors();
        self.pending_argument_anchors = saved;
    }

    fn lower_instr_dispatch(&mut self, i: &'a HirInstr) {
        match i {
            HirInstr::Bind {
                dest,
                expr,
                binding_ty,
                binding,
            } => self.with_statement_loans(|this| {
                this.lower_bind(*dest, this.handle(expr), binding_ty.as_ref(), *binding);
            }),
            HirInstr::Declare { dest, ty, binding } => {
                self.var_types.insert(*dest, ty.clone());
                if let Some(binding) = binding {
                    self.owner_vars.insert(*binding, *dest);
                }
                self.emit(MirInstr::DeclareVar { var: *dest });
            }
            HirInstr::BorrowIter {
                dest,
                expr,
                origin,
                mutable,
            } => {
                let place = self.place_hir(expr);
                let node = self.hir_node(expr);
                let location = node.location();
                let value_ty = node
                    .ty
                    .clone()
                    .or_else(|| place.ty.clone())
                    .expect("checked borrowed iterator place has a type");
                self.borrow_iteration_source(*dest, location, place, value_ty, origin, *mutable);
            }
            // The checker's `with` desugar anchors a manager's liveness at the
            // end of its block with `_mojito_keep_alive(name)`: a use of the
            // named slot (resolved by its checked binding, like any read)
            // without a value, so nothing is evaluated.
            HirInstr::Eval(e) if let Some((owner, local)) = self.kept_alive(e) => {
                let var = match owner {
                    Some(owner) => self.binding_var(owner, &local),
                    None => self.var(&local),
                };
                self.emit(MirInstr::KeepAlive { var });
            }
            HirInstr::Eval(e) => self.discard_result(e),
            HirInstr::Stmt(s) => self.lower_hir_stmt(s),
            HirInstr::Try {
                body,
                except,
                orelse,
                finalbody,
            } => self.with_statement_loans(|this| {
                this.emit_try(body, except.as_ref(), orelse.as_ref(), finalbody.as_ref());
            }),
            HirInstr::KeepAlive(var) => {
                self.emit(MirInstr::KeepAlive { var: *var });
            }
            HirInstr::If {
                cond,
                then,
                elifs,
                orelse,
            } => {
                let cond = match cond {
                    hir::HirCond::Expr(cond) => self.condition_hir(cond),
                    hir::HirCond::Slot(slot) => {
                        let cond = self.fresh(SourceSpan::new(None, DUMMY_SPAN), Some(*slot));
                        self.emit(MirInstr::UseVar {
                            dest: cond,
                            var: *slot,
                            mode: UseMode::Copy,
                            constructor: None,
                        });
                        cond
                    }
                };
                self.flush_argument_anchors();
                let then = self.hir_region(then);
                let elifs = elifs
                    .iter()
                    .map(|elif| MirElif {
                        cond_args: Vec::new(),
                        cond: self.hir_region(&elif.cond),
                        then_args: Vec::new(),
                        then: self.hir_region(&elif.then),
                    })
                    .collect();
                let orelse = self.hir_region(orelse);
                self.emit(MirInstr::If {
                    results: Vec::new(),
                    cond,
                    then,
                    elifs,
                    else_args: Vec::new(),
                    orelse,
                });
            }
            HirInstr::ComptimeIf {
                conds,
                arms,
                orelse,
            } => {
                let conds = conds.iter().map(|c| self.comptime_condition(c)).collect();
                let arms = arms.iter().map(|arm| self.hir_region(arm)).collect();
                let orelse = self.hir_region(orelse);
                self.emit(MirInstr::ComptimeIf {
                    results: Vec::new(),
                    conds,
                    arms,
                    orelse,
                });
            }
            HirInstr::ComptimeAssert { cond, message } => {
                let cond = self.comptime_condition(cond);
                let cond = match message {
                    Some(message) => {
                        GenericConstraint::WithMessage(Box::new(cond), message.clone())
                    }
                    None => cond,
                };
                self.emit(MirInstr::ComptimeAssert {
                    cond: Box::new(cond),
                });
            }
            HirInstr::Loop { label, body } => {
                let body = self.hir_region(body);
                self.emit(MirInstr::Loop {
                    label: Some(LoopLabel(*label)),
                    operands: Vec::new(),
                    args: Vec::new(),
                    results: Vec::new(),
                    body,
                });
            }
            HirInstr::Match { cases, orelse } => {
                let cases = cases
                    .iter()
                    .map(|case| MirCase {
                        args: Vec::new(),
                        body: self.hir_region(case),
                    })
                    .collect();
                let orelse = self.hir_region(orelse);
                self.emit(MirInstr::Match {
                    results: Vec::new(),
                    cases,
                    else_args: Vec::new(),
                    orelse,
                });
            }
            HirInstr::ComptimeFor {
                iter,
                binding,
                body,
                orelse,
                ..
            } => self.lower_comptime_for(iter, *binding, body, orelse),
            // Iterator protocol: compute into a register, then store to the target
            // variable (so the header's branch can read `has_next` as a `UseVar`,
            // and the body binds the loop variable).
            HirInstr::GetIter {
                source,
                dest,
                protocol,
            } => {
                // `GetIter` declares a split iterator slot, typed by the
                // checked `__iter__` result.
                if source != dest {
                    self.var_types.insert(*dest, protocol.iterator_ty.clone());
                }
                self.normalize_iterator(*source, *dest, protocol);
            }
            HirInstr::HasNext { iter, dest } => {
                let r = self.fresh(SourceSpan::new(None, DUMMY_SPAN), None);
                self.emit(MirInstr::HasNext {
                    dest: r,
                    iter: *iter,
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: r,
                    binding_ty: None,
                    in_place: false,
                });
            }
            HirInstr::Next {
                iter,
                raw,
                element_ty,
            } => {
                let r = self.fresh_typed(
                    SourceSpan::new(None, DUMMY_SPAN),
                    Some(*iter),
                    element_ty.clone(),
                );
                self.emit(MirInstr::Next {
                    dest: r,
                    iter: *iter,
                });
                // The raw element is retained untouched in a compiler-owned
                // slot; `BindIteration` adapts it to the loop target only on the
                // yielded edge.
                self.emit(MirInstr::DefVar {
                    var: *raw,
                    src: r,
                    binding_ty: Some(element_ty.clone()),
                    in_place: false,
                });
            }
            HirInstr::CallNext {
                iter,
                raw,
                call,
                error,
                element_ty,
            } => self.call_raising_next(*iter, (*raw, element_ty), call, error),
            HirInstr::BindIteration {
                raw,
                dest,
                iter,
                plan,
                binding,
            } => self.bind_iteration_result(plan, *raw, *dest, *iter, *binding),
        }
    }

    /// Adapt one raw `__next__` result (`raw`) into the user-visible loop target
    /// (`dest`) per the checked [`CheckedIterationBinding`]. This runs only in the
    /// yielded body, so moves and lifecycle copies never execute on the
    /// `StopIteration` edge. Source ownership (borrowed vs consuming `__iter__`)
    /// is already fixed upstream; here only the target convention matters.
    pub(super) fn bind_iteration_result(
        &mut self,
        plan: &mojito_checked::checked::CheckedIterationBinding,
        raw: VarId,
        dest: VarId,
        iterator: VarId,
        binding: mojito_types::origin::OwnerId,
    ) {
        use mojito_checked::checked::IterationBindingAction;
        let span = SourceSpan::new(None, DUMMY_SPAN);
        self.owner_vars.insert(binding, dest);
        match plan.action {
            // A value result binds directly into the loop variable's own
            // storage: `var` owns and may transfer it onward; an immutable or
            // `ref` target (`BorrowValue`) instead drops it at each iteration's
            // end. Both simply move the freshly yielded value into `dest`; the
            // binding's declared mutability enforces the difference.
            IterationBindingAction::MoveValue | IterationBindingAction::BorrowValue => {
                let value = self.fresh_typed(span, Some(raw), plan.binding_ty.clone());
                self.emit(MirInstr::UseVar {
                    dest: value,
                    var: raw,
                    mode: UseMode::Move,
                    constructor: None,
                });
                self.var_types.insert(dest, plan.binding_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: dest,
                    src: value,
                    binding_ty: Some(plan.binding_ty.clone()),
                    in_place: false,
                });
            }
            // `var` target over a reference result: read through the handle and
            // lifecycle-copy the referent into owned storage. `MakeRef` forwards
            // the handle stored in `raw` unchanged; a plain read would instead
            // dereference it and lose the reference identity.
            IterationBindingAction::CopyReference => {
                let handle = self.fresh_typed(span.clone(), Some(raw), plan.yielded_ty.clone());
                self.emit(MirInstr::MakeRef {
                    dest: handle,
                    place: MirPlace::root(raw, Some(plan.yielded_ty.clone())),
                });
                let read = self.fresh_typed(span.clone(), None, plan.binding_ty.clone());
                self.emit(MirInstr::ReadRef {
                    dest: read,
                    reference: handle,
                });
                let owned = self.fresh_typed(span, None, plan.binding_ty.clone());
                self.emit(MirInstr::CopyValue {
                    dest: owned,
                    value: read,
                    constructor: None,
                });
                self.var_types.insert(dest, plan.binding_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: dest,
                    src: owned,
                    binding_ty: Some(plan.binding_ty.clone()),
                    in_place: false,
                });
            }
            // Immutable/`ref` target over a reference result: keep the yielded
            // handle as the binding so body accesses read/write through it.
            // `MakeRef` forwards `raw`'s handle unchanged.
            IterationBindingAction::BorrowReference => {
                let handle = self.fresh_typed(span, Some(raw), plan.binding_ty.clone());
                self.emit(MirInstr::MakeRef {
                    dest: handle,
                    place: MirPlace::root(raw, Some(plan.yielded_ty.clone())),
                });
                self.runtime_aliases.insert(dest);
                self.var_types.insert(dest, plan.binding_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: dest,
                    src: handle,
                    binding_ty: Some(plan.binding_ty.clone()),
                    in_place: false,
                });
                // The borrowed handle aliases the iterated source: re-establish
                // the iterator's interior source loans on the binding so a
                // structural invalidation names the user's variable, not just
                // the compiler's iterator slot. A whole-place loan stays on the
                // iterator object alone: it is exclusive when mutable, and the
                // binding is a reborrow of it, not a competing borrow.
                self.reestablish_interior_source_loans(iterator, dest);
            }
        }
    }

    /// Bind a borrowed iteration source into its retained-source slot: a genuine
    /// reference into the source place (never a value copy), with the owner
    /// dependency established at the granularity the checker proved — an interior
    /// element generation when the origin ends in an `Interior` segment, a
    /// whole-place loan otherwise. The reference itself always designates the
    /// whole retained source; granularity lives only in the loan, and so does
    /// mutability: a `for ref` loop over a mutable source lends it mutably
    /// (exclusive against a whole-place loan of the same source), a value loop
    /// lends it shared. Shared by `for` statements (`HirInstr::BorrowIter`) and
    /// comprehension clauses.
    pub(super) fn borrow_iteration_source(
        &mut self,
        dest: VarId,
        span: SourceSpan,
        place: MirPlace,
        value_ty: Ty,
        origin: &mojito_types::origin::OriginPlace,
        mutable: bool,
    ) {
        let mut loan_place = place.clone();
        let canonical = self.direct_borrow_interior(&mut loan_place, origin);
        let interior = canonical
            .path
            .iter()
            .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
            .then_some(canonical);
        let mut ref_origin = origin.clone();
        if let Some(first_interior) = ref_origin
            .path
            .iter()
            .position(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
        {
            ref_origin.path.truncate(first_interior);
        }
        // The slot holds only a handle: `GetIter` reads it to normalize the
        // iterator (re-rooting a `ref self` `__iter__` at the source), after
        // which dropping the handle is a no-op. The source is never copied.
        let ref_ty = Ty::new(TyKind::Ref(mojito_types::origin::RefTy {
            referent: value_ty,
            origin: mojito_types::origin::Origin::Place(ref_origin),
            mutability: mojito_types::origin::Mutability::Immutable,
        }));
        self.var_types.insert(dest, ref_ty.clone());
        self.iteration_sources.insert(dest, place.clone());
        let handle = self.fresh_typed(span.clone(), Some(place.root), ref_ty.clone());
        self.emit(MirInstr::MakeRef {
            dest: handle,
            place,
        });
        self.emit(MirInstr::DefVar {
            var: dest,
            src: handle,
            binding_ty: Some(ref_ty),
            in_place: false,
        });
        let loans = vec![MirLoan {
            place: loan_place,
            mutable,
            interior,
            shared: false,
            setup: false,
        }];
        let marker = self.fresh_typed(span, Some(loans[0].place.root), Ty::none());
        self.emit(MirInstr::EstablishLoans {
            reference: dest,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(dest, loans);
    }

    /// Normalize the iterable in `source` into the iterator slot `dest`: a
    /// nominal iterable's `__iter__` call ([`Self::call_iter`]), whose
    /// iterator has a slot of its own, or compiler-private storage's
    /// `GetIter`. When the iterator may refer to a borrowed source, its
    /// source loans are re-established on the long-lived iterator slot so
    /// they stay live through the whole loop, rejecting mutation of the
    /// source during iteration. An iterator that cannot refer to its source
    /// leaves it free to die ASAP. Shared by `for` statements and
    /// comprehension clauses.
    pub(super) fn normalize_iterator(
        &mut self,
        source: VarId,
        dest: VarId,
        protocol: &mojito_checked::checked::IterationProtocol,
    ) {
        let borrowed_place = self.iteration_sources.remove(&source);
        match &protocol.prepare {
            Some(selected) => {
                let error = protocol.prepare_raises.as_ref();
                self.call_iter(source, dest, borrowed_place, protocol, selected, error);
            }
            None => self.emit(MirInstr::GetIter { source, dest }),
        }
        if protocol.borrowed_origin.is_some() && protocol.source_retained && source != dest {
            self.reestablish_source_loans(source, dest);
        }
    }

    /// Copy a normalized borrowed source's loans onto the long-lived iterator
    /// slot so the source dependency survives for the whole loop even though the
    /// retained-source slot's last read is the normalization itself. Shared by
    /// `for` statements and comprehension clauses.
    pub(super) fn reestablish_source_loans(&mut self, source: VarId, iterator: VarId) {
        let loans = self
            .aggregate_loans
            .get(&source)
            .cloned()
            .unwrap_or_default();
        self.establish_copied_loans(iterator, loans);
    }

    /// Copy only the interior-generation loans of an iterator object onto its
    /// `ref` loop binding (see the `BorrowReference` binding action).
    fn reestablish_interior_source_loans(&mut self, iterator: VarId, binding: VarId) {
        let loans = self
            .aggregate_loans
            .get(&iterator)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|loan| loan.interior.is_some())
            .collect();
        self.establish_copied_loans(binding, loans);
    }

    /// A loan-carrying value stored through a pointer (`q[] = Span(xs)`)
    /// lives in the pointee storage, which the pointer variable stands for:
    /// its loans join the pointer's own, so the borrowed owners outlive every
    /// later read through the pointer.
    fn retain_pointee_loans(&mut self, pointer: VarId, value: Node<'a>) {
        let stored = self.aggregate_borrows(value);
        if stored.is_empty() {
            return;
        }
        let mut loans = self
            .aggregate_loans
            .get(&pointer)
            .cloned()
            .unwrap_or_default();
        loans.extend(stored);
        self.establish_copied_loans(pointer, loans);
    }

    fn establish_copied_loans(&mut self, reference: VarId, loans: Vec<MirLoan>) {
        if let Some(first) = loans.first() {
            let marker = self.fresh_typed(
                SourceSpan::new(None, DUMMY_SPAN),
                Some(first.place.root),
                Ty::none(),
            );
            self.emit(MirInstr::EstablishLoans {
                reference,
                loans: loans.clone(),
                marker,
                dest_interior: None,
            });
            self.aggregate_loans.insert(reference, loans);
        }
    }

    /// Decompose a place expression (`x`, `p.a.b`, `xs[i]`, `p.items[i].x`) into a
    /// [`MirPlace`] — a root variable plus a projection chain — flattening any
    /// subscript index into a register **once**. The checker guarantees the root
    /// is a variable (or `self`), so a non-variable root is unreachable.
    pub(super) fn place(&mut self, e: Node<'a>) -> MirPlace {
        let place = self.place_unrebound(e);
        e.rebound_place(place)
    }

    fn place_unrebound(&mut self, e: Node<'a>) -> MirPlace {
        // A materialized borrow-source temporary's place is its hidden slot,
        // registered under the checker-minted owner when the argument lowered.
        if let Some(owner) = mojito_checked::checked::materialized_borrow_owner(e.adjustments())
            && let Some(var) = self.owner_vars.get(&owner).copied()
        {
            return MirPlace::root(var, self.var_types.get(&var).cloned());
        }
        if let Some(name) = self.name_spelled(e) {
            return self.name_place_root(&name, e);
        }
        // A call returning a mutable reference: its result is the place.
        if e.is_call()
            && let Some(place) = self.materialize_reference_result_place(e)
        {
            return place;
        }
        match (e.op(), e.object()) {
            (CheckedOp::Member { field, .. }, Some(object)) => {
                let mut p = self.place(object);
                e.project_member(&mut p, field);
                p
            }
            // `p[unsafe_offset=i]`: the pointer element place, as `p[i]`.
            (CheckedOp::MultiIndex { .. }, _)
                if let Some((object, offset)) = e.pointer_keyword_subscript() =>
            {
                let mut p = self.place(object);
                let idx = self.expr(offset);
                if let Some(ty) = e.place_ty().or_else(|| e.ty()) {
                    p.project(Proj::Index(idx), ty);
                } else {
                    p.proj.push(Proj::Index(idx));
                }
                p
            }
            (CheckedOp::Index { .. }, Some(object)) if let Some(index) = e.index() => {
                // A type-keyed projection (`v[Int]` on the self-hosted
                // `Variant`), or a `Tuple` element the checker resolved to
                // its compile-time-index accessor (`t[0]`, upstream's
                // `__getitem_param__` returning `ref` into storage): the
                // accessor call's reference result is the place.
                if (self.type_keyed_accessor_call(e).is_some()
                    || e.element_accessor_call().is_some())
                    && let Some(place) = self.materialize_reference_result_place(e)
                {
                    return place;
                }
                let mut p = self.place(object);
                // A checked Variant projection on any base place
                // (`storage[T]` on a field): the tagged payload place.
                if let Some(index) = e.variant_projection_index() {
                    let ty = e
                        .place_ty()
                        .or_else(|| e.ty())
                        .expect("checked Variant projection has a payload type");
                    p.project(Proj::Variant(index), ty);
                    return p;
                }
                // Compiler-private inline uninit storage: `storage[0]` is the
                // payload place (`Proj::UninitPayload`), not a dynamic index.
                let object_ty = object.ty();
                if let Some(element) = object_ty
                    .as_ref()
                    .and_then(mojito_types::types::uninit_storage_element)
                {
                    p.project(Proj::UninitPayload, element.clone());
                    return p;
                }
                // A literal index into compiler-private heterogeneous Tuple
                // storage is the element's static place, as in `try_place`.
                if let (Some(TyKind::Tuple(_)), Some(index)) = (
                    object_ty.as_ref().map(Ty::kind),
                    index.exact_nonnegative_index(),
                ) {
                    let element = e
                        .place_ty()
                        .or_else(|| e.ty())
                        .expect("checked Tuple storage element has a type");
                    p.project(Proj::ConstIndex(index), element);
                    return p;
                }
                let idx = self.expr(index); // evaluated once, before the store
                if let Some(ty) = e.place_ty().or_else(|| e.ty()) {
                    p.project(Proj::Index(idx), ty);
                } else {
                    p.proj.push(Proj::Index(idx));
                }
                p
            }
            (CheckedOp::TypeApply { name, .. }, _) => {
                let index = e
                    .variant_projection_index()
                    .expect("only checked Variant projection is a place TypeApply");
                let mut p = self.resolved_place(name);
                let ty = e
                    .place_ty()
                    .or_else(|| e.ty())
                    .expect("checked Variant projection has a payload type");
                p.project(Proj::Variant(index), ty);
                p
            }
            (other, _) => {
                // A borrowing view a call returns is a place only as the base
                // of a projection into its borrowed storage
                // (`make(b).src[] = Box(11)`).
                if let Some(place) = self.materialized_temporary_base_place(e) {
                    return place;
                }
                self.emit(MirInstr::Unsupported(format!(
                    "invalid assignment place reached MIR lowering: {other:?}"
                )));
                MirPlace::root(self.var("$invalid_place"), None)
            }
        }
    }

    /// Lower `place OP= rhs` on a user-defined value to its in-place dunder call
    /// (`counter += 2` → `counter.__iadd__(2)`), a `mut self` method that mutates
    /// the receiver in place. Returns `false` for native scalar targets, which
    /// keep the builtin `BinOp` read-modify-write. The selected contract rides the
    /// place node's `SelectedCall` adjustment, so this reuses the ordinary
    /// method-call machinery — `lower_call_receiver` commits the mutation through
    /// the receiver place (var slot, alias, pointer, or reference handle) exactly
    /// as any other `mut self` call.
    pub(super) fn lower_augmented_in_place(
        &mut self,
        place: Node<'a>,
        op: InfixOp,
        rhs_expression: Node<'a>,
    ) -> bool {
        let Some(contract) = place.augmented_in_place_contract() else {
            return false;
        };
        let receiver = self.lower_call_receiver(place);
        let operand = self.lower_in_place_operand(rhs_expression);
        self.emit_interior_invalidations(place, None);
        self.emit_in_place_dunder(place, receiver, &contract, op, operand);
        true
    }

    /// `receiver[index] = value` through a mutable-reference `__getitem__`
    /// when no `__setitem__` exists (the checker recorded an augmented
    /// reference write with no operator): the getter's handle is written
    /// directly, with no read-back or operator step.
    pub(super) fn lower_subscript_reference_set(
        &mut self,
        target: Node<'a>,
        rhs_expression: Node<'a>,
    ) -> bool {
        let Some(plan) = target.augmented_subscript() else {
            return false;
        };
        let Some((object, index)) = target
            .index_operands()
            .map(|(object, _)| object)
            .zip(target.index())
        else {
            return false;
        };
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let index_source = mojito_checked::checked::CheckedCallArgumentSource::Positional(0);
        let retain_index = Self::checked_call_source_requires_place(&plan.getter, index_source);
        let (raw_index, index_place) = self.lower_augmented_argument_source(index, retain_index);
        let getter_index = self.apply_checked_call_value_adjustments(
            &plan.getter,
            index_source,
            raw_index,
            index.location(),
        );
        let getter_call = self.mir_subscript_call_contract(
            plan.getter.clone(),
            &[MirSubscriptArg::Index(getter_index)],
            target.location(),
        );
        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let handle = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.record_call(handle, Some(getter_call.target.clone()), 0);
        self.emit(MirInstr::Index {
            dest: handle,
            base: receiver,
            index: getter_index,
            base_place: receiver_place.map(Box::new),
            index_place: (Self::checked_call_source_place(
                &plan.getter,
                index_source,
                &index_place,
            ))
            .map(Box::new),
            call: Some(Box::new(getter_call)),
            intrinsic: None,
        });
        let handle = self.peel_reference_handle_to(handle, &plan.operand_ty, &target.location());
        let rhs = self.expr(rhs_expression);
        self.yield_stored_value(rhs_expression, rhs);
        self.emit_interior_invalidations(target, None);
        self.emit(MirInstr::WriteRef {
            reference: handle,
            value: rhs,
        });
        true
    }

    /// Lower `receiver[arguments] OP= rhs` from the complete checked accessor
    /// contracts. A value getter evaluates raw receiver/index sources, then the
    /// RHS, then getter-specific adaptations and the getter; the in-place
    /// dunder updates the getter's result in its call-result slot, which the
    /// independently adapted setter takes (an owned value parameter) or reads
    /// (a `read` one). A mutable-reference getter instead establishes the
    /// lvalue before the RHS and calls the dunder on the referent through the
    /// getter's handle, as current Mojo does; a native scalar element reads the
    /// handle and finishes with `WriteRef`. In both paths each source
    /// expression and slice bound is evaluated once.
    pub(super) fn lower_augmented_subscript(
        &mut self,
        target: Node<'a>,
        op: InfixOp,
        rhs_expression: Node<'a>,
    ) -> bool {
        let Some(plan) = target.augmented_subscript() else {
            return false;
        };
        let (descriptors, value_keyword) = target
            .adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    set_value_keyword,
                } => Some((descriptors.clone(), set_value_keyword.clone())),
                _ => None,
            })
            .expect("checked augmented subscript has descriptor metadata");

        let store = AugmentedStore {
            target,
            plan,
            value_keyword,
            op,
            rhs: rhs_expression,
        };
        match target.op() {
            CheckedOp::Index { .. }
                if let Some((object, index)) = target.object().zip(target.index()) =>
            {
                debug_assert_eq!(descriptors, vec![None]);
                self.lower_augmented_index(&store, object, index);
            }
            CheckedOp::Slice {
                lower, upper, step, ..
            } if let Some(object) = target.object() => {
                let descriptor = descriptors.first().cloned().flatten();
                self.lower_augmented_slice(&store, object, [*lower, *upper, *step], descriptor);
            }
            CheckedOp::MultiIndex { args: source, .. } if let Some(object) = target.object() => {
                self.lower_augmented_multi_index(&store, object, source, &descriptors);
            }
            _ => return false,
        }
        true
    }

    /// [`Self::lower_augmented_subscript`] for one index, `object[index]`.
    fn lower_augmented_index(
        &mut self,
        store: &AugmentedStore<'a>,
        object: Node<'a>,
        index: Node<'a>,
    ) {
        let AugmentedStore {
            target,
            ref plan,
            ref value_keyword,
            op,
            rhs: rhs_expression,
        } = *store;
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let index_source = mojito_checked::checked::CheckedCallArgumentSource::Positional(0);
        let retain_index = Self::checked_call_source_requires_place(&plan.getter, index_source)
            || plan.setter.as_ref().is_some_and(|setter| {
                Self::checked_call_source_requires_place(setter, index_source)
            });
        let (raw_index, index_place) = self.lower_augmented_argument_source(index, retain_index);

        if plan.setter.is_none() {
            let getter_index = self.apply_checked_call_value_adjustments(
                &plan.getter,
                index_source,
                raw_index,
                index.location(),
            );
            let getter_call = self.mir_subscript_call_contract(
                plan.getter.clone(),
                &[MirSubscriptArg::Index(getter_index)],
                target.location(),
            );
            self.emit_checked_call_boundary(&plan.getter, &target.location());
            let handle = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
            self.record_call(handle, Some(getter_call.target.clone()), 0);
            self.emit(MirInstr::Index {
                dest: handle,
                base: receiver,
                index: getter_index,
                base_place: receiver_place.map(Box::new),
                index_place: (Self::checked_call_source_place(
                    &plan.getter,
                    index_source,
                    &index_place,
                ))
                .map(Box::new),
                call: Some(Box::new(getter_call)),
                intrinsic: None,
            });
            self.finish_reference_augmented_store(target, handle, plan, op, rhs_expression);
            return;
        }

        // Value-getter ordering is raw receiver/index, RHS, accessor
        // adaptation/getter, operator, setter adaptation/setter.
        let rhs = self.lower_augmented_rhs(rhs_expression, plan.inplace.is_some());
        let getter_index = self.apply_checked_call_value_adjustments(
            &plan.getter,
            index_source,
            raw_index,
            index.location(),
        );
        let getter_call = self.mir_subscript_call_contract(
            plan.getter.clone(),
            &[MirSubscriptArg::Index(getter_index)],
            target.location(),
        );
        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let current = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.record_call(current, Some(getter_call.target.clone()), 0);
        self.emit(MirInstr::Index {
            dest: current,
            base: receiver,
            index: getter_index,
            base_place: receiver_place.clone().map(Box::new),
            index_place: (Self::checked_call_source_place(
                &plan.getter,
                index_source,
                &index_place,
            ))
            .map(Box::new),
            call: Some(Box::new(getter_call)),
            intrinsic: None,
        });
        let result = self.apply_augmented_operator(target, current, plan, op, rhs);

        let setter = plan
            .setter
            .as_ref()
            .expect("value-returning augmented getter has a setter");
        let setter_receiver =
            self.reload_augmented_receiver(receiver, receiver_place.as_ref(), plan, object);
        let setter_raw_index = self.reload_augmented_source(
            raw_index,
            &index_place,
            Self::checked_call_source_mutates(&plan.getter, index_source),
            index.location(),
        );
        let setter_index = self.apply_checked_call_value_adjustments(
            setter,
            index_source,
            setter_raw_index,
            index.location(),
        );
        let value_argument = mojito_checked::checked::CheckedCallArgumentSource::Keyword(0);
        let (value, value_place) =
            self.augmented_setter_value(target, setter, value_argument, result);
        let setter_call = self.mir_subscript_call_contract(
            setter.clone(),
            &[MirSubscriptArg::Index(setter_index)],
            target.location(),
        );
        self.emit_checked_call_boundary(setter, &target.location());
        self.emit(MirInstr::MultiSet {
            receiver: setter_receiver,
            receiver_place: receiver_place.map(Box::new),
            args: vec![MirSubscriptArg::Index(setter_index)],
            arg_places: vec![Self::checked_call_source_place(
                setter,
                index_source,
                &index_place,
            )],
            value,
            value_place: value_place.map(Box::new),
            value_keyword: value_keyword
                .clone()
                .expect("a store through a setter names its value"),
            call: Box::new(setter_call),
        });
    }

    /// [`Self::lower_augmented_subscript`] for one slice, `object[a:b:c]`.
    fn lower_augmented_slice(
        &mut self,
        store: &AugmentedStore<'a>,
        object: Node<'a>,
        bounds: [Option<mojito_checked::checked::CheckedNodeId>; 3],
        descriptor: Option<mojito_checked::checked::CheckedSliceLiteral>,
    ) {
        let AugmentedStore {
            target,
            ref plan,
            ref value_keyword,
            op,
            rhs: rhs_expression,
        } = *store;
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let (argument, argument_place) = self.slice_set_argument(target, descriptor, bounds);
        let getter_call =
            self.mir_subscript_call_contract(plan.getter.clone(), &[], target.location());
        if plan.setter.is_none() {
            self.emit_checked_call_boundary(&plan.getter, &target.location());
            let handle = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
            self.emit_slice_read(
                handle,
                receiver,
                (argument, argument_place),
                receiver_place,
                Some(getter_call),
            );
            self.finish_reference_augmented_store(target, handle, plan, op, rhs_expression);
            return;
        }

        let rhs = self.lower_augmented_rhs(rhs_expression, plan.inplace.is_some());
        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let current = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.emit_slice_read(
            current,
            receiver,
            (argument.clone(), argument_place.clone()),
            receiver_place.clone(),
            Some(getter_call),
        );
        let result = self.apply_augmented_operator(target, current, plan, op, rhs);
        let setter = plan
            .setter
            .as_ref()
            .expect("value-returning augmented getter has a setter");
        let setter_receiver =
            self.reload_augmented_receiver(receiver, receiver_place.as_ref(), plan, object);
        let value_argument = mojito_checked::checked::CheckedCallArgumentSource::Keyword(0);
        let (value, value_place) =
            self.augmented_setter_value(target, setter, value_argument, result);
        let setter_call = self.mir_subscript_call_contract(setter.clone(), &[], target.location());
        self.emit_checked_call_boundary(setter, &target.location());
        self.emit(MirInstr::MultiSet {
            receiver: setter_receiver,
            receiver_place: receiver_place.map(Box::new),
            args: vec![argument],
            arg_places: vec![argument_place],
            value,
            value_place: value_place.map(Box::new),
            value_keyword: value_keyword
                .clone()
                .expect("a store through a setter names its value"),
            call: Box::new(setter_call),
        });
    }

    /// [`Self::lower_augmented_subscript`] for a multi-argument bracket,
    /// `object[i, j]` or `object[i, a:b]`.
    fn lower_augmented_multi_index(
        &mut self,
        store: &AugmentedStore<'a>,
        object: Node<'a>,
        source: &[mojito_checked::checked::CheckedSubscriptArg],
        descriptors: &[Option<mojito_checked::checked::CheckedSliceLiteral>],
    ) {
        let AugmentedStore {
            target,
            ref plan,
            ref value_keyword,
            op,
            rhs: rhs_expression,
        } = *store;
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let mut source_places = Vec::with_capacity(source.len());
        let mut source_spans = Vec::with_capacity(source.len());
        let raw_args = source
            .iter()
            .zip(descriptors)
            .enumerate()
            .map(|(position, (argument, descriptor))| match argument {
                mojito_checked::checked::CheckedSubscriptArg::Index(index) => {
                    debug_assert!(descriptor.is_none());
                    let index = Self::subscript_operand(target, *index);
                    let argument_source =
                        mojito_checked::checked::CheckedCallArgumentSource::Positional(position);
                    let retain_place =
                        Self::checked_call_source_requires_place(&plan.getter, argument_source)
                            || plan.setter.as_ref().is_some_and(|setter| {
                                Self::checked_call_source_requires_place(setter, argument_source)
                            });
                    let (register, place) =
                        self.lower_augmented_argument_source(index, retain_place);
                    source_places.push(place);
                    source_spans.push(Some(index.location()));
                    MirSubscriptArg::Index(register)
                }
                mojito_checked::checked::CheckedSubscriptArg::Slice {
                    lower, upper, step, ..
                } => {
                    let (argument, place) = self.slice_set_argument(
                        target,
                        descriptor.clone(),
                        [*lower, *upper, *step],
                    );
                    source_places.push(place);
                    source_spans.push(None);
                    argument
                }
                _ => {
                    unreachable!("keyword subscript assignment is rejected at checking")
                }
            })
            .collect::<Vec<_>>();
        // Value getters defer every call-local argument adaptation
        // until after the RHS. Building the raw descriptor/index list
        // above has already performed each source evaluation once.
        let inplace = plan.inplace.is_some();
        let value_rhs = plan
            .setter
            .as_ref()
            .map(|_| self.lower_augmented_rhs(rhs_expression, inplace));
        let getter_args = raw_args
            .iter()
            .enumerate()
            .map(|(position, argument)| match argument {
                MirSubscriptArg::Index(register) => MirSubscriptArg::Index(
                    self.apply_checked_call_value_adjustments(
                        &plan.getter,
                        mojito_checked::checked::CheckedCallArgumentSource::Positional(position),
                        *register,
                        source_spans[position]
                            .clone()
                            .unwrap_or_else(|| target.location()),
                    ),
                ),
            })
            .collect::<Vec<_>>();
        let getter_places = source_places
            .iter()
            .enumerate()
            .map(|(position, place)| {
                Self::checked_call_source_place(
                    &plan.getter,
                    mojito_checked::checked::CheckedCallArgumentSource::Positional(position),
                    place,
                )
            })
            .collect::<Vec<_>>();
        let getter_call =
            self.mir_subscript_call_contract(plan.getter.clone(), &getter_args, target.location());

        let Some(rhs) = value_rhs else {
            self.emit_checked_call_boundary(&plan.getter, &target.location());
            let handle = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
            self.record_call(handle, Some(getter_call.target.clone()), 0);
            self.emit(MirInstr::MultiIndex {
                dest: handle,
                object: receiver,
                args: getter_args,
                object_place: receiver_place.map(Box::new),
                arg_places: getter_places,
                kwargs: Vec::new(),
                kwarg_places: Vec::new(),
                call: Some(Box::new(getter_call)),
            });
            self.finish_reference_augmented_store(target, handle, plan, op, rhs_expression);
            return;
        };

        self.emit_checked_call_boundary(&plan.getter, &target.location());
        let current = self.fresh_typed(target.location(), None, plan.getter.result_ty.clone());
        self.record_call(current, Some(getter_call.target.clone()), 0);
        self.emit(MirInstr::MultiIndex {
            dest: current,
            object: receiver,
            args: getter_args,
            object_place: receiver_place.clone().map(Box::new),
            arg_places: getter_places,
            kwargs: Vec::new(),
            kwarg_places: Vec::new(),
            call: Some(Box::new(getter_call)),
        });
        let result = self.apply_augmented_operator(target, current, plan, op, rhs);
        let setter = plan
            .setter
            .as_ref()
            .expect("value-returning augmented getter has a setter");
        let setter_receiver =
            self.reload_augmented_receiver(receiver, receiver_place.as_ref(), plan, object);
        let setter_args = raw_args
            .iter()
            .enumerate()
            .map(|(position, argument)| match argument {
                MirSubscriptArg::Index(register) => {
                    let source_kind =
                        mojito_checked::checked::CheckedCallArgumentSource::Positional(position);
                    let raw = self.reload_augmented_source(
                        *register,
                        &source_places[position],
                        Self::checked_call_source_mutates(&plan.getter, source_kind),
                        source_spans[position]
                            .clone()
                            .unwrap_or_else(|| target.location()),
                    );
                    MirSubscriptArg::Index(
                        self.apply_checked_call_value_adjustments(
                            setter,
                            source_kind,
                            raw,
                            source_spans[position]
                                .clone()
                                .unwrap_or_else(|| target.location()),
                        ),
                    )
                }
            })
            .collect::<Vec<_>>();
        let setter_places = source_places
            .iter()
            .enumerate()
            .map(|(position, place)| {
                Self::checked_call_source_place(
                    setter,
                    mojito_checked::checked::CheckedCallArgumentSource::Positional(position),
                    place,
                )
            })
            .collect::<Vec<_>>();
        let value_argument = mojito_checked::checked::CheckedCallArgumentSource::Keyword(0);
        let (value, value_place) =
            self.augmented_setter_value(target, setter, value_argument, result);
        let setter_call =
            self.mir_subscript_call_contract(setter.clone(), &setter_args, target.location());
        self.emit_checked_call_boundary(setter, &target.location());
        self.emit(MirInstr::MultiSet {
            receiver: setter_receiver,
            receiver_place: receiver_place.map(Box::new),
            args: setter_args,
            arg_places: setter_places,
            value,
            value_place: value_place.map(Box::new),
            value_keyword: value_keyword
                .clone()
                .expect("a store through a setter names its value"),
            call: Box::new(setter_call),
        });
    }

    /// An augmented store's right-hand side: read by its in-place dunder's
    /// convention, its place retained, when `inplace`; otherwise the native
    /// operator's value operand.
    fn lower_augmented_rhs(
        &mut self,
        rhs_expression: Node<'a>,
        inplace: bool,
    ) -> AugmentedOperand<'a> {
        if inplace {
            return self.lower_in_place_operand(rhs_expression);
        }
        AugmentedOperand {
            expression: rhs_expression,
            args: vec![self.expr(rhs_expression)],
            places: vec![None],
        }
    }

    /// The right-hand side of `place OP= rhs` lowered as its in-place dunder's
    /// argument. A view temporary operand (`s += t.rstrip()`) anchors its
    /// borrowed source through the call, as an ordinary method argument does.
    fn lower_in_place_operand(&mut self, rhs_expression: Node<'a>) -> AugmentedOperand<'a> {
        let saved_anchor_permission = std::mem::replace(&mut self.allow_argument_anchors, true);
        let (args, places) = self.lower_call_arguments(&[rhs_expression], false);
        self.allow_argument_anchors = saved_anchor_permission;
        AugmentedOperand {
            expression: rhs_expression,
            args,
            places,
        }
    }

    /// Call the in-place dunder `contract` selected for `op` at `site` on
    /// `receiver`, whose place the `mut self` call writes through.
    fn emit_in_place_dunder(
        &mut self,
        site: Node<'a>,
        (recv, recv_place): (Reg, Option<MirPlace>),
        contract: &mojito_checked::checked::CheckedCallContract,
        op: InfixOp,
        operand: AugmentedOperand<'a>,
    ) {
        // A dunder dispatched through a bound names no declaration to read
        // the result from; the checker proved it returns `None`.
        let dest = self.fresh_typed(site.location(), None, contract.result_ty.clone());
        self.emit_checked_call_boundary(contract, &site.location());
        let method = op
            .inplace_dunder()
            .expect("augmented in-place operator has an in-place dunder")
            .to_string();
        let raises = self.raises(contract.raises.clone(), site.location());
        self.emit_call(MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved: Some(contract.target.clone()),
            raises: raises.clone(),
            reference_result: contract.reference_result.clone().map(Box::new),
            result_adapter: contract.result_adapter.map(MirResultAdapter::new),
            args: operand.args,
            kwargs: Vec::new(),
            recv_place: recv_place.map(Box::new),
            recv_writes: true,
            receiver_convention: contract.receiver_convention,
            arg_places: operand.places,
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: contract.param_decls.clone(),
            instantiated_args: Vec::new(),
            spread: None,
            refined_bound: contract.refined_bound.clone().map(MirRefinedBound::boxed),
        });
        self.emit_nested_closure_argument_keepalives(&[operand.expression], &[]);
        self.error_region(raises);
    }

    /// Finish an augmented store through a mutable-reference getter's
    /// `handle`: an element's in-place dunder runs on the referent through
    /// the handle, with the RHS lowered after the lvalue; a native scalar
    /// element is read through the handle, combined, and written back.
    fn finish_reference_augmented_store(
        &mut self,
        target: Node<'a>,
        handle: Reg,
        plan: &mojito_checked::checked::CheckedAugmentedSubscript,
        op: InfixOp,
        rhs_expression: Node<'a>,
    ) {
        let handle = self.peel_reference_handle_to(handle, &plan.operand_ty, &target.location());
        let reference = match self.f.reg_types.get(&handle.0).map(Ty::kind) {
            Some(TyKind::Ref(reference)) => Some(reference.clone()),
            _ => plan.getter.reference_result.clone(),
        };
        if let Some(inplace) = &plan.inplace
            && let Some(reference) = reference
        {
            let place = self.materialize_call_reference_place(
                target,
                handle,
                &reference,
                None,
                (false, true),
            );
            let recv = self.fresh_typed(
                target.location(),
                Some(place.root),
                place.ty.clone().unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: recv,
                place: place.clone(),
            });
            let operand = self.lower_in_place_operand(rhs_expression);
            self.emit_interior_invalidations(target, None);
            self.emit_in_place_dunder(target, (recv, Some(place)), inplace, op, operand);
            return;
        }
        let rhs = self.expr(rhs_expression);
        let current = self.fresh_typed(target.location(), None, plan.operand_ty.clone());
        self.emit(MirInstr::ReadRef {
            dest: current,
            reference: handle,
        });
        let result = self.fresh_typed(target.location(), None, plan.result_ty.clone());
        self.emit(MirInstr::BinOp {
            op,
            dest: result,
            a: current,
            b: rhs,
            resolved: None,
            raises: None,
            refined_bound: None,
        });
        self.emit_interior_invalidations(target, None);
        self.emit(MirInstr::WriteRef {
            reference: handle,
            value: result,
        });
    }

    /// Apply `op` to a value getter's result `current`. An element's in-place
    /// dunder updates it in its call-result slot, which the setter then takes
    /// or reads ([`Self::augmented_setter_value`]); a native scalar element's
    /// operator yields a fresh value.
    fn apply_augmented_operator(
        &mut self,
        target: Node<'a>,
        current: Reg,
        plan: &mojito_checked::checked::CheckedAugmentedSubscript,
        op: InfixOp,
        rhs: AugmentedOperand<'a>,
    ) -> CValue {
        let Some(inplace) = &plan.inplace else {
            let result = self.fresh_typed(target.location(), None, plan.result_ty.clone());
            self.emit(MirInstr::BinOp {
                op,
                dest: result,
                a: current,
                b: rhs.args[0],
                resolved: None,
                raises: None,
                refined_bound: None,
            });
            return CValue::Value(result);
        };
        let slot = self.var(&format!("{}{}", crate::mir::CALL_RESULT_SLOT, current.0));
        self.var_types.insert(slot, plan.operand_ty.clone());
        self.emit(MirInstr::DefVar {
            var: slot,
            src: current,
            binding_ty: Some(plan.operand_ty.clone()),
            in_place: false,
        });
        let place = MirPlace::root(slot, Some(plan.operand_ty.clone()));
        let recv = self.fresh_typed(target.location(), Some(slot), plan.operand_ty.clone());
        self.emit(MirInstr::LoadPlace {
            dest: recv,
            place: place.clone(),
        });
        self.emit_in_place_dunder(target, (recv, Some(place)), inplace, op, rhs);
        CValue::Slot(slot)
    }

    /// The augmented store's updated `result` passed to `setter`'s value
    /// parameter by its convention: taken by an owned (`var`/`deinit`)
    /// parameter or one the checker adapts, and read in place, with its
    /// place, by a `read` one.
    fn augmented_setter_value(
        &mut self,
        target: Node<'a>,
        setter: &mojito_checked::checked::CheckedCallContract,
        value_argument: mojito_checked::checked::CheckedCallArgumentSource,
        result: CValue,
    ) -> (Reg, Option<MirPlace>) {
        let adapted =
            setter.boundary.arguments.iter().any(|argument| {
                argument.source == value_argument && !argument.adjustments.is_empty()
            });
        let owned = setter.arguments.iter().any(|argument| {
            argument.source == value_argument
                && matches!(
                    argument.convention,
                    Some(
                        mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                )
        });
        if !adapted && !owned {
            return self.read_c(target, result);
        }
        let value = self.take_c(target, result);
        let value = self.apply_checked_call_value_adjustments(
            setter,
            value_argument,
            value,
            target.location(),
        );
        (value, None)
    }

    /// The augmented store's receiver as its setter sees it: reloaded from
    /// its place when the getter's `mut`/`ref` receiver may have rebound it.
    fn reload_augmented_receiver(
        &mut self,
        receiver: Reg,
        receiver_place: Option<&MirPlace>,
        plan: &mojito_checked::checked::CheckedAugmentedSubscript,
        object: Node<'a>,
    ) -> Reg {
        self.reload_augmented_source(
            receiver,
            &receiver_place.cloned(),
            matches!(
                plan.getter.receiver_convention,
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
            ),
            object.location(),
        )
    }

    /// Lower the store `place = value`, a statement's or a walrus's: a
    /// nominal subscript through its getter's reference or its setter, a
    /// pointer through its address, any other place by a store.
    pub(in crate::mir) fn lower_set_place(&mut self, place: Node<'a>, value: Node<'a>) {
        if self.lower_subscript_reference_set(place, value) {
            return;
        }
        if self.lower_subscript_set(place, value) {
            return;
        }
        let (src, _) = self.lower_assignment_value(place, value);
        self.flush_argument_anchors();
        // A store through an origin-bearing pointer writes its
        // address; the checker fixed the offset to 0 and required
        // mutable provenance. A stably bound pointer substitutes its
        // statically known address; otherwise the store goes through the handle —
        // loaded before the invalidation boundary, so a first write
        // through a subtree pointer uses its still-live generation and
        // only later uses observe the self-invalidation.
        if let Some((object, _)) = place.pointer_offset_subscript() {
            if let Some(target) = self.pointer_deref_place(object) {
                self.emit_interior_invalidations(place, None);
                self.emit(MirInstr::Store { place: target, src });
                return;
            }
            if object.is_origin_bearing_pointer() {
                let reference = self.expr(object);
                self.emit_interior_invalidations(place, None);
                self.emit(MirInstr::WriteRef {
                    reference,
                    value: src,
                });
                return;
            }
        }
        self.emit_interior_invalidations(place, None);
        // A projection below a nominal reference-returning accessor
        // (`xs[i].field`) is rooted in that accessor's materialized
        // handle, never in raw nominal storage.
        let p = self
            .lower_projected_reference_place(place)
            .unwrap_or_else(|| self.place(place));
        let stores_reference = matches!(p.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
            && value.adjustments().iter().any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::BorrowShared
                        | mojito_checked::checked::SemanticAdjustment::BorrowMutable
                )
            });
        if stores_reference {
            self.emit(MirInstr::StoreRef {
                place: p,
                reference: src,
            });
        } else {
            // A checked Copyable write-through runs the referent's
            // copy lifecycle before the store, so the written value
            // owns its storage instead of sharing the source's.
            let src = if matches!(p.ty.as_ref().map(Ty::kind), Some(TyKind::Ref(_)))
                && value.adjustments().iter().any(|adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::CopyPlaceValue
                    )
                }) {
                let copied = self.fresh(place.location(), None);
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value: src,
                    constructor: None,
                });
                copied
            } else {
                src
            };
            let pointee_root =
                (!p.is_whole_root() && self.pointer_valued_slot(p.root)).then_some(p.root);
            self.emit(MirInstr::Store { place: p, src });
            if let Some(pointer) = pointee_root {
                self.retain_pointee_loans(pointer, value);
            }
        }
    }

    /// Lower a nominal subscript assignment through the checker-selected
    /// `__setitem__` implementation. Unlike an ordinary `MirPlace`
    /// projection, every slice remains a first-class descriptor argument and
    /// the receiver place is retained for `mut self` write-back.
    pub(super) fn lower_subscript_set(
        &mut self,
        target: Node<'a>,
        value_expression: Node<'a>,
    ) -> bool {
        if target.call_contract().is_none() {
            return false;
        }
        let Some((setter, (value, value_place))) = self.lower_subscript_setter(target, |this| {
            this.outside_call_setup(|this| {
                let value = this.lower_assignment_value(target, value_expression);
                this.converted_store_value(target, value)
            })
        }) else {
            if matches!(
                target.op(),
                CheckedOp::Slice { .. } | CheckedOp::MultiIndex { .. }
            ) {
                self.emit(MirInstr::Unsupported(
                    "checked subscript assignment lacks descriptor metadata".to_string(),
                ));
                return true;
            }
            return false;
        };
        self.emit_subscript_set(setter, value, value_place, Some(value_expression));
        true
    }

    /// A plain store's lowered right-hand side `value` as its setter takes
    /// it. One the checker converted to the element type is not the setter's
    /// argument itself (its value argument is keyed at the store's own site):
    /// the setter's adaptations of the converted value apply here, as they
    /// do to an augmented store's computed value.
    fn converted_store_value(
        &mut self,
        target: Node<'a>,
        (value, value_place): (Reg, Option<MirPlace>),
    ) -> (Reg, Option<MirPlace>) {
        let source = mojito_checked::checked::CheckedCallArgumentSource::Keyword(0);
        let Some(setter) = target.call_contract().filter(|setter| {
            setter.boundary.arguments.iter().any(|argument| {
                argument.source == source && argument.value_source == target.location()
            })
        }) else {
            return (value, value_place);
        };
        let converted = match value_place {
            Some(_) => self.take_c(target, CValue::Value(value)),
            None => value,
        };
        self.augmented_setter_value(target, &setter, source, CValue::Value(converted))
    }

    pub(super) fn lower_assignment_value(
        &mut self,
        target: Node<'a>,
        value_expression: Node<'a>,
    ) -> (Reg, Option<MirPlace>) {
        let (mut value, value_place) = self.lower_call_argument(value_expression);
        if let Some(target_ty) = target.ty() {
            value = self.materialize_register(value, &target_ty, value_expression.location());
        }
        self.yield_stored_value(value_expression, value);
        (value, value_place)
    }

    /// Lower the walrus `target := value` over a place: the store
    /// `target = value`, yielding a copy of the stored value taken before
    /// the store consumes it, as upstream's `emitWalrus` yields its
    /// right-hand side.
    pub(in crate::mir) fn lower_walrus_store(
        &mut self,
        walrus: Node<'a>,
        target: Node<'a>,
        value: Node<'a>,
    ) -> Reg {
        let enclosing = self.walrus_yield.replace(WalrusYield {
            value: value.id(),
            ty: walrus.ty(),
            copy: None,
        });
        self.lower_set_place(target, value);
        let yielded =
            std::mem::replace(&mut self.walrus_yield, enclosing).and_then(|walrus| walrus.copy);
        yielded.unwrap_or_else(|| {
            self.unsupported_value(
                target.location(),
                "a walrus over a place its store lowers without a value",
            )
        })
    }

    /// Record a copy of the lowered stored value `value` when
    /// `value_expression` is the walrus's being stored.
    fn yield_stored_value(&mut self, value_expression: Node<'a>, value: Reg) {
        let Some(ty) = self
            .walrus_yield
            .as_ref()
            .filter(|walrus| walrus.value == value_expression.id() && walrus.copy.is_none())
            .map(|walrus| walrus.ty.clone())
        else {
            return;
        };
        let copied = match ty {
            Some(ty) => self.fresh_typed(value_expression.location(), None, ty),
            None => self.fresh(value_expression.location(), None),
        };
        self.emit(MirInstr::CopyValue {
            dest: copied,
            value,
            constructor: None,
        });
        if let Some(walrus) = &mut self.walrus_yield {
            walrus.copy = Some(copied);
        }
    }

    /// Lower the receiver and index arguments of the nominal subscript
    /// store `target` and select its setter, running `between` before the
    /// setter's setup reads: current Mojo evaluates the receiver first, then
    /// indices and bounds from left to right, and a place index is read at
    /// the setter's setup, after its sibling operands. `None` when `target`
    /// stores through no checked setter.
    fn lower_subscript_setter<R>(
        &mut self,
        target: Node<'a>,
        between: impl FnOnce(&mut Self) -> R,
    ) -> Option<(SubscriptSetter<'a>, R)> {
        let (descriptors, value_keyword) =
            target
                .adjustments()
                .iter()
                .find_map(|adjustment| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::SliceDescriptors {
                        descriptors,
                        set_value_keyword: Some(value_keyword),
                    } => Some((descriptors.clone(), value_keyword.clone())),
                    _ => None,
                })?;
        let (object, source_arguments) = match target.op() {
            CheckedOp::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => (
                Self::subscript_operand(target, *object),
                SetArguments::Slice([*lower, *upper, *step]),
            ),
            CheckedOp::MultiIndex { object, args } => (
                Self::subscript_operand(target, *object),
                SetArguments::Multi(args),
            ),
            _ if descriptors.as_slice() == [None] => {
                let (object, index) = target
                    .index_operands()
                    .map(|(object, _)| object)
                    .zip(target.index())?;
                (object, SetArguments::Index(index))
            }
            _ => return None,
        };
        let (receiver, receiver_place) =
            self.lower_setup_receiver_with(target, object, |this| this.lower_call_receiver(object));
        let mut indices = Vec::new();
        let mut arg_places = Vec::with_capacity(descriptors.len());
        let args = match source_arguments {
            SetArguments::Index(index) => {
                let (register, place) = self.lower_call_argument(index);
                arg_places.push(place);
                indices.push(index);
                vec![MirSubscriptArg::Index(register)]
            }
            SetArguments::Multi(arguments) => arguments
                .iter()
                .zip(descriptors)
                .map(|(argument, descriptor)| match argument {
                    mojito_checked::checked::CheckedSubscriptArg::Index(value) => {
                        debug_assert!(descriptor.is_none());
                        let index = Self::subscript_operand(target, *value);
                        let (register, place) = self.lower_call_argument(index);
                        arg_places.push(place);
                        indices.push(index);
                        MirSubscriptArg::Index(register)
                    }
                    mojito_checked::checked::CheckedSubscriptArg::Slice {
                        lower,
                        upper,
                        step,
                        ..
                    } => {
                        let (argument, place) =
                            self.slice_set_argument(target, descriptor, [*lower, *upper, *step]);
                        arg_places.push(place);
                        argument
                    }
                    _ => unreachable!("keyword subscript assignment is rejected at checking"),
                })
                .collect(),
            SetArguments::Slice([lower, upper, step]) => {
                let (argument, place) = self.slice_set_argument(
                    target,
                    descriptors.first().cloned().flatten(),
                    [lower, upper, step],
                );
                arg_places.push(place);
                vec![argument]
            }
        };
        let between = between(self);
        self.close_call_setup(None);
        let call = self
            .subscript_call_contract(target, &args)
            .expect("checked nominal subscript setter has a call contract");
        Some((
            SubscriptSetter {
                target,
                indices,
                receiver,
                receiver_place,
                args,
                arg_places,
                value_keyword,
                call,
            },
            between,
        ))
    }

    /// Emit `setter`'s call storing `value` (read in `value_place` when it
    /// lies there), after the writes it makes stale: its index arguments',
    /// the store's `value_expression`'s when it has one, and the target's.
    fn emit_subscript_set(
        &mut self,
        setter: SubscriptSetter<'a>,
        value: Reg,
        value_place: Option<MirPlace>,
        value_expression: Option<Node<'a>>,
    ) {
        for index in setter.indices {
            self.emit_interior_invalidations(index, None);
        }
        if let Some(value_expression) = value_expression {
            self.emit_interior_invalidations(value_expression, None);
        }
        self.emit_interior_invalidations(setter.target, None);
        self.emit(MirInstr::MultiSet {
            receiver: setter.receiver,
            receiver_place: setter.receiver_place.map(Box::new),
            args: setter.args,
            arg_places: setter.arg_places,
            value,
            value_place: value_place.map(Box::new),
            value_keyword: setter.value_keyword,
            call: Box::new(setter.call),
        });
    }

    /// The handle of subscript `subscript`'s operand node `id`.
    fn subscript_operand(
        subscript: Node<'a>,
        id: mojito_checked::checked::CheckedNodeId,
    ) -> Node<'a> {
        subscript
            .operand_of(id)
            .expect("a subscript argument's node is its operand")
    }

    /// The subscript argument the slice literal `descriptor` of subscript
    /// `subscript` passes, whose operand nodes are `operands`, evaluated
    /// outside the subscript call's setup, with the place it is read from
    /// ([`Self::slice_literal_argument`]).
    fn slice_set_argument(
        &mut self,
        subscript: Node<'a>,
        descriptor: Option<mojito_checked::checked::CheckedSliceLiteral>,
        operands: [Option<mojito_checked::checked::CheckedNodeId>; 3],
    ) -> (MirSubscriptArg, Option<MirPlace>) {
        let descriptor = descriptor.expect("a slice argument has a bound literal");
        let operands = operands.map(|id| id.map(|id| self.slice_bound(subscript, id)));
        self.outside_call_setup(|this| {
            this.slice_literal_argument(subscript, &descriptor, operands)
        })
    }

    /// The value of slice bound node `id` of subscript `subscript`.
    fn slice_bound(
        &mut self,
        subscript: Node<'a>,
        id: mojito_checked::checked::CheckedNodeId,
    ) -> Reg {
        self.outside_call_setup(|this| this.expr(Self::subscript_operand(subscript, id)))
    }

    /// Like [`place`](Self::place), but returns `None` for a non-place expression
    /// (a call result, a literal, …) instead of panicking — used at a method-call
    /// receiver, which may be a temporary. Only evaluates subscript indices when
    /// the whole chain is a place.
    /// Lower a `try`'s parts and emit the [`MirInstr::Try`], labelled
    /// with the function's next `try` number as `LowerSemanticCF` labels
    /// it, after the declaration of its error slot, as `parseTryStmt`
    /// declares `__try_error__` before the `lit.try` taking it: the
    /// `except` binding's slot when the clause binds one, which upstream
    /// renames the declaration to. The body lowers under that label, so a
    /// raise in it stores into the slot and names this `try`; the handler,
    /// `else`, and `finally` lower in the enclosing context. The first
    /// raise types the slot, or it is an `Error`. A `finally` is copied
    /// before each exit of the parts ([`Self::lower_try_finally`]), after
    /// a raising body without an `except` gets the handler that re-raises
    /// its error.
    fn emit_try(
        &mut self,
        body: &'a hir::HirRegion,
        except: Option<&'a hir::HirExcept>,
        orelse: Option<&'a hir::HirRegion>,
        finalbody: Option<&'a hir::HirRegion>,
    ) {
        let label = TryLabel(self.next_try);
        self.next_try += 1;
        let error = self.try_error_slot(label, except.and_then(|except| except.slot));
        if let Some((binding, slot)) = except.and_then(|except| except.binding.zip(except.slot)) {
            self.owner_vars.insert(binding, slot);
        }
        let enclosing = self.raise_target.replace((label, error));
        let mut body = self.try_part(body);
        self.raise_target = enclosing;
        self.var_types.entry(error).or_insert_with(Ty::error);
        let mut handler = except.map(|except| self.try_part(&except.body));
        let mut orelse = orelse.map(|part| self.try_part(part));
        if let Some(finalbody) = finalbody {
            let finalbody = self.try_part(finalbody);
            if handler.is_none() && raises_to(&body, label) {
                handler = Some(self.reraising_handler(error, &body));
            }
            let orelse = orelse.get_or_insert_with(|| {
                vec![MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::TryYield(Vec::new()),
                }]
            });
            self.lower_try_finally(
                label,
                &finalbody,
                TryParts {
                    body: &mut body,
                    handler: handler.as_mut(),
                    orelse,
                },
            );
        }
        self.emit(MirInstr::Try {
            label,
            error: Some(error),
            results: Vec::new(),
            body,
            handler_args: Vec::new(),
            handler,
            else_args: Vec::new(),
            orelse,
        });
    }

    /// Lower `try` part `part` to its own block list. What the part's
    /// statements record about aliases, loans, displays, packs, and call
    /// setup goes out of scope with it.
    fn try_part(&mut self, part: &'a hir::HirRegion) -> Vec<MirBlock> {
        let aliases = self.aliases.clone();
        let displays = self.displays.clone();
        let parameter_reads = self.parameter_reads.clone();
        let iteration_sources = self.iteration_sources.clone();
        let runtime_aliases = self.runtime_aliases.clone();
        let aggregate_loans = self.aggregate_loans.clone();
        let conversion_view_loans = self.conversion_view_loans.clone();
        let transfer_domain_loans = self.transfer_domain_loans.clone();
        let pending_argument_anchors = std::mem::take(&mut self.pending_argument_anchors);
        let call_setups = std::mem::take(&mut self.call_setups);
        let argument_depth = std::mem::replace(&mut self.argument_depth, 0);
        let allow_argument_anchors = std::mem::replace(&mut self.allow_argument_anchors, false);
        let blocks = self.hir_region(part);
        self.aliases = aliases;
        self.displays = displays;
        self.parameter_reads = parameter_reads;
        self.iteration_sources = iteration_sources;
        self.runtime_aliases = runtime_aliases;
        self.aggregate_loans = aggregate_loans;
        self.conversion_view_loans = conversion_view_loans;
        self.transfer_domain_loans = transfer_domain_loans;
        self.pending_argument_anchors = pending_argument_anchors;
        self.call_setups = call_setups;
        self.argument_depth = argument_depth;
        self.allow_argument_anchors = allow_argument_anchors;
        blocks
    }

    /// A place for a call argument's *write-back* — a variable or a field chain,
    /// **without** any dynamic index (so building it emits nothing and avoids
    /// re-evaluating an index that the argument's value already consumed). Returns
    /// `None` for a temporary or an indexed place (write-back to those is refused by
    /// the VM). Distinct from [`Self::try_place`], which emits index evaluations.
    pub(super) fn simple_place(&mut self, e: Node<'a>) -> Option<MirPlace> {
        let place = self.simple_place_unrebound(e)?;
        Some(e.rebound_place(place))
    }

    fn simple_place_unrebound(&mut self, e: Node<'a>) -> Option<MirPlace> {
        if let Some(name) = self.name_spelled(e) {
            // A module constant read is the parameter value the check
            // recorded: a value, never a place.
            return (!e.constant_param_value()).then(|| self.name_place_root(&name, e));
        }
        match (e.op(), e.object()) {
            (CheckedOp::Index { .. }, Some(object))
                if let Some(index) = e.variant_projection_index() =>
            {
                let mut p = self.simple_place(object)?;
                let ty = e.place_ty().or_else(|| e.ty())?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            (CheckedOp::Member { field, .. }, Some(object)) => {
                // A compile-time member read (`T.K`, `A.K`) is a value.
                if e.is_dtype_constant()
                    || e.simd_length().is_some()
                    || e.recorded_value().is_some()
                {
                    return None;
                }
                let mut p = self.simple_place(object)?;
                e.project_member(&mut p, field);
                Some(p)
            }
            (CheckedOp::TypeApply { name, .. }, _) => {
                let index = e.variant_projection_index()?;
                let mut p = self.resolved_place(name);
                let ty = e.place_ty().or_else(|| e.ty())?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            _ => None,
        }
    }

    /// Decompose `e` into a place **iff** it is a variable or a *pure field
    /// chain* rooted at one (`x`, `p.a`, `p.a.b`) — no dynamic index. Used to
    /// distinguish a place read (`LoadPlace`) from a temporary/indexed read, and
    /// a partial move (`p.a^`) from an untracked indexed transfer. Emits nothing.
    pub(super) fn pure_field_place(&mut self, e: Node<'a>) -> Option<MirPlace> {
        let place = self.pure_field_place_unrebound(e)?;
        Some(e.rebound_place(place))
    }

    fn pure_field_place_unrebound(&mut self, e: Node<'a>) -> Option<MirPlace> {
        if let Some(name) = self.name_spelled(e) {
            // `Self.<name>` (a reified value-parameter read, e.g. `Self.size`)
            // resolves off the receiver `self`: `Self` in expression position is
            // an alias for `self`, and the backend's field navigation also
            // searches a struct's `value_params`. `Self` never appears bare in an
            // expression (only `Self.field`), so this alias is safe.
            let root = if name == "Self" { "self" } else { &name };
            return Some(self.name_place_root(root, e));
        }
        match (e.op(), e.object()) {
            (CheckedOp::Member { field, .. }, Some(object)) => {
                // A compile-time member read (`T.K`, `A.K`) is a value.
                if e.is_dtype_constant()
                    || e.simd_length().is_some()
                    || e.recorded_value().is_some()
                {
                    return None;
                }
                let mut p = self.pure_field_place(object)?;
                e.project_member(&mut p, field);
                Some(p)
            }
            _ => None,
        }
    }

    pub(super) fn try_place(&mut self, e: Node<'a>) -> Option<MirPlace> {
        let place = self.try_place_unrebound(e)?;
        Some(e.rebound_place(place))
    }

    fn try_place_unrebound(&mut self, e: Node<'a>) -> Option<MirPlace> {
        if let Some(name) = self.name_spelled(e) {
            return Some(self.name_place_root(&name, e));
        }
        match (e.op(), e.object()) {
            (CheckedOp::MultiIndex { .. }, _)
                if let Some((object, offset)) = e.pointer_keyword_subscript() =>
            {
                let mut p = self.try_place(object)?;
                let idx = self.expr(offset);
                if let Some(ty) = e.place_ty().or_else(|| e.ty()) {
                    p.project(Proj::Index(idx), ty);
                } else {
                    p.proj.push(Proj::Index(idx));
                }
                Some(p)
            }
            (CheckedOp::Member { field, .. }, Some(object)) => {
                // A compile-time member read (`T.K`, `A.K`) is a value.
                if e.is_dtype_constant()
                    || e.simd_length().is_some()
                    || e.recorded_value().is_some()
                {
                    return None;
                }
                let mut p = self.try_place(object)?;
                e.project_member(&mut p, field);
                Some(p)
            }
            (CheckedOp::Index { .. }, Some(object)) if let Some(index) = e.index() => {
                let mut p = self.try_place(object)?;
                if let Some(index) = e.variant_projection_index() {
                    let ty = e.place_ty().or_else(|| e.ty())?;
                    p.project(Proj::Variant(index), ty);
                    return Some(p);
                }
                // A literal index into compiler-private heterogeneous Tuple
                // storage is part of the place's static identity. Keeping it
                // out of a register lets ownership distinguish element 0 from
                // element 1 while every dynamic/nominal subscript retains the
                // ordinary single-evaluation Index(Reg) path.
                let projection = match object.ty().as_ref().map(Ty::kind) {
                    Some(TyKind::Tuple(_)) => index
                        .exact_nonnegative_index()
                        .map_or_else(|| Proj::Index(self.expr(index)), Proj::ConstIndex),
                    _ => Proj::Index(self.expr(index)),
                };
                if let Some(ty) = e.place_ty().or_else(|| e.ty()) {
                    p.project(projection, ty);
                } else {
                    p.proj.push(projection);
                }
                Some(p)
            }
            (CheckedOp::TypeApply { name, .. }, _) => {
                let index = e.variant_projection_index()?;
                let mut p = self.resolved_place(name);
                let ty = e.place_ty().or_else(|| e.ty())?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            _ => None,
        }
    }

    /// Lower the "catch-all" straight-line statements. Statement node `node`
    /// decides the lowering; `s`, the syntax rebuilt from it, binds only the
    /// expression operands still lowered from syntax. `hir` is the HIR
    /// statement it lowers: its expression roots, and the slot HIR interned
    /// for the binding the statement declares. Every reachable
    /// case is handled; the categorization decisions are documented per arm.
    pub(super) fn lower_stmt(&mut self, node: &CheckedStmt, hir: &'a hir::HirStmt) {
        // Bracket the statement for temporary-argument anchors: hidden
        // `$arg_loan_r` slots created while lowering this statement's calls
        // stay live (as do their loans) until the consuming call returns, at
        // the latest the statement's end, via a `KeepAlive` use. An assignment
        // flushes them before its store, because upstream destroys the
        // temporary at the call.
        // Nested statement lowering saves and restores the enclosing list, so
        // each statement flushes exactly its own anchors.
        let saved = std::mem::take(&mut self.pending_argument_anchors);
        self.lower_stmt_dispatch(node, hir);
        self.flush_argument_anchors();
        self.pending_argument_anchors = saved;
    }

    /// Emit the trailing `KeepAlive` use of every hidden `$arg_loan_r` slot
    /// anchored so far, ending the temporaries' statement lifetime here. A
    /// terminator operand (a `return`/branch expression) lowers outside any
    /// statement bracket, so its anchors flush before the terminator itself.
    pub(super) fn flush_argument_anchors(&mut self) {
        for var in std::mem::take(&mut self.pending_argument_anchors) {
            self.emit(MirInstr::KeepAlive { var });
        }
    }

    /// Make the closure of the nested `def` `info` without storage that a
    /// sibling captures ([`NestedInfo::slotted`]), declared `name` at `at`,
    /// and store it in the declaration's slot.
    fn lower_slotted_nested_def(&mut self, info: &NestedInfo, name: &str, at: SourceSpan) {
        let src = self.emit_nested_closure(info, at);
        let var = self.declare_binding_var(info.binding, name);
        if let Some(ty) = &info.callable_ty {
            self.var_types.entry(var).or_insert_with(|| ty.clone());
        }
        self.emit(MirInstr::DefVar {
            var,
            src,
            binding_ty: info.callable_ty.clone(),
            in_place: false,
        });
    }

    /// Build the storage struct `storage` of the capturing lambda `info`
    /// at `at` in a hidden slot, once per evaluation, as upstream's
    /// `resolveAnonymousClosure` emits a closure instance where the lambda
    /// stands.
    pub(in crate::mir) fn lambda_storage_slot(
        &mut self,
        info: &NestedInfo,
        storage: &ClosureStorage,
        at: &SourceSpan,
    ) -> VarId {
        let src = self.build_closure_storage(info, storage, at);
        let slot = self.fresh_var();
        self.var_types.insert(slot, storage.ty.clone());
        self.emit(MirInstr::DefVar {
            var: slot,
            src,
            binding_ty: Some(storage.ty.clone()),
            in_place: false,
        });
        slot
    }

    /// Build the storage struct `storage` of the capture-list `def`
    /// `info`, declared `name` at `at`, in the declaration's slot.
    fn lower_closure_storage_value(
        &mut self,
        info: &NestedInfo,
        storage: &ClosureStorage,
        name: &str,
        at: &SourceSpan,
    ) {
        let dest = self.build_closure_storage(info, storage, at);
        let var = self.declare_binding_var(info.binding, name);
        self.var_types.insert(var, storage.ty.clone());
        self.emit(MirInstr::DefVar {
            var,
            src: dest,
            binding_ty: Some(storage.ty.clone()),
            in_place: false,
        });
    }

    /// The storage struct `storage` of the closure `info` at `at`, as
    /// upstream's `ClosureEmitter::emitClosure` calls the storage's
    /// `__init__` with the captures: a `var` capture's place is read, a
    /// moved capture's storage is handed to its `var` parameter, as an
    /// owned argument's is, and a reference capture's place, a captured
    /// sibling's storage slot included, is lent.
    pub(in crate::mir) fn build_closure_storage(
        &mut self,
        info: &NestedInfo,
        storage: &ClosureStorage,
        at: &SourceSpan,
    ) -> Reg {
        let mut args = Vec::new();
        let mut arg_places = Vec::new();
        for (capture, (_, field)) in info.captures.iter().zip(&storage.fields) {
            let place = self.capture_place(capture);
            let ty = match field.kind() {
                TyKind::Ref(reference) => reference.referent.clone(),
                _ => field.clone(),
            };
            let dest = self.fresh_typed(at.clone(), Some(place.root), ty);
            if capture.kind == mojito_ast::ast::CaptureKind::Move {
                self.emit(MirInstr::UseVar {
                    dest,
                    var: place.root,
                    mode: UseMode::Forward,
                    constructor: None,
                });
                arg_places.push(None);
            } else {
                self.emit(MirInstr::LoadPlace {
                    dest,
                    place: place.clone(),
                });
                arg_places.push(Some(place));
            }
            args.push(dest);
        }
        let TyKind::Struct(structure, parameters) = storage.ty.kind() else {
            unreachable!("a closure's storage is a struct")
        };
        let constructor = format!("{structure}.__init__");
        let dest = self.fresh_typed(at.clone(), None, storage.ty.clone());
        self.record_call(dest, Some(constructor.clone()), 0);
        self.emit_call(MirInstr::Call {
            dest,
            func: FuncRef::named(&constructor),
            raises: None,
            args,
            kwargs: Vec::new(),
            arg_places,
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            // The storage's parameters are the enclosing binders, which no
            // capture need spell.
            instantiated_args: parameters.clone(),
            spread: None,
        });
        dest
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    fn lower_stmt_dispatch(&mut self, node: &CheckedStmt, hir: &'a hir::HirStmt) {
        let statement_binding = node.binding;
        let operands: Vec<Node<'a>> = hir
            .expressions
            .iter()
            .map(|expression| self.handle(expression))
            .collect();
        match (&node.op, operands.as_slice()) {
            (CheckedStmtOp::RefDecl { .. }, &[value]) => {
                let Some(reference) = hir.declared else {
                    self.emit(MirInstr::Unsupported(
                        "a ref declaration without its HIR slot reached MIR".into(),
                    ));
                    return;
                };
                if let Some(binding) = statement_binding {
                    self.owner_vars.insert(binding, reference);
                }
                let mutable = value.borrow_mutability().unwrap_or(true);
                // A borrowed temporary (`ref x = make_list()`) materializes
                // into a hidden owned slot registered under the checker-minted
                // owner; the binding then aliases that slot exactly like an
                // owned place (the place branch below resolves the expression
                // to the slot's root).
                let materialized =
                    mojito_checked::checked::materialized_borrow_owner(value.adjustments());
                if let Some(owner) = materialized {
                    let source = self.expr(value);
                    // Reference-context expression lowering may already have
                    // materialized the slot through `reference_handle`; only
                    // store the value if this is its first materialization.
                    if !self.owner_vars.contains_key(&owner) {
                        let ty = self
                            .f
                            .reg_types
                            .get(&source.0)
                            .cloned()
                            .or_else(|| value.ty());
                        let hidden = self.var(&format!("$mat_r{}", source.0));
                        if let Some(ty) = ty.clone() {
                            self.var_types.insert(hidden, ty);
                        }
                        self.emit(MirInstr::DefVar {
                            var: hidden,
                            src: source,
                            binding_ty: ty,
                            in_place: false,
                        });
                        self.owner_vars.insert(owner, hidden);
                    }
                }
                // `ref y = x` over a binding that itself stores a runtime
                // handle (a borrowed loop element, a `mut` parameter)
                // forwards the handle: the new binding aliases the same
                // storage and carries the same loans.
                if materialized.is_none()
                    && let Some(name) = self.name_spelled(value)
                    && let var = self.expression_var(&name, value)
                    && (var as usize) >= self.f.n_params
                    && self.runtime_aliases.contains(&var)
                    && let Some(slot_ty) = self.var_types.get(&var).cloned()
                    && matches!(slot_ty.kind(), TyKind::Ref(_))
                {
                    let slot_ty = Some(slot_ty);
                    let mut place = MirPlace::root(var, slot_ty.clone());
                    place.through = Some(var);
                    let handle_ty = mir_place_handle_ty(&place, None).unwrap_or_else(Ty::error);
                    let handle = self.fresh_typed(node.location(), Some(var), handle_ty);
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place,
                    });
                    if let Some(ty) = slot_ty.clone() {
                        self.var_types.insert(reference, ty);
                    }
                    self.runtime_aliases.insert(reference);
                    self.emit(MirInstr::DefVar {
                        var: reference,
                        src: handle,
                        binding_ty: slot_ty,
                        in_place: false,
                    });
                    if let Some(loans) = self.aggregate_loans.get(&var).cloned() {
                        let marker = self.fresh_typed(
                            node.location(),
                            Some(loans[0].place.root),
                            Ty::none(),
                        );
                        self.emit(MirInstr::EstablishLoans {
                            reference,
                            loans: reference_binding_loans(&loans),
                            marker,
                            dest_interior: None,
                        });
                        self.aggregate_loans.insert(reference, loans);
                    }
                    return;
                }
                if materialized.is_none()
                    && (value.reference_result().is_some()
                        || !(matches!(
                            Some(value.op()),
                            Some(
                                CheckedOp::Identifier(_)
                                    | CheckedOp::Member { .. }
                                    | CheckedOp::Index { .. }
                                    | CheckedOp::TypeApply { .. }
                            )
                        ) || value.pointer_keyword_subscript().is_some()))
                {
                    let source = self.expr(value);
                    self.runtime_aliases.insert(reference);
                    // A reference-producing expression stores a runtime handle
                    // in this local slot. Carry its checked `TyKind::Ref` onto the
                    // slot immediately: later aggregate construction may need
                    // to forward the handle before any ordinary read has had a
                    // chance to seed `var_types` incidentally.
                    let binding_ty = self
                        .f
                        .reg_types
                        .get(&source.0)
                        .filter(|ty| matches!(ty.kind(), TyKind::Ref(_)))
                        .cloned()
                        .or_else(|| {
                            value
                                .reference_result()
                                .map(|inner| Ty::new(TyKind::Ref(inner)))
                        })
                        .or_else(|| value.ty());
                    if let Some(ty) = binding_ty.clone() {
                        self.var_types.insert(reference, ty);
                    }
                    self.emit(MirInstr::DefVar {
                        var: reference,
                        src: source,
                        binding_ty,
                        in_place: false,
                    });
                    let candidates: Vec<Node<'a>> = match value.op() {
                        CheckedOp::Call { .. } | CheckedOp::MethodCall { .. } => value
                            .object()
                            .into_iter()
                            .chain(value.args())
                            .chain(value.kwargs().into_iter().map(|(_, argument)| argument))
                            .collect(),
                        _ => Vec::new(),
                    };
                    let candidate_places: Vec<_> = candidates
                        .into_iter()
                        .filter_map(|candidate| {
                            self.simple_place(candidate)
                                .map(|place| (candidate.owner(), place))
                        })
                        .collect();
                    let checked_places = value.reference_places();
                    let mut loans = Vec::new();
                    for origin in checked_places {
                        let fallback = candidate_places
                            .iter()
                            .find(|(owner, _)| *owner == Some(origin.root))
                            .map(|(_, place)| place.root);
                        let Some(canonical) = self.mir_interior_origin(&origin, fallback) else {
                            continue;
                        };
                        // The canonical output origin is also the physical
                        // lifetime dependency.  A candidate may itself be a
                        // runtime reference handle, whose slot is not the
                        // ultimate owner; in that case retain the canonical
                        // owner root directly instead of arbitrarily choosing
                        // the first argument handle.
                        let place = candidate_places
                            .iter()
                            .find(|(_, place)| place.root == canonical.root)
                            .map_or_else(
                                || {
                                    MirPlace::root(
                                        canonical.root,
                                        self.var_types.get(&canonical.root).cloned(),
                                    )
                                },
                                |(_, place)| place.clone(),
                            );
                        let interior = canonical
                            .path
                            .iter()
                            .any(|segment| {
                                matches!(segment, mojito_types::origin::OriginSeg::Interior(_))
                            })
                            .then_some(canonical);
                        loans.push(MirLoan {
                            place,
                            mutable,
                            interior,
                            shared: false,
                            setup: false,
                        });
                    }
                    if let Some(first) = loans.first() {
                        let marker =
                            self.fresh_typed(node.location(), Some(first.place.root), Ty::none());
                        self.emit(MirInstr::EstablishLoans {
                            reference,
                            loans: reference_binding_loans(&loans),
                            marker,
                            dest_interior: None,
                        });
                        self.aggregate_loans.insert(reference, loans);
                    }
                    return;
                }
                // A projection below a nominal reference-returning accessor is
                // rooted in that runtime handle, not in raw nominal storage.
                // Materializing the checked accessor here also ensures the
                // subscript and its index are evaluated exactly once at the
                // reference declaration.
                let projected_reference = self.lower_projected_reference_place(value);
                let place = projected_reference
                    .clone()
                    .unwrap_or_else(|| self.place(value));
                // Some reference-producing places (currently Dict lookup)
                // define a new interior generation as part of locating the
                // storage. Invalidate the previous generation before installing
                // this reference's fresh one.
                if projected_reference.is_none() {
                    self.emit_interior_invalidations(value, None);
                }
                let checked_places = value.reference_places();
                let mut loans = Vec::new();
                for origin in checked_places {
                    let Some(canonical) = self.mir_interior_origin(&origin, Some(place.root))
                    else {
                        continue;
                    };
                    let interior = canonical
                        .path
                        .iter()
                        .any(|segment| {
                            matches!(segment, mojito_types::origin::OriginSeg::Interior(_))
                        })
                        .then_some(canonical);
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior,
                        shared: false,
                        setup: false,
                    });
                }
                if loans.is_empty() {
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior: None,
                        shared: false,
                        setup: false,
                    });
                }
                // A substituted local alias has no runtime handle value, but
                // its slot is still the checked capability through which every
                // derived place is accessed. Retain that `ref T` declaration
                // type so MIR verification can prove `place.through` instead of
                // treating the analytical alias slot as untyped storage.
                if let Some(reference_ty) = statement_binding.and_then(|binding| {
                    self.checked
                        .declarations
                        .iter()
                        .find(|declaration| declaration.binding == Some(binding))
                        .and_then(|declaration| declaration.ty.clone())
                        .filter(|ty| matches!(ty.kind(), TyKind::Ref(_)))
                }) {
                    self.var_types.insert(reference, reference_ty);
                }
                // The slot holds the handle `lit.ref` would be: every read
                // through the binding addresses its place, and a backend
                // addresses that place from this handle.
                let handle_ty = self
                    .var_types
                    .get(&reference)
                    .filter(|ty| matches!(ty.kind(), TyKind::Ref(_)))
                    .cloned()
                    .or_else(|| {
                        mir_place_handle_ty(
                            &place,
                            Some(if mutable {
                                mojito_types::origin::Mutability::Mutable
                            } else {
                                mojito_types::origin::Mutability::Immutable
                            }),
                        )
                    })
                    .unwrap_or_else(Ty::error);
                self.var_types
                    .entry(reference)
                    .or_insert_with(|| handle_ty.clone());
                let handle = self.fresh_typed(node.location(), Some(place.root), handle_ty.clone());
                self.emit(MirInstr::MakeRef {
                    dest: handle,
                    place: place.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: reference,
                    src: handle,
                    binding_ty: Some(handle_ty),
                    in_place: false,
                });
                self.aliases.insert(reference, loans[0].clone());
                let marker = self.fresh_typed(node.location(), Some(place.root), Ty::none());
                self.emit(MirInstr::EstablishLoans {
                    reference,
                    loans: reference_binding_loans(&loans),
                    marker,
                    dest_interior: None,
                });
                self.aggregate_loans.insert(reference, loans);
            }
            // --- Writes through a place (any nesting) --------------------------
            (CheckedStmtOp::SetPlace { .. }, &[place, value]) => self.lower_set_place(place, value),
            (CheckedStmtOp::AugAssign { op, .. }, &[place, value]) => {
                if self.lower_augmented_subscript(place, *op, value) {
                    return;
                }
                if self.lower_augmented_in_place(place, *op, value) {
                    return;
                }
                // `place OP= e` — read the place, apply the op, write it back. A bare
                // variable uses the `UseVar`/`DefVar` fast path (what move-analysis
                // reads for a var); a projected place uses `LoadPlace`/`Store`, with
                // the place flattened once so its indices are evaluated once.
                if let Some(name) = self.name_spelled(place) {
                    // HIR may give a same-spelled sibling declaration a
                    // distinct runtime slot. Resolve the checked owner for the
                    // write half just as `self.expr(place)` does for the read.
                    let var = self.expression_var(&name, place);
                    let cur = self.expr(place);
                    let rhs = self.expr(value);
                    let res = self.fresh(place.location(), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                        raises: None,
                        refined_bound: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    if self.runtime_aliases.contains(&var) && !self.pointer_valued_slot(var) {
                        let handle = self.fresh(place.location(), Some(var));
                        self.emit(MirInstr::MakeRef {
                            dest: handle,
                            place: {
                                let mut place =
                                    MirPlace::root(var, self.var_types.get(&var).cloned());
                                place.through = Some(var);
                                place
                            },
                        });
                        self.emit(MirInstr::WriteRef {
                            reference: handle,
                            value: res,
                        });
                    } else if let Some(loan) = self.aliases.get(&var).cloned() {
                        let mut target = loan.place;
                        target.through = Some(var);
                        self.emit(MirInstr::Store {
                            place: target,
                            src: res,
                        });
                    } else {
                        self.emit(MirInstr::DefVar {
                            var,
                            src: res,
                            binding_ty: None,
                            in_place: false,
                        });
                    }
                } else if let Some((object, _)) = place.pointer_offset_subscript()
                    && let Some(target) = self.pointer_deref_place(object)
                {
                    // `p[0] OP= e` through a stably bound pointer: owner-place
                    // load and store, exactly like an alias write-back.
                    let cur = self.fresh(place.location(), Some(target.root));
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: target.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(place.location(), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                        raises: None,
                        refined_bound: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store {
                        place: target,
                        src: res,
                    });
                } else if let Some((object, _)) = place.pointer_offset_subscript()
                    && object.is_origin_bearing_pointer()
                {
                    // `p[0] OP= e` through an origin-bearing pointer: read and
                    // write through the handle, evaluated once.
                    let reference = self.expr(object);
                    let cur = self.fresh(place.location(), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(place.location(), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                        raises: None,
                        refined_bound: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else if matches!(
                    place.place_ty().as_ref().map(Ty::kind),
                    Some(TyKind::Ref(_))
                ) {
                    // A projected ref-valued slot (for example an element of
                    // `List[ref T]`) is two distinct places: the container slot
                    // stores the handle, while augmented assignment reads and
                    // writes the referent. Preserve the handle explicitly so a
                    // nominal container's index dunder cannot feed `ref` itself
                    // into the arithmetic operation.
                    let reference = self.reference_handle(place);
                    let cur = self.fresh(place.location(), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(place.location(), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                        raises: None,
                        refined_bound: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else {
                    let p = self
                        .lower_projected_reference_place(place)
                        .unwrap_or_else(|| self.place(place));
                    let cur = self.fresh(place.location(), None);
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: p.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(place.location(), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                        raises: None,
                        refined_bound: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store { place: p, src: res });
                }
            }

            // --- Simple effectful statements -----------------------------------
            (CheckedStmtOp::Comptime { name, denotes, .. }, &[value]) => {
                self.lower_comptime_binding(node, name, denotes.as_ref(), value);
            }
            // `pass` has no runtime effect. Imports were consumed by linking and
            // are no-ops in a lowered module body.
            (
                CheckedStmtOp::Pass
                | CheckedStmtOp::Import { .. }
                | CheckedStmtOp::FromImport { .. },
                _,
            ) => {}

            // A direct nested declaration with captures builds its storage
            // (`NestedInfo::storage`) exactly here, so copy and move
            // captures snapshot or transfer before any following statement
            // can mutate or use the source binding. Any other lowers to
            // nothing unless a sibling captures it (`NestedInfo::slotted`).
            (CheckedStmtOp::Def(def), _) => {
                let info = statement_binding
                    .and_then(|binding| self.nested.get(&binding))
                    .filter(|info| info.materialized_here)
                    .cloned();
                match info {
                    Some(info) => match &info.storage {
                        Some(storage) => self.lower_closure_storage_value(
                            &info,
                            storage,
                            &def.name,
                            &node.location(),
                        ),
                        None if info.slotted => {
                            self.lower_slotted_nested_def(&info, &def.name, node.location());
                        }
                        None => {}
                    },
                    None => self.emit(MirInstr::Unsupported(
                        "nested def/struct/trait declaration".into(),
                    )),
                }
            }
            // A nested `def` we couldn't lift because it nests another declaration,
            // or a nested `struct`/`trait`, stays a clean `Unsupported`.
            (CheckedStmtOp::Struct(_) | CheckedStmtOp::Trait(_), _) => self.emit(
                MirInstr::Unsupported("nested def/struct/trait declaration".into()),
            ),

            // Destructuring `(var a, (ref b, _)) = t`: the destinations'
            // places first, then the value once, then each leaf from its
            // element (`lower_destructure`).
            (CheckedStmtOp::Destructure { target, .. }, &[ref places @ .., value]) => {
                self.lower_destructure(target, places, value);
            }

            // --- Unreachable after the checker ---------------------------------
            // Parse-only statements are flagged `Unsupported`, so a checked program
            // never reaches MIR with them.
            (
                CheckedStmtOp::With { .. }
                | CheckedStmtOp::ComptimeIf { .. }
                | CheckedStmtOp::ComptimeFor { .. }
                | CheckedStmtOp::ComptimeMatch { .. }
                | CheckedStmtOp::ComptimeAssert { .. },
                _,
            ) => {
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked statement reached MIR lowering: {}",
                    node.op.kind_name()
                )));
            }
            // These are lowered by `hir::Lower` directly (to instrs/terminators), so
            // they never arrive here wrapped in a `HirInstr::Stmt`.
            (
                CheckedStmtOp::If { .. }
                | CheckedStmtOp::While { .. }
                | CheckedStmtOp::For { .. }
                | CheckedStmtOp::Scope(_)
                | CheckedStmtOp::Break
                | CheckedStmtOp::Continue
                | CheckedStmtOp::Try { .. }
                | CheckedStmtOp::Return(_)
                | CheckedStmtOp::Raise(_)
                | CheckedStmtOp::VarDecl { .. }
                | CheckedStmtOp::Assign { .. }
                | CheckedStmtOp::Expr(_),
                _,
            ) => {
                self.emit(MirInstr::Unsupported(format!(
                    "malformed HIR statement instruction: {}",
                    node.op.kind_name()
                )));
            }
            // A `match` without its plan is a form the checker admits before
            // its lowering lands: a later-phase boundary here.
            (CheckedStmtOp::Match { .. }, _) => {
                self.emit(MirInstr::Unsupported("'match' statement lowering".into()));
            }
            (
                CheckedStmtOp::RefDecl { .. }
                | CheckedStmtOp::SetPlace { .. }
                | CheckedStmtOp::AugAssign { .. }
                | CheckedStmtOp::Comptime { .. }
                | CheckedStmtOp::Destructure { .. },
                _,
            ) => self.emit(MirInstr::Unsupported(
                "a HIR statement's expressions do not match its checked node".into(),
            )),
        }
    }

    /// Lower a destructuring assignment: the places among `places` (the
    /// target's place leaves, in order) are evaluated first, as the pin
    /// evaluates destinations before the right-hand side, then `value` once,
    /// then each leaf is bound, written, or discarded from its element.
    pub(in crate::mir) fn lower_destructure(
        &mut self,
        target: &CheckedTarget,
        places: &[Node<'a>],
        value: Node<'a>,
    ) {
        let plan = value
            .tuple_unpack_plan()
            .expect("a checked destructuring value carries an extraction plan");
        // A name re-assigns its slot; any other place, or a nominal
        // subscript's receiver and indices, is evaluated now and written
        // after the value.
        let lowered = places
            .iter()
            .map(|&place| {
                if self.name_spelled(place).is_some() {
                    return None;
                }
                if place.call_contract().is_some()
                    && let Some((setter, ())) = self.lower_subscript_setter(place, |_| ())
                {
                    return Some(LeafStore::Setter(Box::new(setter)));
                }
                // A subscript with only a getter is written through the
                // reference it returns.
                let materialized = (place.reference_result().is_some()
                    && place.index_operands().is_some())
                .then(|| self.materialize_reference_result_place(place))
                .flatten();
                Some(LeafStore::Place(
                    materialized
                        .or_else(|| self.lower_projected_reference_place(place))
                        .unwrap_or_else(|| self.place(place)),
                ))
            })
            .collect();
        let mut base_place = self.simple_place(value);
        // A tuple place is read where it stands, as upstream's unpacking
        // projects each element from the tuple's address; only a temporary
        // is evaluated.
        let mut tuple = match &base_place {
            Some(place) => {
                let loaded = self.fresh_typed(
                    value.location(),
                    Some(place.root),
                    value
                        .ty()
                        .or_else(|| place.ty.clone())
                        .unwrap_or_else(Ty::error),
                );
                self.emit(MirInstr::LoadPlace {
                    dest: loaded,
                    place: place.clone(),
                });
                loaded
            }
            None => self.expr(value),
        };
        // A temporary read through element references lives in a hidden
        // slot under the owner the checker minted for it, the root those
        // references name.
        if base_place.is_none()
            && let Some(owner) = plan.first().and_then(|element| element.temporary)
            && !self.owner_vars.contains_key(&owner)
        {
            let ty = self
                .f
                .reg_types
                .get(&tuple.0)
                .cloned()
                .or_else(|| value.ty());
            let hidden = self.var(&format!("$mat_r{}", tuple.0));
            if let Some(ty) = ty.clone() {
                self.var_types.insert(hidden, ty);
            }
            self.emit(MirInstr::DefVar {
                var: hidden,
                src: tuple,
                binding_ty: ty.clone(),
                in_place: false,
            });
            self.owner_vars.insert(owner, hidden);
            // The slot now owns the tuple, so its elements are read from
            // the slot, not from the register the store consumed.
            let place = MirPlace::root(hidden, ty);
            tuple = self.fresh_typed(
                value.location(),
                Some(hidden),
                place.ty.clone().unwrap_or_else(Ty::error),
            );
            self.emit(MirInstr::LoadPlace {
                dest: tuple,
                place: place.clone(),
            });
            base_place = Some(place);
        }
        // A loan-carrying element (a view into the unpacked value's source)
        // inherits the value's loans, exactly as binding the whole value
        // would: otherwise the source dies at the call that produced the
        // tuple.
        let loans = self.aggregate_borrows(value);
        let CheckedTargetKind::Tuple(items) = &target.kind else {
            self.emit(MirInstr::Unsupported(
                "a destructuring target that is not a tuple".into(),
            ));
            return;
        };
        let mut frame = DestructureFrame {
            places,
            lowered,
            next: 0,
            loans,
            root_place: base_place.clone(),
            value,
        };
        self.lower_destructure_elements(items, &plan, tuple, base_place.as_ref(), &mut frame);
    }

    /// Bind each of `items` from its element of the tuple in `base`
    /// (`base_place` when it is a place), per `plan`.
    fn lower_destructure_elements(
        &mut self,
        items: &[CheckedTarget],
        plan: &[CheckedTupleUnpackElement],
        base: Reg,
        base_place: Option<&MirPlace>,
        frame: &mut DestructureFrame<'a, '_>,
    ) {
        assert_eq!(
            plan.len(),
            items.len(),
            "checked tuple destructuring arity matches its targets"
        );
        let location = frame.value.location();
        for (i, (item, extraction)) in items.iter().zip(plan).enumerate() {
            // `_` discards its element without reading it.
            if matches!(item.kind, CheckedTargetKind::Discard) {
                continue;
            }
            let idx = self.fresh_typed(location.clone(), None, Ty::int());
            self.emit(MirInstr::Const {
                dest: idx,
                k: Const::Int(i as i64),
            });
            let raw_ty = extraction.reference.clone().map_or_else(
                || extraction.ty.clone(),
                |reference| Ty::new(TyKind::Ref(reference)),
            );
            let raw = self.fresh_typed(location.clone(), None, raw_ty.clone());
            let call = extraction.accessor.clone().map(|target| MirSubscriptCall {
                target,
                raises: None,
                result_ty: raw_ty.clone(),
                receiver_requires_place: extraction.reference.is_some(),
                // A `ref self` accessor reads its receiver unless the
                // reference it returns is mutable, the checker's effective
                // convention for a reference-returning receiver.
                receiver_convention: extraction.reference.as_ref().map(|reference| {
                    if reference.mutability == mojito_types::origin::Mutability::Mutable {
                        mojito_ast::ast::ArgConvention::Ref
                    } else {
                        mojito_ast::ast::ArgConvention::Imm
                    }
                }),
                arguments: Vec::new(),
                capture_accesses: Vec::new(),
                reference_result: extraction.reference.clone(),
                param_arg_regs: extraction
                    .param_decls
                    .iter()
                    .map(|_| MirParamArg {
                        name: None,
                        value: Some(idx),
                        expr: None,
                    })
                    .collect(),
                param_decls: extraction.param_decls.clone(),
                refined_bound: None,
            });
            let intrinsic = call
                .is_none()
                .then(|| frame.value.intrinsic_index_dispatch())
                .flatten();
            self.emit(MirInstr::Index {
                dest: raw,
                base,
                index: idx,
                base_place: base_place.cloned().map(Box::new),
                index_place: None,
                call: call.map(Box::new),
                intrinsic,
            });
            // A nested tuple's elements are read through this element's
            // reference; a `ref` leaf keeps the reference itself.
            match &item.kind {
                CheckedTargetKind::Tuple(inner) => {
                    let (inner_base, inner_place) = match &extraction.reference {
                        Some(reference) => {
                            let place = self.destructured_reference_place(raw, reference, frame);
                            let loaded = self.fresh_typed(
                                location.clone(),
                                Some(place.root),
                                reference.referent.clone(),
                            );
                            self.emit(MirInstr::LoadPlace {
                                dest: loaded,
                                place: place.clone(),
                            });
                            (loaded, Some(place))
                        }
                        None => (raw, None),
                    };
                    self.lower_destructure_elements(
                        inner,
                        &extraction.elements,
                        inner_base,
                        inner_place.as_ref(),
                        frame,
                    );
                    continue;
                }
                // A `ref` leaf, or a loop target's bare name, binds the
                // element's reference.
                CheckedTargetKind::Declare {
                    name,
                    mode:
                        mojito_ast::ast::BindingMode::Ref | mojito_ast::ast::BindingMode::Immutable,
                    binding,
                    binding_ty,
                } => {
                    self.bind_destructured_reference(
                        name,
                        *binding,
                        binding_ty.clone(),
                        raw,
                        frame,
                    );
                    continue;
                }
                _ => {}
            }
            let elem = if extraction.reference.is_some() {
                let element_ty = extraction.ty.clone();
                let value = self.fresh_typed(
                    location.clone(),
                    base_place.map(|place| place.root),
                    element_ty.clone(),
                );
                self.emit(MirInstr::ReadRef {
                    dest: value,
                    reference: raw,
                });
                // The target owns its element independently of the unpacked
                // place, so a lifecycle type runs its copy initializer rather
                // than aliasing the place's storage.
                let copied = self.fresh_typed(location.clone(), None, element_ty);
                self.emit(MirInstr::CopyValue {
                    dest: copied,
                    value,
                    constructor: None,
                });
                copied
            } else {
                raw
            };
            match &item.kind {
                CheckedTargetKind::Declare {
                    name,
                    binding,
                    binding_ty,
                    ..
                } => {
                    let var = match binding {
                        Some(owner) => self.binding_var(*owner, name),
                        None => self.var(name),
                    };
                    if let Some(ty) = binding_ty.clone() {
                        self.var_types.insert(var, ty);
                    }
                    self.emit(MirInstr::DefVar {
                        var,
                        src: elem,
                        binding_ty: binding_ty.clone(),
                        in_place: false,
                    });
                    self.establish_destructured_loans(var, extraction.carries_loans, frame);
                }
                CheckedTargetKind::Place(_) => {
                    let place_node = frame.places[frame.next];
                    let lowered = frame.lowered[frame.next].take();
                    frame.next += 1;
                    match (self.name_spelled(place_node), lowered) {
                        (Some(name), _) => {
                            let var = self.expression_var(&name, place_node);
                            self.emit_interior_invalidations(place_node, Some(var));
                            let binding_ty = place_node.place_ty().or_else(|| place_node.ty());
                            if let Some(ty) = binding_ty.clone() {
                                self.var_types.insert(var, ty);
                            }
                            self.emit(MirInstr::DefVar {
                                var,
                                src: elem,
                                binding_ty,
                                in_place: false,
                            });
                            self.establish_destructured_loans(var, extraction.carries_loans, frame);
                        }
                        (None, Some(LeafStore::Place(place))) => {
                            self.emit_interior_invalidations(place_node, Some(place.root));
                            self.emit(MirInstr::Store { place, src: elem });
                        }
                        // An earlier leaf's setter may have written the
                        // receiver back, so it is read again for this one.
                        (None, Some(LeafStore::Setter(mut setter))) => {
                            setter.receiver = self.reload_augmented_source(
                                setter.receiver,
                                &setter.receiver_place,
                                true,
                                place_node.location(),
                            );
                            let (value, value_place) =
                                self.converted_store_value(place_node, (elem, None));
                            self.emit_subscript_set(*setter, value, value_place, None);
                        }
                        (None, None) => self.emit(MirInstr::Unsupported(
                            "a destructuring place leaf without its lowered place".into(),
                        )),
                    }
                }
                CheckedTargetKind::Sequence(_) => self.emit(MirInstr::Unsupported(
                    "sequence destructuring target lowering".into(),
                )),
                CheckedTargetKind::Discard | CheckedTargetKind::Tuple(_) => {}
            }
        }
    }

    /// The place a nested tuple element's reference `handle` designates: a
    /// hidden reference slot, as a reference-returning call's receiver
    /// place is materialized. The slot lives within the statement, whose
    /// reads it serves and whose `ref` leaves borrow the source themselves,
    /// so it holds no loan of its own.
    fn destructured_reference_place(
        &mut self,
        handle: Reg,
        reference: &mojito_types::origin::RefTy,
        frame: &DestructureFrame<'a, '_>,
    ) -> MirPlace {
        let variable = self.var(&format!("$destr_ref_r{}", handle.0));
        let storage_ty = Ty::new(TyKind::Ref(reference.clone()));
        self.var_types.insert(variable, storage_ty.clone());
        self.runtime_aliases.insert(variable);
        self.emit(MirInstr::DefVar {
            var: variable,
            src: handle,
            binding_ty: Some(storage_ty.clone()),
            in_place: false,
        });
        if let Some(root) = frame.root_place.clone() {
            let loans = vec![MirLoan {
                place: root,
                mutable: reference.mutability == mojito_types::origin::Mutability::Mutable,
                interior: None,
                shared: false,
                setup: false,
            }];
            let marker = self.fresh_typed(
                frame.value.location(),
                Some(loans[0].place.root),
                Ty::none(),
            );
            self.emit(MirInstr::EstablishLoans {
                reference: variable,
                loans: reference_binding_loans(&loans),
                marker,
                dest_interior: None,
            });
            self.aggregate_loans.insert(variable, loans);
        }
        let mut place = MirPlace::root(variable, Some(storage_ty));
        place.ty = Some(reference.referent.clone());
        place.through = Some(variable);
        place
    }

    /// A `ref` leaf: the element's reference handle becomes the binding's
    /// slot, which borrows the destructured source for as long as it lives.
    fn bind_destructured_reference(
        &mut self,
        name: &str,
        binding: Option<mojito_types::origin::OwnerId>,
        binding_ty: Option<Ty>,
        raw: Reg,
        frame: &DestructureFrame<'a, '_>,
    ) {
        let var = match binding {
            Some(owner) => self.binding_var(owner, name),
            None => self.var(name),
        };
        let mutable = binding_ty.as_ref().is_some_and(|ty| {
            matches!(
                ty.kind(),
                TyKind::Ref(reference)
                    if reference.mutability == mojito_types::origin::Mutability::Mutable
            )
        });
        if let Some(ty) = binding_ty.clone() {
            self.var_types.insert(var, ty);
        }
        self.runtime_aliases.insert(var);
        self.emit(MirInstr::DefVar {
            var,
            src: raw,
            binding_ty,
            in_place: false,
        });
        if let Some(place) = frame.root_place.clone() {
            let marker = self.fresh_typed(frame.value.location(), Some(place.root), Ty::none());
            self.emit(MirInstr::EstablishLoans {
                reference: var,
                loans: reference_binding_loans(&[MirLoan {
                    place,
                    mutable,
                    interior: None,
                    shared: false,
                    setup: false,
                }]),
                marker,
                dest_interior: None,
            });
        }
    }

    /// A loan-carrying element's binding inherits the destructured value's
    /// loans.
    fn establish_destructured_loans(
        &mut self,
        var: VarId,
        carries_loans: bool,
        frame: &DestructureFrame<'a, '_>,
    ) {
        if !carries_loans {
            return;
        }
        let Some(first) = frame.loans.first() else {
            return;
        };
        let marker = self.fresh_typed(frame.value.location(), Some(first.place.root), Ty::none());
        self.aggregate_loans.insert(var, frame.loans.clone());
        self.emit(MirInstr::EstablishLoans {
            reference: var,
            loans: frame.loans.clone(),
            marker,
            dest_interior: None,
        });
    }

    /// `comptime N = e` is an ordinary binding at runtime, unless the check
    /// bound it to an application, which each read materializes. A local type
    /// alias (`comptime U = T` over a template's binder) lives in the
    /// checker's scope alone: it records no binding and its value no type.
    /// A display the elaborator evaluates (`comptime L = [n, n + 1]`) has no
    /// runtime form either: it is recorded here for the loops that iterate
    /// the name and the thunks that read it, and only a thunk binds it.
    /// `node` is the binding's statement node, `denotes` the application
    /// the check bound it to, and `value` its value's handle.
    fn lower_comptime_binding(
        &mut self,
        node: &CheckedStmt,
        name: &str,
        denotes: Option<&mojito_types::param_expr::ParamExpr>,
        value: Node<'a>,
    ) {
        let statement_binding = node.binding;
        let display = value
            .adjustments()
            .iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ComptimeDisplay {
                    sequence, ..
                } => Some(sequence),
                _ => None,
            })
            .cloned();
        if let Some(sequence) = display
            && !self.enclosing_binders.lifted
        {
            if let Some((owner, ty)) = statement_binding.zip(value.ty()) {
                let binding = DisplayBinding {
                    owner,
                    name: name.to_string(),
                    value,
                    ty,
                    binders: self.enclosing_binders.clone(),
                    preceding: self.comptime_thunks.evaluated.len(),
                };
                match self
                    .comptime_thunks
                    .bind_display(node.id, &binding, &sequence)
                {
                    Ok(()) => self.displays.push(binding),
                    Err(finding) => self.emit(MirInstr::Unsupported(finding)),
                }
            }
            return;
        }
        // An alias of a display binding (`comptime A = L`) is another name
        // of it, which the check binds to the display's own owner.
        if value.is_name() && statement_binding.is_some_and(|owner| self.display(owner).is_some()) {
            return;
        }
        // A binding of an untyped attribute (`comptime f = flag._mlir_attr()`)
        // is compile-time only, read where an op's attribute takes it.
        if statement_binding.is_none()
            && value
                .ty()
                .is_none_or(|ty| matches!(ty.kind(), TyKind::KgenDeferred))
        {
            return;
        }
        // A thunk lifted after the binding reads it as the expression over
        // the binders it denotes (`comptime k = n + 1`).
        let mut denoted = self.value_binder_node(value.id());
        // A binding the check bound to an application (`comptime s =
        // label(3)`) is that application at each read, as upstream's alias
        // is: every read materializes it, so it has no slot.
        if let Some(application) = denotes
            && let Some(binding) = statement_binding
            && !self.enclosing_binders.lifted
        {
            self.comptime_thunks
                .bind_value(binding, application.clone());
            return;
        }
        let materialized = value.literal_materialization();
        // A literal a vector annotation splats (`comptime v: SIMD[DType.int32,
        // 4] = 1`) is its materialized slot, never the scalar it spells.
        if materialized.as_ref().is_some_and(|ty| {
            mojito_types::types::simd_slots(ty)
                .is_some_and(|(_, width)| width.known().is_none_or(|width| width > 1))
        }) {
            denoted = None;
        }
        // An `Int` or a `Bool` read off a display binding (`comptime e =
        // L[1]`), or one the check lifted (`comptime b = n > 2`), is the
        // application of its thunk, which later thunks read as well.
        let src = match self.display_read(value) {
            Some((src, applied)) => {
                denoted = applied;
                src
            }
            None => self.expr(value),
        };
        // A binding that denotes no parameter expression (`comptime t =
        // (L[0], L[1])`) is evaluated again by each thunk that reads it.
        if let Some(binding) = statement_binding
            && !self.enclosing_binders.lifted
        {
            match denoted {
                Some(denoted) => self.comptime_thunks.bind_value(binding, denoted),
                None => self.comptime_thunks.bind_evaluated(binding, node.id),
            }
        }
        self.bind_comptime_register(name, value, statement_binding, materialized, src);
    }

    /// Bind `src`, the lowered `value` of a `comptime` binding, to the
    /// binding's slot.
    pub(in crate::mir) fn bind_comptime_value(
        &mut self,
        name: &str,
        value: Node<'a>,
        statement_binding: Option<mojito_types::origin::OwnerId>,
        src: Reg,
    ) {
        let materialized = value.literal_materialization();
        self.bind_comptime_register(name, value, statement_binding, materialized, src);
    }

    fn bind_comptime_register(
        &mut self,
        name: &str,
        value: Node<'a>,
        statement_binding: Option<mojito_types::origin::OwnerId>,
        materialized: Option<Ty>,
        src: Reg,
    ) {
        let var = match statement_binding {
            Some(binding) => self.declare_binding_var(binding, name),
            None => self.var(name),
        };
        // A comptime statement remains a fallback HIR statement rather
        // than `HirInstr::Bind`; copy its checked expression type when
        // present, or the already-typed initializer register for
        // synthetic/compatibility paths. This makes closure capture
        // places typed without relying on an unrelated later use. A
        // materialized literal binds at its materialization target, and a
        // converted initializer (`comptime t: String = "s"`) at its
        // conversion's, a literal parameter value read the check
        // materialized (`comptime k = A.K`) included.
        let literal = |ty: &Ty| mojito_types::types::default_literal(ty) != *ty;
        let converted = self.f.reg_types.get(&src.0).cloned().filter(|register| {
            value.implicit_conversion().is_some()
                || (value.ty().is_some_and(|ty| literal(&ty)) && !literal(register))
        });
        let binding_ty = materialized
            .or(converted)
            .or_else(|| value.ty())
            .or_else(|| self.f.reg_types.get(&src.0).cloned());
        // A literal binds as its materialization at the binding's type, as a
        // `var` initializer does: a vector annotation splats it.
        let src = match &binding_ty {
            Some(target) => self.materialize_register(src, target, value.location()),
            None => src,
        };
        // A lifted evaluation's binding is a parameter-domain value, which
        // lives in compile-time memory a borrow of it may outlast.
        if self.enclosing_binders.lifted
            && let Some(ty) = binding_ty.clone()
        {
            self.bind_comptime_memory(
                value.location(),
                var,
                src,
                ty,
                mojito_types::origin::Mutability::Immutable,
            );
            return;
        }
        if let Some(ty) = binding_ty.clone() {
            self.var_types.insert(var, ty);
        }
        self.emit(MirInstr::DefVar {
            var,
            src,
            binding_ty,
            in_place: false,
        });
    }

    pub(super) fn lower_hir_stmt(&mut self, statement: &'a mojito_hir::hir::HirStmt) {
        let tables = self.checked;
        let Some(node) = tables.statement(statement.node) else {
            self.emit(MirInstr::Unsupported(
                "a HIR statement without its checked node reached MIR".into(),
            ));
            return;
        };
        self.with_statement_loans(|this| this.lower_stmt(node, statement));
    }

    /// Bind `value`, a HIR binding's value, to slot `dest`: a declaration of
    /// checked binding `binding` at `binding_ty`, or a write to the slot's
    /// existing binding.
    fn lower_bind(
        &mut self,
        dest: VarId,
        value: Node<'a>,
        binding_ty: Option<&Ty>,
        binding: Option<mojito_types::origin::OwnerId>,
    ) {
        let mut src = self.expr(value);
        if let Some(target) = binding_ty {
            src = self.materialize_register(src, target, value.location());
        }
        // The consuming call has returned: its temporary arguments'
        // loans end before the store replaces the destination.
        self.flush_argument_anchors();
        let writes_through_reference = self.aliases.contains_key(&dest)
            || (self.runtime_aliases.contains(&dest) && !self.pointer_valued_slot(dest));
        if !writes_through_reference
            && let Some(ty) = self
                .f
                .reg_types
                .get(&src.0)
                .cloned()
                .or_else(|| value.ty())
                .or_else(|| binding_ty.cloned())
        {
            self.var_types.insert(dest, ty);
        }
        if let Some(binding) = binding {
            self.owner_vars.insert(binding, dest);
        }
        // The initializer is evaluated before replacing the old
        // binding.  Whole-owner invalidation therefore sits here,
        // immediately before the write to the destination slot.
        self.emit_interior_invalidations(value, Some(dest));
        if let Some(loan) = self.aliases.get(&dest).cloned() {
            let mut place = loan.place;
            place.through = Some(dest);
            self.emit(MirInstr::Store { place, src });
        } else if self.runtime_aliases.contains(&dest) && !self.pointer_valued_slot(dest) {
            let handle = self.fresh(value.location(), Some(dest));
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place: {
                    let mut place = MirPlace::root(dest, self.var_types.get(&dest).cloned());
                    place.through = Some(dest);
                    place
                },
            });
            self.emit(MirInstr::WriteRef {
                reference: handle,
                value: src,
            });
        } else {
            // A declaration's slot is a non-raising call's result storage;
            // a write to an existing binding is a separate store.
            let in_place =
                binding_ty.is_some() && self.produced_in_place(src, value.raises().is_none());
            self.emit(MirInstr::DefVar {
                var: dest,
                src,
                binding_ty: binding_ty.cloned(),
                in_place,
            });
            let aggregate_loans = self.aggregate_borrows(value);
            if let Some(first) = aggregate_loans.first() {
                let marker = self.fresh_typed(value.location(), Some(first.place.root), Ty::none());
                self.emit(MirInstr::EstablishLoans {
                    reference: dest,
                    loans: aggregate_loans.clone(),
                    marker,
                    dest_interior: None,
                });
            }
            self.record_pointer_address(dest, value, &aggregate_loans);
            if aggregate_loans.is_empty() {
                self.aggregate_loans.remove(&dest);
            } else {
                self.aggregate_loans.insert(dest, aggregate_loans);
            }
        }
    }

    /// The `_mojito_keep_alive(name)` call the checker's `with` desugar
    /// anchors a manager's liveness with, as its argument's checked binding
    /// and its spelling in this frame.
    fn kept_alive(
        &self,
        expression: &'a hir::HirExpr,
    ) -> Option<(Option<mojito_types::origin::OwnerId>, String)> {
        let CheckedOp::Call { name, args, .. } = &self.hir_node(expression).op else {
            return None;
        };
        let [argument] = args.as_slice() else {
            return None;
        };
        let argument = self.checked.expression(*argument)?;
        let CheckedOp::Identifier(local) = &argument.op else {
            return None;
        };
        if name != "_mojito_keep_alive" {
            return None;
        }
        let local = self
            .slots
            .get(&argument.id)
            .and_then(|slot| self.vars.get(*slot as usize))
            .unwrap_or(local);
        Some((argument.binding, local.clone()))
    }

    /// Lower one statement or expression through `lower` with no
    /// conversion-view loans of an enclosing statement in scope.
    pub(in crate::mir) fn with_statement_loans<R>(
        &mut self,
        lower: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let outer = std::mem::take(&mut self.conversion_view_loans);
        let result = lower(self);
        self.conversion_view_loans = outer;
        result
    }

    pub(super) fn lower_return_value(&mut self, expression: &'a hir::HirExpr) -> Reg {
        if self.returns_reference {
            // Returning an ordinary place borrows that place. Returning a place
            // whose *storage* is already `ref T`, or forwarding another
            // reference-producing expression, instead returns the existing
            // handle. Borrowing the ref-valued slot would manufacture `ref ref T`
            // at runtime.
            let forwards_handle = matches!(
                (expression.place.as_ref().map(|place| &place.ty)).map(Ty::kind),
                Some(TyKind::Ref(_))
            ) || self.hir_node(expression).adjustments.iter().any(
                |adjustment| {
                    matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                    )
                },
            );
            if forwards_handle {
                self.reference_handle_hir(expression)
            } else {
                let place = self
                    .projected_reference_place_hir(expression)
                    .unwrap_or_else(|| self.place_hir(expression));
                let location = self.hir_node(expression).location();
                let dest = self.fresh(location, Some(place.root));
                self.emit(MirInstr::MakeRef { dest, place });
                dest
            }
        } else {
            // A lifted condition leaf returns its `Bool(x)` conversion.
            let value = if self.enclosing_binders.lifted {
                self.condition_hir(expression)
            } else {
                self.expr_hir(expression)
            };
            match self.f.ret_ty.clone() {
                Some(target) => {
                    let location = self.hir_node(expression).location();
                    self.materialize_register(value, &target, location)
                }
                None => value,
            }
        }
    }

    /// A `comptime for` as `hlcf.comptime.for`, its body opening with the
    /// `comptime if` on `has_next` whose arm is the suite and whose else
    /// arm is the stop region `orelse`, as `LowerSemanticCF` leaves it. A
    /// loop with no recorded sequence is the explicit boundary: the
    /// iterable is one the elaborator unrolls in the AST, never a loop the
    /// check kept.
    fn lower_comptime_for(
        &mut self,
        iter: &'a hir::HirExpr,
        binding: Option<mojito_types::origin::OwnerId>,
        body: &'a hir::HirRegion,
        orelse: &'a hir::HirRegion,
    ) {
        let iteration = comptime_iteration(self.checked, iter);
        let sequence = iteration.clone().map(|iteration| {
            // The variable is an alias of the element over the iterator,
            // which the body's parameter arguments and the thunks lifted in
            // it read.
            if let Some(owner) = binding {
                self.comptime_thunks
                    .bind_value(owner, iteration.next_value.clone());
            }
            let source = mojito_checked::checked::ComptimeSequence {
                initial: iteration.initial,
                has_next: iteration.has_next,
                next_iter: iteration.next_iter,
                next_value: iteration.next_value,
            };
            (iteration.binder, source)
        });
        if let Some((binder, source)) = sequence
            && let Some(iteration) = iteration
        {
            let suite = self.loop_body(&iteration, binding, body);
            let stop = self.loop_body(&iteration, binding, orelse);
            let test = MirInstr::ComptimeIf {
                results: Vec::new(),
                conds: vec![source.stop_test()],
                arms: vec![suite],
                orelse: stop,
            };
            self.emit(MirInstr::ComptimeFor {
                binder,
                source,
                operands: Vec::new(),
                args: Vec::new(),
                results: Vec::new(),
                body: vec![MirBlock {
                    instrs: vec![test],
                    term: MirTerm::Unreachable,
                }],
                orelse: vec![MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Unreachable,
                }],
            });
        } else {
            // The body still lowers, for the slots it types.
            if let Some(iteration) = iteration {
                self.loop_body(&iteration, binding, body);
            } else {
                self.hir_region(body);
            }
            self.emit(MirInstr::Unsupported(format!(
                "comptime for over `{:?}` is not a compile-time sequence",
                self.hir_node(iter).op
            )));
        }
    }

    /// A `comptime for` body, which sees the loop's index as a value binder,
    /// so a bracket argument built from it (`g[i]()`) resolves.
    fn loop_body(
        &mut self,
        iteration: &mojito_checked::checked::ComptimeIteration,
        binding: Option<mojito_types::origin::OwnerId>,
        body: &'a hir::HirRegion,
    ) -> Vec<MirBlock> {
        let outer = self.enclosing_binders.clone();
        self.enclosing_binders = outer.with_loops(&[LoopIndex::of(iteration, binding)]);
        self.comptime_thunks.enter_scope(&self.enclosing_binders);
        let blocks = self.hir_region(body);
        self.enclosing_binders = outer;
        blocks
    }

    /// Lower HIR region `region` to a block list of its own, its block ids
    /// local (entry 0), sharing this function's registers and slots.
    pub(super) fn hir_region(&mut self, region: &'a hir::HirRegion) -> Vec<MirBlock> {
        let graph = &self.cfg.g;
        let placeholders = region
            .blocks
            .iter()
            .map(|_| MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Unreachable,
            })
            .collect();
        let outer = std::mem::replace(&mut self.f.blocks, placeholders);
        let cur = self.cur;
        let reference = self.setup_reference.take();
        for (index, block) in region.blocks.iter().enumerate() {
            self.cur = index;
            let block = &graph[*block];
            for instr in &block.instrs {
                self.lower_instr(instr);
            }
            let term = block.term.as_ref().map_or_else(
                || MirTerm::TryYield(Vec::new()),
                |term| self.lower_term(term),
            );
            self.f.blocks[self.cur].term = term;
        }
        self.setup_reference = reference;
        self.cur = cur;
        std::mem::replace(&mut self.f.blocks, outer)
    }

    /// Lower a HIR block terminator; the branch/return operands are flattened into
    /// `self.cur` first, then the `MirTerm` references their result registers.
    pub(super) fn lower_term(&mut self, t: &'a Terminator) -> MirTerm {
        match t {
            Terminator::Return(expression) => {
                let value = expression.as_ref().map(|e| self.lower_return_value(e));
                self.flush_argument_anchors();
                MirTerm::Return(value)
            }
            Terminator::ReturnWithCleanup { value, cleanup } => {
                // Preserve source evaluation order: materialize the return value
                // before destroying loop-owned iterators. In particular, `return
                // item^` must transfer the yielded element before the iterator's
                // residual storage is released.
                let value = value.as_ref().map(|e| self.lower_return_value(e));
                self.flush_argument_anchors();
                for var in cleanup {
                    // Keep the loop-owned root live into this return block.
                    // Without this marker, edge-based last-use elaboration can
                    // destroy a loop iterator at block entry (before the return
                    // value and the current yielded element have been handled).
                    // Drop elaboration ends it after the marker, and a
                    // `finally` copied before this return runs before it.
                    self.emit(MirInstr::KeepAlive { var: *var });
                }
                MirTerm::Return(value)
            }
            Terminator::TryYield => MirTerm::TryYield(Vec::new()),
            Terminator::Yield => MirTerm::Yield(Vec::new()),
            Terminator::ComptimeYield => MirTerm::ComptimeYield(Vec::new()),
            Terminator::ElifCond(cond) => {
                let cond = self.condition_hir(cond);
                self.flush_argument_anchors();
                MirTerm::ElifCond {
                    cond,
                    values: Vec::new(),
                }
            }
            Terminator::Break(label) => MirTerm::Break {
                label: Some(LoopLabel(*label)),
                values: Vec::new(),
            },
            Terminator::Continue(label) => MirTerm::Continue {
                label: Some(LoopLabel(*label)),
                values: Vec::new(),
            },
            Terminator::ComptimeBreak => MirTerm::ComptimeBreak(Vec::new()),
            Terminator::ComptimeContinue => MirTerm::ComptimeContinue(Vec::new()),
            Terminator::MatchNext => MirTerm::MatchNext(Vec::new()),
            Terminator::MatchComplete => MirTerm::MatchComplete(Vec::new()),
            Terminator::Raise(value) => {
                let src = self.expr_hir(value);
                // `raise "message"` raises the `Error` the literal converts
                // to, as `raise Error("message")` does.
                let src =
                    if self.f.reg_types.get(&src.0).map(Ty::kind) == Some(&TyKind::StringLiteral) {
                        let error =
                            self.fresh_typed(self.hir_node(value).location(), None, Ty::error());
                        self.emit(error_conversion(error, src));
                        error
                    } else {
                        src
                    };
                self.flush_argument_anchors();
                let (instrs, term) = self.raise_exit(src);
                for instruction in instrs {
                    self.emit(instruction);
                }
                term
            }
            Terminator::Unreachable => MirTerm::Unreachable,
        }
    }

    /// `dest = source.__iter__()` through `selected`: an ordinary method
    /// call, followed by its error region when `__iter__` raises `error`, as
    /// upstream's `emitForStmt` calls `__iter__`, so a raise leaves through
    /// the call's raise edge. A borrowed source is called on the place its
    /// handle borrows; an owned one is moved into the consuming receiver.
    fn call_iter(
        &mut self,
        source: VarId,
        dest: VarId,
        borrowed_place: Option<MirPlace>,
        protocol: &mojito_checked::checked::IterationProtocol,
        selected: &str,
        error: Option<&Ty>,
    ) {
        let span = SourceSpan::new(None, DUMMY_SPAN);
        let owned = protocol.mode == mojito_checked::checked::IterationMode::Owned;
        let receiver_ty = self.var_types.get(&source).cloned();
        let recv_place = (!owned).then(|| {
            borrowed_place.map_or_else(
                || MirPlace::root(source, receiver_ty.clone()),
                |mut place| {
                    place.through = Some(source);
                    place
                },
            )
        });
        let recv = self.fresh(span.clone(), Some(source));
        match &recv_place {
            Some(place) => self.emit(MirInstr::LoadPlace {
                dest: recv,
                place: place.clone(),
            }),
            None => self.emit(MirInstr::UseVar {
                dest: recv,
                var: source,
                mode: UseMode::Move,
                constructor: None,
            }),
        }
        let iterator = self.fresh_typed(span.clone(), Some(dest), protocol.iterator_ty.clone());
        self.record_call(iterator, Some(selected.to_string()), 0);
        let raises = self.raises(error.cloned(), span);
        self.emit_call(MirInstr::MethodCall {
            dest: iterator,
            recv,
            method: "__iter__".to_string(),
            resolved: Some(selected.to_string()),
            raises: raises.clone(),
            reference_result: None,
            result_adapter: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_place: recv_place.map(Box::new),
            recv_writes: owned,
            receiver_convention: owned.then_some(ArgConvention::Var),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
            refined_bound: None,
        });
        self.error_region(raises);
        self.emit(MirInstr::DefVar {
            var: dest,
            src: iterator,
            binding_ty: Some(protocol.iterator_ty.clone()),
            in_place: false,
        });
    }

    /// `raw = iter.__next__()` over a typed-raising iterator: the call on
    /// the iterator slot, its error region raising to the enclosing `try`,
    /// and the element stored into the yield slot `raw`, typed `raw_ty`, as
    /// upstream's `emitCallInTryBlock` stores the call's result into a
    /// temporary inside its `lit.try`.
    pub(super) fn call_raising_next(
        &mut self,
        iter: VarId,
        (raw, raw_ty): (VarId, &Ty),
        call: &mojito_checked::checked::CheckedIteratorCall,
        error: &Ty,
    ) {
        let span = SourceSpan::new(None, DUMMY_SPAN);
        let place = MirPlace::root(iter, self.var_types.get(&iter).cloned());
        let recv = self.fresh(span.clone(), Some(iter));
        self.emit(MirInstr::LoadPlace {
            dest: recv,
            place: place.clone(),
        });
        let element = self.fresh_typed(span.clone(), Some(iter), call.result_ty.clone());
        self.record_call(element, Some(call.target.clone()), 0);
        let raises = self.raises(Some(error.clone()), span);
        self.emit_call(MirInstr::MethodCall {
            dest: element,
            recv,
            method: "__next__".to_string(),
            resolved: Some(call.target.clone()),
            raises: raises.clone(),
            reference_result: call.reference_result.clone().map(Box::new),
            result_adapter: call.result_adapter.map(MirResultAdapter::new),
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_place: Some(Box::new(place)),
            recv_writes: true,
            receiver_convention: Some(ArgConvention::Mut),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
            refined_bound: None,
        });
        self.error_region(raises);
        self.emit(MirInstr::DefVar {
            var: raw,
            src: element,
            binding_ty: Some(raw_ty.clone()),
            in_place: false,
        });
    }
}

/// The arguments of a slice or multi-subscript store target: a slice's
/// bound nodes, or a multi-subscript's checked arguments.
#[derive(Clone, Copy)]
enum SetArguments<'a> {
    Index(Node<'a>),
    Slice([Option<mojito_checked::checked::CheckedNodeId>; 3]),
    Multi(&'a [mojito_checked::checked::CheckedSubscriptArg]),
}

/// The loans a `ref` binding's own slot holds. Upstream's `ref` binding names
/// an origin rather than taking an exclusive borrow: it keeps its owner live
/// and carries the interior generation it designates, but excludes no access
/// to the owner and no other reference, so each loan is a `setup` loan. The
/// lowering facts a borrower built from the binding copies (`aggregate_loans`,
/// `aliases`) keep the exclusive form, so a view of the binding borrows the
/// owner as a view of the owner does.
fn reference_binding_loans(loans: &[MirLoan]) -> Vec<MirLoan> {
    loans
        .iter()
        .cloned()
        .map(|loan| MirLoan {
            setup: true,
            ..loan
        })
        .collect()
}
