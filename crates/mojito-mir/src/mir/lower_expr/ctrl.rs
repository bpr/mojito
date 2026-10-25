//! Control-flow expressions: short-circuit/ternary/compare chains,
//! collection plans, and comprehensions.

use super::expr::OperatorDispatch;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{
    CheckedComprehensionClause, CheckedExpr, CheckedOp, DictLiteral, SemanticAdjustment,
};

/// The construction the checker selected for display or comprehension
/// `node`. Collection syntax is deliberately absent from this decision:
/// lowering consumes the nominal plan just like any other resolved call.
pub(super) struct CollectionPlan<'n> {
    pub(super) target: &'n Ty,
    /// The exact lowered `__init__` the construction calls.
    pub(super) constructor: Option<&'n str>,
    /// The literal keyword `constructor` is called with, bound to `None`.
    pub(super) marker: Option<&'n str>,
    /// A comprehension's insertion method.
    pub(super) insert: Option<&'n str>,
}

/// Return the collection construction the checker selected for display or
/// comprehension `node`.
pub(super) fn collection_plan(node: &CheckedExpr) -> Option<CollectionPlan<'_>> {
    display_adjustments(node).find_map(|adjustment| match adjustment {
        SemanticAdjustment::ConstructCollection {
            target,
            insert,
            constructor,
            marker,
            ..
        } => Some(CollectionPlan {
            target,
            constructor: constructor.as_deref(),
            marker: marker.as_deref(),
            insert: insert.as_deref(),
        }),
        _ => None,
    })
}

/// Return the literal constructors the checker selected for dictionary
/// display `node`, when it is upstream's `__dict_literal__` construction.
pub(super) fn dict_literal_plan(node: &CheckedExpr) -> Option<&DictLiteral> {
    display_adjustments(node).find_map(|adjustment| match adjustment {
        SemanticAdjustment::ConstructCollection {
            literal: Some(literal),
            ..
        } => Some(literal.as_ref()),
        _ => None,
    })
}

/// Return the fixed-size array literal constructor the checker selected for
/// list display `node`: the concrete target type and the exact lowered
/// `__init__` overload symbol of the variadic literal constructor.
pub(super) fn array_literal_plan(node: &CheckedExpr) -> Option<(Ty, String)> {
    display_adjustments(node).find_map(|adjustment| match adjustment {
        SemanticAdjustment::ConstructArrayLiteral {
            target,
            constructor,
            ..
        } => Some((target.clone(), constructor.clone())),
        _ => None,
    })
}

impl<'a> Flatten<'a> {
    /// A condition whose value is a `Boolable` struct (checker-marked
    /// `Truthiness`) converts through `Bool(x)` — the same call `Bool(x)`
    /// spells, which both backends dispatch to the struct's `__bool__`.
    pub(in crate::mir) fn truthiness(&mut self, condition: Node<'a>, value: Reg) -> Reg {
        let boolable = condition.adjustments().iter().any(|adjustment| {
            matches!(
                adjustment,
                mojito_checked::checked::SemanticAdjustment::Truthiness
            )
        });
        if !boolable {
            return value;
        }
        self.bool_conversion(condition.location(), value)
    }

    /// The `Bool(x)` call both backends dispatch to a struct's `__bool__`.
    pub(in crate::mir) fn bool_conversion(&mut self, span: SourceSpan, value: Reg) -> Reg {
        let dest = self.fresh_typed(span, None, Ty::bool());
        self.emit(MirInstr::Call {
            dest,
            func: FuncRef::named("Bool"),
            raises: None,
            args: vec![value],
            kwargs: Vec::new(),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        });
        dest
    }

    /// Lower `a and b` / `a or b` as upstream's `hlcf.if` on `a`: for `and`
    /// the then arm evaluates and yields `b` and the else arm yields `a`;
    /// `or` swaps the arms. The right operand is evaluated only when needed,
    /// which matters for observable side effects and for ownership, where a
    /// moved operand on the not-taken side must not count as moved.
    pub(in crate::mir) fn short_circuit(
        &mut self,
        op: InfixOp,
        a: Node<'a>,
        b: Node<'a>,
        span: SourceSpan,
    ) -> Reg {
        let ra = self.expr(a);
        let rhs = self.region(|this| MirTerm::Yield(vec![this.expr(b)]));
        let lhs = self.region(|_| MirTerm::Yield(vec![ra]));
        let (then, orelse) = match op {
            InfixOp::And => (rhs, lhs),
            _ => (lhs, rhs),
        };
        let d = self.fresh(span, None);
        self.emit(MirInstr::If {
            results: vec![d],
            cond: ra,
            then,
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse,
        });
        d
    }

    /// Lower a ternary `then_e if cond else else_e` as upstream's `hlcf.if`
    /// on `cond`, or its `hlcf.comptime.if` when the checker found `cond` a
    /// parameter value, so the elaborator keeps the live arm alone. A result
    /// the checker proved register-passable is yielded by the arms as the
    /// op's value; any other is moved by each arm into a result slot, which
    /// the op's consumer receives as a forward, as upstream moves an arm's
    /// value into its destination buffer: a `^` arm by its own move, and any
    /// other (a temporary, or a place arm's copy) by a move out of the
    /// temporary that holds it.
    pub(in crate::mir) fn ternary(
        &mut self,
        node: Node<'a>,
        cond: Node<'a>,
        then_e: Node<'a>,
        else_e: Node<'a>,
    ) -> Reg {
        let sp = node.location();
        let test = if node.comptime_conditional() {
            ArmTest::Comptime(self.comptime_condition_of(cond))
        } else {
            let rc = self.expr(cond);
            ArmTest::Run(self.truthiness(cond, rc))
        };
        if node.register_passable_result() {
            let then = self.region(|this| test.yield_values(vec![this.expr(then_e)]));
            let orelse = self.region(|this| test.yield_values(vec![this.expr(else_e)]));
            let d = self.fresh(sp, None);
            self.emit(test.op(vec![d], then, orelse));
            return d;
        }
        let result = self.fresh_var();
        let arm = |this: &mut Self, value: Node<'a>| {
            this.region(|this| {
                let src = this.expr(value);
                let src = if value.transfer_operand().is_some() {
                    src
                } else {
                    this.move_out_of_temporary(value, src)
                };
                this.emit(MirInstr::DefVar {
                    var: result,
                    src,
                    binding_ty: None,
                    in_place: false,
                });
                test.yield_values(Vec::new())
            })
        };
        let then = arm(self, then_e);
        let orelse = arm(self, else_e);
        self.emit(test.op(Vec::new(), then, orelse));
        let d = self.fresh(sp, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Forward,
            constructor: None,
        });
        d
    }

    /// Hold `src`, the owned temporary `value` evaluated to, in a slot of
    /// its own and move it out, which runs its move constructor.
    fn move_out_of_temporary(&mut self, value: Node<'a>, src: Reg) -> Reg {
        let ty = self.f.reg_types.get(&src.0).cloned().or_else(|| value.ty());
        let slot = self.fresh_var();
        if let Some(ty) = ty.clone() {
            self.var_types.insert(slot, ty);
        }
        self.emit(MirInstr::DefVar {
            var: slot,
            src,
            binding_ty: ty.clone(),
            in_place: false,
        });
        let moved = self.fresh_typed(value.location(), Some(slot), ty.unwrap_or_else(Ty::error));
        self.emit(MirInstr::UseVar {
            dest: moved,
            var: slot,
            mode: UseMode::Move,
            constructor: None,
        });
        moved
    }

    /// Lower chained comparison `chain`, `a op1 b op2 c …`, to a `Bool`, as
    /// upstream's nested `hlcf.if`s: each link after the first is evaluated
    /// in the then arm of an `if` on the previous comparison, whose else arm
    /// yields that comparison, so a false link short-circuits the rest.
    /// Each operand is evaluated **once**, left to right. Each link is the
    /// call of the dunder the checker recorded for it, reading its operands
    /// in place at its own setup, as a lone operator does. Right after the
    /// first link, before its `if`, the first middle operand becomes an
    /// owned temporary for the second link (a place is copied, a temporary
    /// or `^` is moved), which dies after that link or on the first link's
    /// false arm. A later middle is read in place by both of its links; a
    /// temporary there is held in a slot likewise.
    pub(in crate::mir) fn compare_chain(
        &mut self,
        chain: Node<'a>,
        first: Node<'a>,
        rest: &[(InfixOp, Node<'a>)],
    ) -> Reg {
        let links = chain.compare_links();
        self.chain_link(chain, links, first, rest, 0, None)
    }

    /// Install the caller-side loans a callee's transfer effects imply at
    /// this call. The checker recorded the substituted sources per
    /// occurrence; the destination actual's root var receives the merged
    /// `EstablishLoans`, so ownership analysis sees mutation conflicts and
    /// the drops pass keeps the loan roots alive while the carrier lives.
    pub(in crate::mir) fn install_call_transfers(
        &mut self,
        call: Node<'a>,
        recv_place: Option<&MirPlace>,
        arg_places: &[Option<MirPlace>],
    ) {
        fn flatten(
            origin: &mojito_types::origin::Origin,
            out: &mut Vec<mojito_types::origin::OriginPlace>,
        ) {
            match origin {
                mojito_types::origin::Origin::Place(place) => out.push(place.clone()),
                mojito_types::origin::Origin::Union(origins) => {
                    for origin in origins {
                        flatten(origin, out);
                    }
                }
                _ => {}
            }
        }

        for transfer in &call.checked.call_transfers {
            let dest_root = match transfer.dest {
                mojito_checked::checked::CheckedTransferDest::Receiver => {
                    recv_place.map(|place| place.root)
                }
                mojito_checked::checked::CheckedTransferDest::Argument(index) => arg_places
                    .get(index)
                    .and_then(|place| place.as_ref())
                    .map(|place| place.root),
                // A captured owner resolves only in the frame that owns the
                // storage; elsewhere the verbatim-propagated effect covers it.
                mojito_checked::checked::CheckedTransferDest::Owner(owner) => {
                    self.owner_vars.get(&owner).copied()
                }
            };
            let Some(dest_root) = dest_root else {
                continue;
            };
            // A destination rooted at one of THIS function's parameters is
            // covered transitively: the enclosing callable's derived effect
            // installs the loan at ITS caller, where the storage actually
            // lives. Installing here would also read source vars the callee
            // may already have moved.
            if (dest_root as usize) < self.f.n_params {
                continue;
            }
            let mut places = Vec::new();
            for origin in &transfer.sources {
                flatten(origin, &mut places);
            }
            // Merge with the destination DOMAIN's existing loans: a second
            // transfer (a loop iteration, another append) extends that
            // generation's loan set rather than replacing it, while sibling
            // interior domains keep independent generations.
            let dest_interior = (!transfer.dest_path.is_empty()).then(|| MirInteriorOrigin {
                root: dest_root,
                path: transfer.dest_path.clone(),
            });
            let mut loans = match &dest_interior {
                Some(domain) => self
                    .transfer_domain_loans
                    .get(&(dest_root, domain.path.clone()))
                    .cloned()
                    .unwrap_or_default(),
                None => self
                    .aggregate_loans
                    .get(&dest_root)
                    .cloned()
                    .unwrap_or_default(),
            };
            let before = loans.len();
            for origin in places {
                let Some(canonical) = self.mir_interior_origin(&origin, None) else {
                    continue;
                };
                if loans.iter().any(|loan| {
                    loan.place.root == canonical.root
                        && loan.interior.as_ref().map(|interior| &interior.path)
                            == Some(&canonical.path)
                }) || loans
                    .iter()
                    .any(|loan| loan.place.root == canonical.root && loan.interior.is_none())
                {
                    continue;
                }
                let place =
                    MirPlace::root(canonical.root, self.var_types.get(&canonical.root).cloned());
                let interior = canonical
                    .path
                    .iter()
                    .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
                    .then_some(canonical);
                loans.push(MirLoan {
                    place,
                    mutable: transfer.mutable,
                    interior,
                    shared: false,
                    setup: false,
                });
            }
            if loans.is_empty() || loans.len() == before {
                continue;
            }
            let marker = self.fresh_typed(call.location(), Some(dest_root), Ty::none());
            match &dest_interior {
                Some(domain) => {
                    self.transfer_domain_loans
                        .insert((dest_root, domain.path.clone()), loans.clone());
                }
                None => {
                    self.aggregate_loans.insert(dest_root, loans.clone());
                }
            }
            self.emit(MirInstr::EstablishLoans {
                reference: dest_root,
                loans,
                marker,
                dest_interior,
            });
        }
    }

    /// Construct an empty nominal collection by `constructor`, the empty
    /// `__init__` its check selected, and bind it to a synthetic slot so
    /// each checked mutating insertion can use the ordinary method-call ABI.
    pub(in crate::mir) fn begin_nominal_collection(
        &mut self,
        expression: Node<'a>,
        target: &Ty,
        constructor: &str,
    ) -> VarId {
        let empty = self.constructor_call(
            expression,
            constructor,
            target.clone(),
            Vec::new(),
            Vec::new(),
        );
        let collection = self.fresh_var();
        self.var_types.insert(collection, target.clone());
        self.emit(MirInstr::DefVar {
            var: collection,
            src: empty,
            binding_ty: Some(target.clone()),
            in_place: false,
        });
        collection
    }

    /// Execute one checked append/add/setitem operation on a synthetic nominal
    /// collection slot.  Borrowing the receiver avoids invoking its copy
    /// constructor; a `mut self` implementation commits through `recv_place`.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the callee needs the Vec; `&[Reg]` would add an allocation"
    )]
    pub(in crate::mir) fn insert_nominal_collection(
        &mut self,
        expression: Node<'a>,
        collection: VarId,
        target: &Ty,
        resolved: &str,
        args: Vec<Reg>,
    ) {
        let method = mojito_symbol::symbol::split_method_symbol(resolved)
            .map_or(resolved, |(_, method)| method)
            .to_string();
        let recv = self.fresh_typed(expression.location(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: recv,
            var: collection,
            mode: UseMode::BorrowMut,
            constructor: None,
        });
        let dest = self.fresh_typed(expression.location(), None, Ty::none());
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved: Some(resolved.to_string()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args: args.clone(),
            kwargs: Vec::new(),
            recv_place: Some(Box::new(MirPlace::root(collection, Some(target.clone())))),
            recv_writes: true,
            receiver_convention: Some(mojito_ast::ast::ArgConvention::Mut),
            arg_places: vec![None; args.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
        });
    }

    pub(in crate::mir) fn finish_nominal_collection(
        &mut self,
        expression: Node<'a>,
        collection: VarId,
        target: &Ty,
    ) -> Reg {
        let result = self.fresh_typed(expression.location(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: result,
            var: collection,
            mode: UseMode::Move,
            constructor: None,
        });
        result
    }

    /// Lower comprehension clauses directly into MIR control flow. This is the
    /// same left-to-right nesting as an explicit series of `for`/`if` blocks;
    /// the final leaf performs the collection family's insertion protocol.
    pub(in crate::mir) fn comprehension_clauses(
        &mut self,
        plan: &ComprehensionPlan<'_, 'a>,
        index: usize,
    ) {
        if index == plan.clauses.len() {
            // A dictionary comprehension evaluates each entry's key, then its
            // value, as Mojo does. List/set leaves evaluate one item.
            let key = plan.key.map(|expression| self.expr(expression));
            let value_reg = self.expr(plan.value);
            let mut arguments = Vec::with_capacity(1 + usize::from(key.is_some()));
            if let Some(key) = key {
                arguments.push(key);
            }
            arguments.push(value_reg);
            self.insert_nominal_collection(
                plan.value,
                plan.collection,
                plan.target,
                plan.insert,
                arguments,
            );
            return;
        }

        match &plan.clauses[index] {
            CheckedComprehensionClause::If(condition) => {
                let condition = plan.operand(*condition);
                let value = self.expr(condition);
                let condition = self.truthiness(condition, value);
                let then = self.region(|this| {
                    this.comprehension_clauses(plan, index + 1);
                    MirTerm::Yield(Vec::new())
                });
                self.emit(MirInstr::If {
                    results: Vec::new(),
                    cond: condition,
                    then,
                    elifs: Vec::new(),
                    else_args: Vec::new(),
                    orelse: vec![MirBlock {
                        instrs: Vec::new(),
                        term: MirTerm::Yield(Vec::new()),
                    }],
                });
            }
            CheckedComprehensionClause::For {
                var, iter, unpack, ..
            } => {
                let iter = plan.operand(*iter);
                let iterator_name = format!("$compiter{}", self.vars.len());
                let iterator = self.var(&iterator_name);
                let iterator_ty = iter.ty();
                let protocol = iter
                    .adjustments()
                    .iter()
                    .find_map(|adjustment| match adjustment {
                        SemanticAdjustment::Iterate(protocol) => Some(protocol.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| mojito_checked::checked::IterationProtocol {
                        mode: if matches!(iter.op(), CheckedOp::Transfer(_)) {
                            mojito_checked::checked::IterationMode::Owned
                        } else {
                            mojito_checked::checked::IterationMode::Borrowed
                        },
                        binding: None,
                        borrowed_origin: None,
                        yield_interior: Vec::new(),
                        source_retained: true,
                        prepare: None,
                        prepare_raises: None,
                        iterator_ty: iterator_ty.clone().unwrap_or_else(Ty::error),
                        next: None,
                        exhaustion: None,
                    });
                if let Some(origin) = &protocol.borrowed_origin {
                    let place = self.place(iter);
                    let value_ty = iterator_ty
                        .or_else(|| place.ty.clone())
                        .expect("checked borrowed comprehension iterable has a type");
                    self.borrow_iteration_source(
                        iterator,
                        iter.location(),
                        place,
                        value_ty,
                        origin,
                        false,
                    );
                } else {
                    if let Some(ty) = iterator_ty.clone() {
                        self.var_types.insert(iterator, ty);
                    }
                    let iterator_value = self.expr(iter);
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty,
                        in_place: false,
                    });
                }
                // The same source/iterator-object slot split as the
                // statement path: the iterator is a value of its own type in
                // its own slot, so `GetIter` normalization cannot clobber the
                // storage (or the reference handle) a borrowing iterator
                // still refers to, and an owned one empties the source.
                let borrowed = protocol.borrowed_origin.is_some();
                let split_source = borrowed || protocol.prepare.is_some();
                let iterator_object = if split_source {
                    let object = self.var(&format!("$compiterobj{}", self.vars.len()));
                    self.var_types.insert(object, protocol.iterator_ty.clone());
                    object
                } else {
                    iterator
                };
                self.normalize_iterator(iterator, iterator_object, &protocol);

                let binding_index = plan.clauses[..index]
                    .iter()
                    .filter(|clause| matches!(clause, CheckedComprehensionClause::For { .. }))
                    .count();
                let binding = plan
                    .bindings
                    .get(binding_index)
                    .expect("checked comprehension binder metadata");
                // `emitForStmt` after `LowerSemanticCF`: a raising `__next__`
                // in a `try` whose handler breaks, or `__has_next__`, an `if`
                // that breaks when it fails, and `__next__`; then the body
                // and `continue`.
                let label = Some(self.loop_label());
                let body = self.region(|this| {
                    let binding_var = this.var(&format!("$comp{}${}", var, binding.owner.0));
                    // Retain the raw `__next__` result in a compiler-owned slot, then
                    // adapt it to the comprehension target with the same checked
                    // matrix the statement path uses via `BindIteration`.
                    let raw_var = this.var(&format!("$compyield{}${}", var, binding.owner.0));
                    this.var_types
                        .insert(raw_var, binding.plan.yielded_ty.clone());
                    let raw = (raw_var, &binding.plan.yielded_ty);
                    match (&protocol.exhaustion, protocol.next.as_deref()) {
                        (Some(error), Some(call)) => {
                            this.raising_next_try(iterator_object, raw, call, error, label);
                        }
                        _ => this.next_or_break(iterator_object, raw, label, iter.location()),
                    }
                    this.bind_iteration_result(
                        &binding.plan,
                        raw_var,
                        binding_var,
                        iterator_object,
                        binding.owner,
                    );
                    // A destructuring target distributes the binder to its
                    // leaves, as the statement `target = binder` would.
                    if let Some(unpack) = unpack {
                        let places = unpack
                            .target
                            .expressions()
                            .into_iter()
                            .map(|id| plan.operand(id))
                            .collect::<Vec<_>>();
                        let value = plan.operand(unpack.value);
                        this.with_statement_loans(|this| {
                            this.lower_destructure(&unpack.target, &places, value);
                        });
                    }
                    this.comprehension_clauses(plan, index + 1);
                    MirTerm::Continue {
                        label,
                        values: Vec::new(),
                    }
                });
                self.emit(MirInstr::Loop {
                    label,
                    operands: Vec::new(),
                    args: Vec::new(),
                    results: Vec::new(),
                    body,
                });
                if split_source
                    && !borrowed
                    && protocol.source_retained
                    && protocol.mode == mojito_checked::checked::IterationMode::Borrowed
                {
                    // An owned temporary source is used only by `GetIter` before
                    // the loop, so when the iterator may refer to its storage a
                    // liveness anchor at the exit keeps it live through the loop,
                    // and elaboration destroys it exactly once, after the loop. A source
                    // the iterator cannot refer to dies right after `GetIter`. A
                    // borrowed source is kept live by its loan and owned (dropped)
                    // by its enclosing scope. Mirrors the statement loop's exit
                    // anchor.
                    self.emit(MirInstr::KeepAlive { var: iterator });
                }
            }
        }
    }

    pub(in crate::mir) fn comprehension(
        &mut self,
        node: Node<'a>,
        clauses: &[CheckedComprehensionClause],
        key: Option<Node<'a>>,
        value: Node<'a>,
    ) -> Reg {
        let Some(CollectionPlan {
            target,
            constructor: Some(constructor),
            insert: Some(insert),
            ..
        }) = collection_plan(node.checked)
        else {
            return self.unsupported_value(
                node.location(),
                "a comprehension without its checked construction reached MIR",
            );
        };
        let collection = self.begin_nominal_collection(node, target, constructor);
        let plan = ComprehensionPlan {
            collection,
            target,
            insert,
            key,
            value,
            clauses,
            node,
            bindings: &node.checked.comprehension_bindings,
        };
        self.comprehension_clauses(&plan, 0);
        self.finish_nominal_collection(node, collection, target)
    }

    /// Lower `build` as a region of its own: one block, ended by the
    /// terminator `build` returns, sharing this frame's registers and slots.
    /// An enclosing operand's pending setup read is set aside while it
    /// builds, so a read never starts inside the region. A temporary an
    /// argument of the region makes dies in the region, as upstream's does:
    /// its anchor's `lifetime.keep_alive` ends the region's block.
    fn region(&mut self, build: impl FnOnce(&mut Self) -> MirTerm) -> Vec<MirBlock> {
        let outer = std::mem::replace(
            &mut self.f.blocks,
            vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Unreachable,
            }],
        );
        let cur = std::mem::replace(&mut self.cur, 0);
        let reference = self.setup_reference.take();
        let anchors = std::mem::take(&mut self.pending_argument_anchors);
        let setups = self.call_setups.len();
        let term = build(self);
        debug_assert_eq!(self.call_setups.len(), setups);
        self.flush_argument_anchors();
        self.pending_argument_anchors = anchors;
        self.f.blocks[self.cur].term = term;
        self.setup_reference = reference;
        self.cur = cur;
        std::mem::replace(&mut self.f.blocks, outer)
    }

    /// A fresh label for a loop of this function, unique beside HIR's.
    const fn loop_label(&mut self) -> LoopLabel {
        self.next_label += 1;
        LoopLabel(self.next_label - 1)
    }

    /// A comprehension loop's step over a typed-raising iterator: `raw =
    /// iter.__next__()` in a `try` whose handler breaks out of loop
    /// `label`, as upstream's `emitForStmt` calls it.
    fn raising_next_try(
        &mut self,
        iter: VarId,
        raw: (VarId, &Ty),
        call: &mojito_checked::checked::CheckedIteratorCall,
        error: &Ty,
        label: Option<LoopLabel>,
    ) {
        let try_label = TryLabel(self.next_try);
        self.next_try += 1;
        let enclosing = self.raise_target.replace(try_label);
        let body = self.region(|this| {
            this.call_raising_next(iter, raw, call, error);
            MirTerm::TryYield(Vec::new())
        });
        self.raise_target = enclosing;
        let handler = vec![MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Break {
                label,
                values: Vec::new(),
            },
        }];
        self.emit(MirInstr::Try {
            label: try_label,
            results: Vec::new(),
            body,
            handler_args: Vec::new(),
            handler: Some((None, handler)),
            else_args: Vec::new(),
            orelse: None,
        });
    }

    /// A comprehension loop's step over compiler-private iterator storage:
    /// break out of loop `label` unless `iter` has another element, then
    /// store it into `raw`.
    fn next_or_break(
        &mut self,
        iter: VarId,
        (raw, raw_ty): (VarId, &Ty),
        label: Option<LoopLabel>,
        location: SourceSpan,
    ) {
        let has_next = self.fresh(location.clone(), Some(iter));
        self.emit(MirInstr::HasNext {
            dest: has_next,
            iter,
        });
        self.emit(MirInstr::If {
            results: Vec::new(),
            cond: has_next,
            then: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Yield(Vec::new()),
            }],
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Break {
                    label,
                    values: Vec::new(),
                },
            }],
        });
        let element = self.fresh_typed(location, Some(iter), raw_ty.clone());
        self.emit(MirInstr::Next {
            dest: element,
            iter,
        });
        self.emit(MirInstr::DefVar {
            var: raw,
            src: element,
            binding_ty: Some(raw_ty.clone()),
            in_place: false,
        });
    }

    /// What a chain's next link reads for middle operand `operand`, whose
    /// link just read it as `value`. The first middle becomes an owned
    /// temporary (a place copied, a temporary or `^` taken as is); a later
    /// middle place stays where it lies, and a named one is read there again
    /// at the next link, so its owner lives through that link. An owned
    /// temporary of a type with a destructor lives in a hidden slot, so it
    /// is destroyed after its last link or on a false edge.
    fn chain_middle(&mut self, operand: Node<'a>, value: Reg, first: bool) -> ChainMiddle<'a> {
        let place = matches!(
            operand.op(),
            CheckedOp::Identifier(_) | CheckedOp::Member { .. } | CheckedOp::Index { .. }
        ) && !operand.is_fresh_temporary();
        if place && !first {
            return if is_field_chain(operand) {
                ChainMiddle::Place(operand)
            } else {
                ChainMiddle::Read(value)
            };
        }
        let ty = self
            .f
            .reg_types
            .get(&value.0)
            .cloned()
            .or_else(|| operand.checked.ty.clone());
        let Some(ty) = ty.filter(crate::mir::calls::owns_droppable_storage) else {
            return ChainMiddle::Read(value);
        };
        let owned = if place {
            // A whole slot the first link read is read again for the copy,
            // so the copy reads its source's memory as upstream's does, and
            // copy elision sees the source's death right after it.
            let source = self.f.blocks[self.cur]
                .instrs
                .iter()
                .rev()
                .find_map(|instruction| match instruction {
                    MirInstr::LoadPlace { dest, place }
                        if *dest == value && place.proj.is_empty() && place.through.is_none() =>
                    {
                        Some(place.clone())
                    }
                    _ => None,
                });
            let copied_from = match source {
                Some(place) => {
                    let reread = self.fresh_typed(operand.location(), Some(place.root), ty.clone());
                    self.emit(MirInstr::LoadPlace {
                        dest: reread,
                        place,
                    });
                    reread
                }
                None => value,
            };
            let copied = self.fresh_typed(operand.location(), None, ty.clone());
            self.emit(MirInstr::CopyValue {
                dest: copied,
                value: copied_from,
                constructor: None,
            });
            copied
        } else {
            value
        };
        let variable = self.var(&format!("$chain_middle_r{}", owned.0));
        self.var_types.insert(variable, ty.clone());
        self.emit(MirInstr::DefVar {
            var: variable,
            src: owned,
            binding_ty: Some(ty.clone()),
            in_place: false,
        });
        ChainMiddle::Slot(variable, ty)
    }

    /// Lower link `index` of chain `chain` and every link after it, the
    /// left operand read from `prev` (`first` for the first link): the
    /// comparison, and for a link before the last an `if` on it whose then
    /// arm yields the rest of the chain and whose else arm yields it.
    fn chain_link(
        &mut self,
        chain: Node<'a>,
        links: &[mojito_checked::checked::CheckedCompareLink],
        first: Node<'a>,
        rest: &[(InfixOp, Node<'a>)],
        index: usize,
        prev: Option<ChainMiddle<'a>>,
    ) -> Reg {
        let sp = chain.location();
        let (op, operand) = rest[index];
        self.open_call_setup();
        let left = match prev {
            None => self.lower_operator_operand(chain, first),
            Some(ChainMiddle::Read(value)) => value,
            Some(ChainMiddle::Place(place)) => self.lower_operator_operand(chain, place),
            Some(ChainMiddle::Slot(variable, ty)) => {
                let read = self.fresh_typed(sp.clone(), Some(variable), ty.clone());
                self.emit(MirInstr::LoadPlace {
                    dest: read,
                    place: MirPlace::root(variable, Some(ty)),
                });
                read
            }
        };
        let cur = self.lower_operator_operand(chain, operand);
        self.close_call_setup(None);
        let link = links.get(index);
        let resolved = link.and_then(|link| link.target.clone());
        // A link records no effect of its own: a dispatched link raises
        // what the chain does.
        let dispatch = OperatorDispatch {
            raises: resolved.as_ref().and_then(|_| chain.raises()),
            resolved,
            negated_equality: link.is_some_and(|link| link.negated_equality),
        };
        let cmp = self.emit_comparison(sp.clone(), op, left, cur, dispatch);
        if index + 1 == rest.len() {
            return cmp;
        }
        let middle = self.chain_middle(operand, cur, index == 0);
        let then = self.region(|this| {
            MirTerm::Yield(vec![this.chain_link(
                chain,
                links,
                first,
                rest,
                index + 1,
                Some(middle),
            )])
        });
        let orelse = self.region(|_| MirTerm::Yield(vec![cmp]));
        let d = self.fresh(sp, None);
        self.emit(MirInstr::If {
            results: vec![d],
            cond: cmp,
            then,
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse,
        });
        d
    }
}

/// What selects a ternary's arm: a run-time `Bool` register (`hlcf.if`),
/// or a parameter condition the elaborator decides (`hlcf.comptime.if`).
enum ArmTest {
    Run(Reg),
    Comptime(mojito_types::types::GenericConstraint),
}

impl ArmTest {
    /// The terminator ending an arm that yields `values`.
    const fn yield_values(&self, values: Vec<Reg>) -> MirTerm {
        match self {
            Self::Run(_) => MirTerm::Yield(values),
            Self::Comptime(_) => MirTerm::ComptimeYield(values),
        }
    }

    /// The two-armed op selecting `then` or `orelse`, defining `results`.
    fn op(self, results: Vec<Reg>, then: Vec<MirBlock>, orelse: Vec<MirBlock>) -> MirInstr {
        match self {
            Self::Run(cond) => MirInstr::If {
                results,
                cond,
                then,
                elifs: Vec::new(),
                else_args: Vec::new(),
                orelse,
            },
            Self::Comptime(cond) => MirInstr::ComptimeIf {
                results,
                conds: vec![cond],
                arms: vec![then],
                orelse,
            },
        }
    }
}

/// What a chained comparison's next link reads as its left operand.
enum ChainMiddle<'a> {
    /// The register the previous link read.
    Read(Reg),
    /// A named place (`x`, `x.f`) the next link reads where it lies.
    Place(Node<'a>),
    /// The hidden slot holding the first middle's owned temporary, of its
    /// type, read at the next link's setup.
    Slot(VarId, Ty),
}

/// The checked adjustments of a collection display. A local `comptime`
/// binding's display carries its construction under the binding's record.
fn display_adjustments(node: &CheckedExpr) -> impl Iterator<Item = &SemanticAdjustment> {
    node.adjustments.iter().map(|adjustment| match adjustment {
        SemanticAdjustment::ComptimeDisplay { construction, .. } => &**construction,
        other => other,
    })
}

/// Whether `node` is a named place, `x` or a field chain off one (`x.f.g`),
/// which reading again evaluates nothing.
fn is_field_chain(node: Node<'_>) -> bool {
    match node.op() {
        CheckedOp::Identifier(_) => true,
        CheckedOp::Member { .. } => node.member_object().is_some_and(is_field_chain),
        _ => false,
    }
}
