//! Expression lowering: short-circuit/ternary/compare chains, collection and
//! comprehension construction, nested-closure emission, and the `expr_unconverted`
//! expression dispatcher.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::*;

impl Flatten<'_> {
    /// Lower `a and b` / `a or b` into control flow so the right operand is only
    /// evaluated when needed (Python/Mojo short-circuit semantics). The result is
    /// carried in a synthetic variable across the branch and read back in the
    /// merge block. (Preserving the short-circuit — vs an eager `BinOp` — matters
    /// both for observable side effects and for Stage 6 ownership, where a moved
    /// operand on the not-taken side must not count as moved.)
    pub(super) fn short_circuit(
        &mut self,
        op: InfixOp,
        a: &Expr,
        b: &Expr,
        span: SourceSpan,
    ) -> Reg {
        let ra = self.expr(a);
        let result = self.fresh_var();
        // Seed the result with the left operand's value: for `and` a false `ra`
        // is the answer; for `or` a true `ra` is. The rhs block overwrites it.
        self.emit(MirInstr::DefVar {
            var: result,
            src: ra,
            binding_ty: None,
        });

        let rhs_blk = self.new_block();
        let merge_blk = self.new_block();
        // `and`: evaluate rhs only when `ra` is true; `or`: only when false.
        let (then_b, else_b) = match op {
            InfixOp::And => (rhs_blk, merge_blk),
            _ => (merge_blk, rhs_blk),
        };
        self.f.blocks[self.cur].term = MirTerm::Branch {
            cond: ra,
            then_b,
            else_b,
        };

        self.cur = rhs_blk;
        let rb = self.expr(b); // may itself split blocks (nested and/or)
        self.emit(MirInstr::DefVar {
            var: result,
            src: rb,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);

        self.cur = merge_blk;
        let d = self.fresh(span, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Lower a ternary `then_e if cond else else_e` to a value: branch on `cond`,
    /// each arm writing the result variable, then read it at the merge.
    pub(super) fn ternary(
        &mut self,
        cond: &Expr,
        then_e: &Expr,
        else_e: &Expr,
        sp: SourceSpan,
    ) -> Reg {
        let rc = self.expr(cond);
        let result = self.fresh_var();
        let then_blk = self.new_block();
        let else_blk = self.new_block();
        let merge_blk = self.new_block();
        self.f.blocks[self.cur].term = MirTerm::Branch {
            cond: rc,
            then_b: then_blk,
            else_b: else_blk,
        };
        self.cur = then_blk;
        let rt = self.expr(then_e);
        self.emit(MirInstr::DefVar {
            var: result,
            src: rt,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
        self.cur = else_blk;
        let re = self.expr(else_e);
        self.emit(MirInstr::DefVar {
            var: result,
            src: re,
            binding_ty: None,
        });
        self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
        self.cur = merge_blk;
        let d = self.fresh(sp, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Lower a chained comparison `a op1 b op2 c …` to a `Bool`. Each operand is
    /// evaluated **once**, left to right; a false link short-circuits the rest (the
    /// remaining operands are not evaluated). The result variable holds the last
    /// comparison evaluated (which is `false` on the link that failed).
    pub(super) fn compare_chain(
        &mut self,
        first: &Expr,
        rest: &[(InfixOp, Expr)],
        sp: SourceSpan,
    ) -> Reg {
        let result = self.fresh_var();
        let merge_blk = self.new_block();
        let mut prev = self.expr(first);
        for (i, (op, operand)) in rest.iter().enumerate() {
            let cur = self.expr(operand);
            let cmp = self.fresh(sp.clone(), None);
            self.emit(MirInstr::BinOp {
                op: *op,
                dest: cmp,
                a: prev,
                b: cur,
                resolved: None,
            });
            self.emit(MirInstr::DefVar {
                var: result,
                src: cmp,
                binding_ty: None,
            });
            if i + 1 == rest.len() {
                self.f.blocks[self.cur].term = MirTerm::Jump(merge_blk);
            } else {
                // A false link is the answer (result is already it); a true link
                // continues to the next comparison.
                let next_blk = self.new_block();
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: cmp,
                    then_b: next_blk,
                    else_b: merge_blk,
                };
                self.cur = next_blk;
                prev = cur;
            }
        }
        self.cur = merge_blk;
        let d = self.fresh(sp, None);
        self.emit(MirInstr::UseVar {
            dest: d,
            var: result,
            mode: UseMode::Copy,
        });
        d
    }

    /// Return the collection constructor and insertion protocol selected by the
    /// checker.  Collection syntax is deliberately absent from this decision:
    /// lowering consumes the nominal plan just like any other resolved call.
    pub(super) fn collection_plan(&self, expression: &Expr) -> Option<(Ty, Option<String>)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ConstructCollection { target, insert } => {
                    Some((target, insert))
                }
                _ => None,
            })
    }

    /// Construct an empty nominal collection and bind it to a synthetic slot so
    /// each checked mutating insertion can use the ordinary method-call ABI.
    pub(super) fn begin_nominal_collection(&mut self, expression: &Expr, target: &Ty) -> VarId {
        let Ty::Struct(name, _) = target else {
            unreachable!("checked collection target is nominal")
        };
        let empty = self.fresh_typed(expression.source_span(), None, target.clone());
        self.emit(MirInstr::Call {
            dest: empty,
            func: FuncRef::named(name),
            raises: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
        });
        let collection = self.fresh_var();
        self.var_types.insert(collection, target.clone());
        self.emit(MirInstr::DefVar {
            var: collection,
            src: empty,
            binding_ty: Some(target.clone()),
        });
        collection
    }

    /// Execute one checked append/add/setitem operation on a synthetic nominal
    /// collection slot.  Borrowing the receiver avoids invoking its copy
    /// constructor; a `mut self` implementation commits through `recv_place`.
    pub(super) fn insert_nominal_collection(
        &mut self,
        expression: &Expr,
        collection: VarId,
        target: &Ty,
        resolved: &str,
        args: Vec<Reg>,
    ) {
        let method = resolved
            .rsplit_once('.')
            .map_or(resolved, |(_, method)| method)
            .to_string();
        let recv = self.fresh_typed(expression.source_span(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: recv,
            var: collection,
            mode: UseMode::BorrowMut,
        });
        let dest = self.fresh_typed(expression.source_span(), None, Ty::None);
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved: Some(resolved.to_string()),
            raises: None,
            args: args.clone(),
            kwargs: Vec::new(),
            recv_place: Some(MirPlace::root(collection, Some(target.clone()))),
            arg_places: vec![None; args.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
        });
    }

    pub(super) fn finish_nominal_collection(
        &mut self,
        expression: &Expr,
        collection: VarId,
        target: &Ty,
    ) -> Reg {
        let result = self.fresh_typed(expression.source_span(), Some(collection), target.clone());
        self.emit(MirInstr::UseVar {
            dest: result,
            var: collection,
            mode: UseMode::Move,
        });
        result
    }

    /// Lower comprehension clauses directly into MIR control flow. This is the
    /// same left-to-right nesting as an explicit series of `for`/`if` blocks;
    /// the final leaf performs the collection family's insertion protocol.
    pub(super) fn comprehension_clauses(
        &mut self,
        clauses: &[crate::ast::ComprehensionClause],
        bindings: &[crate::checked::CheckedComprehensionBinding],
        index: usize,
        plan: &ComprehensionPlan<'_>,
    ) {
        if index == clauses.len() {
            // Dictionary evaluation is key-before-value, matching an ordinary
            // display and indexed assignment. List/set leaves evaluate one item.
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

        match &clauses[index] {
            crate::ast::ComprehensionClause::If(condition) => {
                let condition = self.expr(condition);
                let body = self.new_block();
                let continuation = self.new_block();
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: condition,
                    then_b: body,
                    else_b: continuation,
                };
                self.cur = body;
                self.comprehension_clauses(clauses, bindings, index + 1, plan);
                self.f.blocks[self.cur].term = MirTerm::Jump(continuation);
                self.cur = continuation;
            }
            crate::ast::ComprehensionClause::For {
                var, owned, iter, ..
            } => {
                let iterator_name = format!("$compiter{}", self.vars.len());
                let iterator = self.var(&iterator_name);
                let iterator_ty = self.checked_ty(iter);
                if let Some(ty) = iterator_ty.clone() {
                    self.var_types.insert(iterator, ty);
                }
                let protocol = self
                    .checked_adjustments(iter)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::Iterate(protocol) => Some(protocol),
                        _ => None,
                    })
                    .unwrap_or(crate::IterationProtocol {
                        mode: if *owned {
                            crate::IterationMode::Owned
                        } else {
                            crate::IterationMode::Borrowed
                        },
                        borrowed_origin: None,
                        reference: None,
                        prepare: Vec::new(),
                        has_next: None,
                        next: None,
                        exhaustion: None,
                    });
                if let Some(origin) = &protocol.borrowed_origin {
                    let place = self.place(iter);
                    let value_ty = iterator_ty
                        .clone()
                        .or_else(|| place.ty.clone())
                        .expect("checked borrowed comprehension iterable has a type");
                    let iterator_value =
                        self.fresh_typed(iter.source_span(), Some(place.root), value_ty);
                    self.emit(MirInstr::LoadPlace {
                        dest: iterator_value,
                        place: place.clone(),
                    });
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty.clone(),
                    });
                    let canonical = self
                        .mir_interior_origin(origin, Some(place.root))
                        .expect("checked borrowed comprehension origin has a MIR owner");
                    let loans = vec![MirLoan {
                        place,
                        mutable: false,
                        interior: Some(canonical),
                    }];
                    let marker =
                        self.fresh_typed(iter.source_span(), Some(loans[0].place.root), Ty::None);
                    self.emit(MirInstr::EstablishLoans {
                        reference: iterator,
                        loans: loans.clone(),
                        marker,
                    });
                    self.aggregate_loans.insert(iterator, loans);
                } else {
                    let iterator_value = self.expr(iter);
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty.clone(),
                    });
                }
                self.emit(MirInstr::GetIter {
                    iter: iterator,
                    mode: protocol.mode,
                    prepare: protocol.prepare.clone(),
                });

                let header = self.new_block();
                let body = self.new_block();
                let exit = self.new_block();
                let binding_index = clauses[..index]
                    .iter()
                    .filter(|clause| matches!(clause, crate::ast::ComprehensionClause::For { .. }))
                    .count();
                let binding = bindings
                    .get(binding_index)
                    .expect("checked comprehension binder metadata");
                let element_value = self.fresh(iter.source_span(), Some(iterator));
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = header;
                let has_next = self.fresh(iter.source_span(), Some(iterator));
                if let Some(exhaustion) = protocol.exhaustion.clone() {
                    self.emit(MirInstr::TryNext {
                        dest: element_value,
                        yielded: has_next,
                        iter: iterator,
                        method: protocol
                            .next
                            .clone()
                            .expect("raising iterator has checked __next__ symbol"),
                        exhaustion,
                    });
                } else {
                    self.emit(MirInstr::HasNext {
                        dest: has_next,
                        iter: iterator,
                        method: protocol.has_next.clone(),
                    });
                }
                self.f.blocks[self.cur].term = MirTerm::Branch {
                    cond: has_next,
                    then_b: body,
                    else_b: exit,
                };

                self.cur = body;
                if protocol.exhaustion.is_none() {
                    self.emit(MirInstr::Next {
                        dest: element_value,
                        iter: iterator,
                        method: protocol.next.clone(),
                    });
                }
                let binding_var = self.var(&format!("$comp{}${}", var, binding.owner.0));
                self.owner_vars.insert(binding.owner, binding_var);
                let element_ty = Some(binding.ty.clone());
                if let Some(ty) = element_ty.clone() {
                    self.var_types.insert(binding_var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var: binding_var,
                    src: element_value,
                    binding_ty: element_ty,
                });
                self.comprehension_clauses(clauses, bindings, index + 1, plan);
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = exit;
            }
        }
    }

    pub(super) fn comprehension(
        &mut self,
        expression: &Expr,
        _kind: crate::ast::CollectionKind,
        key: Option<&Expr>,
        value: &Expr,
        clauses: &[crate::ast::ComprehensionClause],
    ) -> Reg {
        let (target, insert) = self
            .collection_plan(expression)
            .expect("checked collection comprehension has a nominal construction plan");
        let insert = insert.expect("list/set/dict comprehension has an insertion method");
        let collection = self.begin_nominal_collection(expression, &target);
        let bindings = self.comprehension_bindings(expression);
        let plan = ComprehensionPlan {
            collection,
            target: &target,
            insert: &insert,
            key,
            value,
        };
        self.comprehension_clauses(clauses, &bindings, 0, &plan);
        self.finish_nominal_collection(expression, collection, &target)
    }

    /// Post-order: each subexpression emits one instruction and yields its result
    /// `Reg`, so `foo(bar(x))` → `t0 = bar(x); t1 = foo(t0)`. Total over `Expr`.
    pub(super) fn expr_hir(&mut self, expression: &crate::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.expr(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(super) fn reference_handle_hir(&mut self, expression: &crate::hir::HirExpr) -> Reg {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.reference_handle(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(super) fn projected_reference_place_hir(
        &mut self,
        expression: &crate::hir::HirExpr,
    ) -> Option<MirPlace> {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.lower_projected_reference_place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(super) fn place_hir(&mut self, expression: &crate::hir::HirExpr) -> MirPlace {
        let mut index = HashMap::new();
        index_hir_expression(&expression.syntax, expression, &mut index);
        self.active_semantics.push(index);
        let result = self.place(&expression.syntax);
        self.active_semantics.pop();
        result
    }

    pub(super) fn expr(&mut self, e: &Expr) -> Reg {
        let result = self.expr_with_adjustments(e);
        // An emit-site type (a conversion result, a closure value) is more
        // precise than the source expression's pre-adjustment checked type.
        if let Some(ty) = self.checked_ty(e) {
            self.f.reg_types.entry(result.0).or_insert(ty);
        }
        result
    }

    pub(super) fn expr_with_adjustments(&mut self, e: &Expr) -> Reg {
        if self.checked_adjustments(e).iter().any(|adjustment| {
            matches!(
                adjustment,
                crate::SemanticAdjustment::BorrowShared | crate::SemanticAdjustment::BorrowMutable
            )
        }) {
            return self.reference_handle(e);
        }
        if let Some(reference) = self.reference_result(e) {
            let handle = self.reference_handle(e);
            let value_ty = match self.f.reg_types.get(&handle.0) {
                Some(Ty::Ref(reference)) => (*reference.referent).clone(),
                _ => (*reference.referent).clone(),
            };
            let read = self.fresh_typed(span(e), None, value_ty.clone());
            self.emit(MirInstr::ReadRef {
                dest: read,
                reference: handle,
            });
            // A reference-returning expression has two checked uses. `ref x =
            // expression` is intercepted by the Borrow adjustment above and
            // retains its handle. Every ordinary value use reads the referent
            // into independently owned storage, so lifecycle types must run
            // their copy initializer rather than alias backing storage.
            let dest = self.fresh_typed(span(e), None, value_ty);
            self.emit(MirInstr::CopyValue { dest, value: read });
            return dest;
        }
        if let Some(target) = self.index_normalization(e) {
            // The source Indexer is evaluated exactly once. The checked target
            // may be concrete or an abstract trait-dispatch symbol; MethodCall
            // already retargets the latter from the runtime receiver while
            // preserving the selected signature.
            let recv = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(recv.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, Ty::Int);
            self.emit(MirInstr::MethodCall {
                dest,
                recv,
                method: "__mlir_index__".to_string(),
                resolved: Some(target),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                recv_place: None,
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                param_decls: Vec::new(),
            });
            return dest;
        }
        if let Some(target) = self.implicit_conversion(e) {
            let argument = self.expr_unconverted(e);
            // The conversion result is the constructed type, not the source
            // expression's checked type; targets are concrete constructors.
            let dest = match target.split(".__init__").next() {
                Some(constructed) if !constructed.is_empty() => self.fresh_typed(
                    span(e),
                    None,
                    Ty::Struct(constructed.to_string(), Vec::new()),
                ),
                _ => self.fresh(span(e), None),
            };
            self.emit(MirInstr::Call {
                dest,
                func: FuncRef::named(&target),
                raises: None,
                args: vec![argument],
                kwargs: Vec::new(),
                arg_places: vec![None],
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
            });
            return dest;
        }
        if let Some(target) = self.literal_materialization(e) {
            let value = self.expr_unconverted(e);
            if let Some(source) = self.checked_ty(e) {
                self.f.reg_types.entry(value.0).or_insert(source);
            }
            let dest = self.fresh_typed(span(e), None, target.clone());
            self.emit(MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            });
            return dest;
        }
        self.expr_unconverted(e)
    }

    pub(super) fn reference_result(&self, expression: &Expr) -> Option<crate::origin::RefTy> {
        // The selected-call contract is the canonical checked handoff.  In
        // particular, a free-function reference result may share its source
        // expression with another compatibility adjustment, so consulting only
        // the legacy single-operation slot can silently lose `ref[a, b] T` and
        // type a runtime handle local as ordinary `T` storage.
        self.checked_call_contract(expression)
            .and_then(|contract| contract.reference_result)
            .or_else(|| {
                self.checked_adjustments(expression)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::ReferenceResult { reference } => Some(reference),
                        _ => None,
                    })
            })
    }

    pub(super) fn expr_unconverted(&mut self, e: &Expr) -> Reg {
        match &e.kind {
            // --- Literals ------------------------------------------------------
            ExprKind::Int(n) => self.constant(e, Const::IntLiteral(n.clone())),
            ExprKind::Float(x) => self.constant(e, Const::FloatLiteral(x.clone())),
            ExprKind::Bool(b) => self.constant(e, Const::Bool(*b)),
            ExprKind::Str(s) => self.constant(e, Const::Str(s.clone())),
            ExprKind::None => self.constant(e, Const::None),
            ExprKind::Uninitialized => self.constant(e, Const::None),
            ExprKind::Spread(_) => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(
                    "unexpanded call spread reached MIR lowering".to_string(),
                ));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }

            // --- Variable reads ------------------------------------------------
            // A bare read defaults to `Copy`; a call site refines it to
            // `Borrow*`/`Move` per the callee's convention (Stage 6).
            ExprKind::Identifier(name) => {
                if let Some(target) = self.resolved_callable(e) {
                    return self.constant(e, Const::Function(target));
                }
                if let Some(info) = self.nested_info(e) {
                    return self.load_nested_closure(name, &info, span(e));
                }
                if !self.vars.iter().any(|candidate| candidate == name)
                    && self.overloads.is_function(name)
                {
                    return self.constant(e, Const::Function(name.clone()));
                }
                let var = self.expression_var(name, e);
                let d = self.fresh(span(e), Some(var));
                if self.is_origin_bearing_pointer(e) {
                    // Reading a pointer variable produces its handle value;
                    // `UseVar` would read through the stored `Value::Ref` the
                    // way a `ref` binding does. `MakeRef` on the root forwards
                    // the existing handle unchanged.
                    self.emit(MirInstr::MakeRef {
                        dest: d,
                        place: MirPlace::root(var, self.var_types.get(&var).cloned()),
                    });
                    return d;
                }
                if let Some(loan) = self.aliases.get(&var).cloned() {
                    let mut place = loan.place;
                    place.through = Some(var);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                } else if self.runtime_aliases.contains(&var) {
                    let handle = self.fresh(e.source_span(), Some(var));
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place: {
                            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
                            place.through = Some(var);
                            place
                        },
                    });
                    self.emit(MirInstr::ReadRef {
                        dest: d,
                        reference: handle,
                    });
                } else {
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Copy,
                    });
                }
                d
            }
            // `x^`: a move out of a variable. `p.a^` (a pure field chain) is a
            // partial move of that field. A constant index into compiler-private
            // Tuple storage is also an independently tracked slot; this is the
            // move path used by whole heterogeneous-pack forwarding and public
            // Tuple's private backing field. Other indexed transfers have
            // already been restricted by checking to copyable value reads.
            ExprKind::Transfer(inner) => {
                if let ExprKind::Identifier(name) = &inner.kind {
                    let var = self.expression_var(name, inner);
                    let d = self.fresh(span(e), Some(var));
                    self.emit(MirInstr::UseVar {
                        dest: d,
                        var,
                        mode: UseMode::Move,
                    });
                    d
                } else if let Some(place) = self.pure_field_place(inner) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else if let ExprKind::Index { object, .. } = &inner.kind
                    && matches!(self.checked_ty(object), Some(Ty::Tuple(_)))
                    && let Some(place) = self.try_place(inner)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MovePlace { dest: d, place });
                    d
                } else {
                    self.expr(inner)
                }
            }

            // --- Operators -----------------------------------------------------
            ExprKind::Prefix(op, a) => {
                let ra = self.expr(a);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::UnOp {
                    op: *op,
                    dest: d,
                    a: ra,
                });
                d
            }
            // `and`/`or` short-circuit — lowered to CFG blocks, not an eager BinOp.
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), a, b) => {
                self.short_circuit(*op, a, b, span(e))
            }
            // A checked nominal membership operation is an ordinary borrowed
            // `container.__contains__(value)` call.  Keeping it as a value-only
            // `BinOp` loses the receiver place; the VM would then install a
            // shallow struct value in the callee's `self` slot and destroy its
            // owned fields on return.  For pointer-backed collections that can
            // free the caller's storage.  Preserve source evaluation order
            // (value before container), the selected overload, and the normal
            // method-call place/capture contract.
            ExprKind::Infix(op @ (InfixOp::In | InfixOp::NotIn), value, container)
                if matches!(self.checked_ty(container), Some(Ty::Struct(..))) =>
            {
                let (argument, arg_place) = self.lower_call_argument(value);
                let (recv, recv_place) = self.lower_call_receiver(container);
                let contains = self.fresh_typed(span(e), None, Ty::Bool);
                self.emit_interior_invalidations(container, None);
                self.emit_call_invalidations(e, std::slice::from_ref(value), &[]);
                self.emit(MirInstr::MethodCall {
                    dest: contains,
                    recv,
                    method: "__contains__".to_string(),
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: vec![argument],
                    kwargs: Vec::new(),
                    recv_place,
                    arg_places: vec![arg_place],
                    kwarg_places: Vec::new(),
                    capture_accesses: self.checked_call_capture_accesses(e),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                });
                self.emit_nested_closure_argument_keepalives(std::slice::from_ref(value), &[]);
                if matches!(op, InfixOp::NotIn) {
                    let dest = self.fresh_typed(span(e), None, Ty::Bool);
                    self.emit(MirInstr::UnOp {
                        op: PrefixOp::Not,
                        dest,
                        a: contains,
                    });
                    dest
                } else {
                    contains
                }
            }
            ExprKind::Infix(op, a, b) => {
                let ra = self.expr(a); // operands left-to-right (evaluation order is explicit)
                let rb = self.expr(b);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::BinOp {
                    op: *op,
                    dest: d,
                    a: ra,
                    b: rb,
                    resolved: self.resolved_callable(e),
                });
                d
            }

            // --- Calls / access ------------------------------------------------
            // NOTE: keyword args + default-slot matching (`call::match_call_slots`)
            // are a follow-up; the checker has already validated them, so only the
            // positional `args` are flattened here.
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                // A checked pointer construction materializes the frame/slot
                // handle for its source place; the checked pointer type keeps
                // the origin while the runtime value erases it.
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::PointerToPlace { .. })
                }) {
                    let value = &kwargs
                        .first()
                        .expect("checked pointer construction has a 'to=' argument")
                        .value;
                    let place = self.place(value);
                    let dest = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::MakeRef { dest, place });
                    return dest;
                }
                if let Some(crate::SemanticAdjustment::ConstructVariant {
                    alternatives,
                    index,
                }) = self.checked_adjustments(e).into_iter().find(|adjustment| {
                    matches!(
                        adjustment,
                        crate::SemanticAdjustment::ConstructVariant { .. }
                    )
                }) {
                    let value = self.expr(
                        args.first()
                            .expect("checked Variant construction has one payload"),
                    );
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeVariant {
                        dest,
                        alternatives,
                        index,
                        value,
                    });
                    return dest;
                }
                // SIMD construction resolves its `[DType.<dt>, width]` parameters
                // here (the MIR is otherwise untyped about them).
                if let Some(r) = self.try_simd_call(e, args) {
                    return r;
                }
                // A call to a nested `def` (a closure, called by name in scope):
                // rewrite to its lifted function, prepending the captured enclosing
                // locals as leading arguments (passed as places, so the `mut`
                // capture parameters write back — reference-capture semantics).
                if let Some(info) = self.nested_info(e) {
                    return self.lower_nested_call(e, &info, param_args, args, kwargs);
                }
                // A local with a function type (normally a callable parameter)
                // shadows any global function of the same name.
                if self.vars.iter().any(|candidate| candidate == name) {
                    let callee = self.expr(&Expr {
                        kind: ExprKind::Identifier(name.clone()),
                        span: e.span,
                        source: e.source.clone(),
                        syntax_id: crate::token::SyntaxId::fresh(),
                    });
                    let callable_ty = self
                        .vars
                        .iter()
                        .position(|candidate| candidate == name)
                        .and_then(|variable| self.var_types.get(&(variable as VarId)))
                        .cloned()
                        .or_else(|| self.f.reg_types.get(&callee.0).cloned());
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let param_decls = callable_ty
                        .as_ref()
                        .map(generic_callable_param_decls)
                        .unwrap_or_default();
                    let (regs, arg_places) = self.lower_call_arguments(args);
                    let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                    let place = self.resolved_place(name);
                    let callee_place = place.is_typed().then_some(place);
                    let dest = self.fresh(span(e), None);
                    self.emit_call_invalidations(e, args, kwargs);
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    let (instantiated_contract, instantiated_args) = self
                        .instantiated_callable_contract(e)
                        .map_or((None, Vec::new()), |(contract, arguments)| {
                            (Some(contract), arguments)
                        });
                    self.emit(MirInstr::CallIndirect {
                        dest,
                        callee,
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        callee_place,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs,
                        param_decls,
                        instantiated_contract,
                        instantiated_args,
                    });
                    return dest;
                }
                // `__RuntimeTuple` is the compiler-private heterogeneous pack
                // storage primitive. Public `Tuple` is an ordinary nominal
                // variadic struct and follows the call path below.
                if name == "__RuntimeTuple"
                    && kwargs.is_empty()
                    && !self.overloads.is_function(name)
                {
                    let regs = self.args(args);
                    let element_types = match self.checked_ty(e) {
                        Some(Ty::Tuple(elements)) => Some(elements),
                        _ => None,
                    };
                    let dest = self.fresh(span(e), None);
                    self.emit(MirInstr::MakeTuple {
                        dest,
                        elems: regs,
                        element_types,
                    });
                    return dest;
                }
                // Compile-time parameter arguments (`Name[param_args](...)`),
                // evaluated before ordinary call arguments: a
                // **value** parameter is a comptime `Int` expression flattened to a
                // register; a **type** parameter is erased (`None`).
                let param_arg_regs = self.param_arg_regs(param_args);
                // Retain only checker-selected `mut`/`ref` caller places. A
                // syntactically simple copied argument remains eligible for
                // ASAP destruction after its value has been evaluated.
                let (regs, arg_places) = self.lower_call_arguments(args);
                let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                let target = self
                    .resolved_callable(e)
                    .unwrap_or_else(|| self.overloaded_name(name, args.len()));
                let d = self.fresh(span(e), None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(&target),
                    raises: self.checked_raises(e),
                    args: regs,
                    kwargs: kw,
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                d
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                if let Some(operation) =
                    self.checked_adjustments(e).into_iter().find(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::VariantIs { .. }
                                | crate::SemanticAdjustment::VariantTypeSupported { .. }
                                | crate::SemanticAdjustment::VariantSet { .. }
                                | crate::SemanticAdjustment::VariantTake { .. }
                                | crate::SemanticAdjustment::VariantReplace { .. }
                        )
                    })
                {
                    let ExprKind::Member { object, .. } = &callee.kind else {
                        unreachable!("checked Variant operation has a member callee")
                    };
                    match operation {
                        crate::SemanticAdjustment::VariantIs { index, .. } => {
                            let variant = self.expr(object);
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::VariantIs {
                                dest,
                                variant,
                                index,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTypeSupported { supported } => {
                            let dest = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest,
                                k: Const::Bool(supported),
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantSet { index, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.set receiver is a writable place");
                            let value = self
                                .expr(args.first().expect("checked Variant.set has one payload"));
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantSet {
                                dest,
                                place,
                                index,
                                value,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantTake { index, checked, .. } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.take receiver is an owned place");
                            let variant = self.fresh(span(object), None);
                            self.emit(MirInstr::MovePlace {
                                dest: variant,
                                place,
                            });
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantTake {
                                dest,
                                variant,
                                index,
                                checked,
                            });
                            return dest;
                        }
                        crate::SemanticAdjustment::VariantReplace {
                            input_index,
                            output_index,
                            checked,
                            ..
                        } => {
                            let place = self
                                .try_place(object)
                                .expect("checked Variant.replace receiver is writable");
                            let value = self.expr(
                                args.first()
                                    .expect("checked Variant.replace has one payload"),
                            );
                            let dest = self.fresh(span(e), None);
                            self.emit_interior_invalidations(e, None);
                            self.emit(MirInstr::VariantReplace {
                                dest,
                                place,
                                input_index,
                                output_index,
                                value,
                                checked,
                            });
                            return dest;
                        }
                        _ => unreachable!("filtered Variant operation"),
                    }
                }
                if let Some(param_decls) = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::ParameterizedMethodCall { param_decls } => {
                            Some(param_decls)
                        }
                        _ => None,
                    },
                ) {
                    let ExprKind::Member { object, field } = &callee.kind else {
                        unreachable!("checked parameterized method call has a member callee")
                    };
                    // Keep this as a direct method invocation. In particular,
                    // do not synthesize a bound-method value (which would make
                    // its receiver/environment escapable).
                    let (recv, recv_place) = self.lower_call_receiver(object);
                    let param_arg_regs = self.param_arg_regs(param_args);
                    let (argument_regs, arg_places) = self.lower_call_arguments(args);
                    let (keyword_regs, kwarg_places) = self.lower_call_keywords(kwargs);
                    let dest = self.fresh(span(e), None);
                    let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                    self.emit_interior_invalidations(object, None);
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::MethodCall {
                        dest,
                        recv,
                        method: field.clone(),
                        resolved: self.resolved_callable(e),
                        raises: self.checked_raises(e),
                        args: argument_regs,
                        kwargs: keyword_regs,
                        recv_place: if implicitly_copied_receiver {
                            None
                        } else {
                            recv_place
                        },
                        arg_places,
                        kwarg_places,
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs,
                        param_decls,
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return dest;
                }
                let callee_place = self.callable_receiver_place(callee);
                let callable_ty = self.checked_ty(callee);
                let callee = self.expr(callee);
                let callable_ty = callable_ty.or_else(|| self.f.reg_types.get(&callee.0).cloned());
                let param_arg_regs = self.param_arg_regs(param_args);
                let param_decls = callable_ty
                    .as_ref()
                    .map(generic_callable_param_decls)
                    .unwrap_or_default();
                let (arg_regs, arg_places) = self.lower_call_arguments(args);
                let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs);
                let dest = self.fresh(span(e), None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                let (instantiated_contract, instantiated_args) = self
                    .instantiated_callable_contract(e)
                    .map_or((None, Vec::new()), |(contract, arguments)| {
                        (Some(contract), arguments)
                    });
                self.emit(MirInstr::CallIndirect {
                    dest,
                    callee,
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: arg_regs,
                    kwargs: kw_regs,
                    callee_place,
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs,
                    param_decls,
                    instantiated_contract,
                    instantiated_args,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                dest
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let pointer_storage = self.checked_adjustments(e).into_iter().find_map(
                    |adjustment| match adjustment {
                        crate::SemanticAdjustment::PointerStorageTake { element } => {
                            Some((true, element))
                        }
                        crate::SemanticAdjustment::PointerStorageDestroy { element } => {
                            Some((false, element))
                        }
                        _ => None,
                    },
                );
                if let Some((take, element)) = pointer_storage {
                    let pointer = self.expr(object);
                    let index = self.expr(
                        args.first()
                            .expect("checked pointer storage operation has one index"),
                    );
                    debug_assert!(kwargs.is_empty());
                    let dest = self.fresh(span(e), None);
                    self.emit(if take {
                        MirInstr::PointerStorageTake {
                            dest,
                            pointer,
                            index,
                            element,
                        }
                    } else {
                        MirInstr::PointerStorageDestroy {
                            dest,
                            pointer,
                            index,
                            element,
                        }
                    });
                    return dest;
                }
                let explicit_destroy = self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::ExplicitDestroy)
                });
                let implicitly_copied_receiver = self.implicitly_copies_consuming_receiver(e);
                if let ExprKind::Identifier(type_name) = &object.kind
                    && !self.vars.iter().any(|name| name == type_name)
                {
                    let (regs, arg_places) = self.lower_call_arguments(args);
                    let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                    let d = self.fresh(span(e), None);
                    let target = self
                        .resolved_callable(e)
                        .unwrap_or_else(|| format!("{type_name}.{method}"));
                    self.emit_call_invalidations(e, args, kwargs);
                    let capture_accesses = self.checked_call_capture_accesses(e);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&target),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places,
                        kwarg_places,
                        capture_accesses,
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // A **static** method on a parameterized built-in type — the receiver
                // is a type, not a value (`UnsafePointer[T].alloc(n)`). Lower to a
                // builtin call `Type.method(args)`; the element type is erased.
                if let ExprKind::TypeApply { name, .. } = &object.kind {
                    let regs = self.args(args);
                    let kw: Vec<(String, Reg)> = kwargs
                        .iter()
                        .map(|k| (k.name.clone(), self.expr(&k.value)))
                        .collect();
                    let d = self.fresh(span(e), None);
                    self.emit_call_invalidations(e, args, kwargs);
                    self.emit(MirInstr::Call {
                        dest: d,
                        func: FuncRef::named(&format!("{name}.{method}")),
                        raises: self.checked_raises(e),
                        args: regs,
                        kwargs: kw,
                        arg_places: vec![None; args.len()],
                        kwarg_places: vec![None; kwargs.len()],
                        capture_accesses: self.checked_call_capture_accesses(e),
                        param_arg_regs: Vec::new(),
                    });
                    self.emit_nested_closure_argument_keepalives(args, kwargs);
                    return d;
                }
                // If the receiver is a place, load it through that place (indices
                // evaluated once) and keep the place for write-back; otherwise it is
                // a temporary evaluated for its value only.
                let receiver_expr = if explicit_destroy {
                    match &object.kind {
                        ExprKind::Transfer(inner) => inner.as_ref(),
                        _ => object.as_ref(),
                    }
                } else {
                    object.as_ref()
                };
                let (recv, recv_place) = self.lower_call_receiver(receiver_expr);
                // Retain checker-selected `mut`/`ref` ordinary-argument places,
                // mirroring a free-function `Call`.
                let (regs, arg_places) = self.lower_call_arguments(args);
                let (kw, kwarg_places) = self.lower_call_keywords(kwargs);
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(receiver_expr, None);
                self.emit_call_invalidations(e, args, kwargs);
                let capture_accesses = self.checked_call_capture_accesses(e);
                // An ordinary method call can still select a generic method and
                // infer all of its compile-time arguments from runtime actuals.
                // Preserve that declaration vocabulary even though there are no
                // explicit `method[...]` value arguments to lower.
                let param_decls = self
                    .checked_call_contract(e)
                    .map(|contract| contract.param_decls)
                    .unwrap_or_default();
                self.emit(MirInstr::MethodCall {
                    dest: d,
                    recv,
                    method: method.clone(),
                    resolved: self.resolved_callable(e),
                    raises: self.checked_raises(e),
                    args: regs,
                    kwargs: kw,
                    recv_place: if explicit_destroy || implicitly_copied_receiver {
                        None
                    } else {
                        recv_place
                    },
                    arg_places,
                    kwarg_places,
                    capture_accesses,
                    param_arg_regs: Vec::new(),
                    param_decls,
                });
                self.emit_nested_closure_argument_keepalives(args, kwargs);
                if explicit_destroy
                    && !implicitly_copied_receiver
                    && let Some(place) = self.try_place(receiver_expr)
                {
                    if place.proj.is_empty() {
                        self.emit(MirInstr::ConsumeVar { var: place.root });
                    } else {
                        self.emit(MirInstr::ConsumePlace {
                            place,
                            marker: recv,
                        });
                    }
                }
                d
            }
            ExprKind::Member { object, field } => {
                // A pure field chain rooted at a variable (`p.a`, `p.a.b`) lowers to
                // a `LoadPlace` (a place read) so the ownership analysis sees *which*
                // field is read — enabling field-sensitive partial-move checking
                // (reading `p.b` after `p.a^` stays legal). A member of a temporary
                // or an indexed base keeps the register-based `GetField`.
                let descriptor_field = matches!(
                    self.checked_ty(object),
                    Some(Ty::Struct(name, args))
                        if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
                            && args.is_empty()
                );
                if !descriptor_field && let Some(place) = self.pure_field_place(e) {
                    let place_root = place.root;
                    let place_ty = place.ty.clone();
                    let loaded = self.fresh_typed(
                        span(e),
                        Some(place_root),
                        place_ty
                            .clone()
                            .or_else(|| self.checked_ty(e))
                            .unwrap_or(Ty::Error),
                    );
                    self.emit(MirInstr::LoadPlace {
                        dest: loaded,
                        place,
                    });
                    // A field expression selected by the checker for a
                    // consuming value context owns its result just like a
                    // bare-variable `UseVar { Copy }`. Keep `LoadPlace` itself
                    // handle-preserving for method receivers, borrowed call
                    // arguments, iteration, and other explicit place
                    // operations; make only the checked value-copy boundary
                    // visible here so a nested lifecycle field runs its
                    // `__copyinit__` instead of merely duplicating an owning
                    // UnsafePointer.
                    //
                    // Reference-valued fields retain their existing handle/read
                    // path.  Their ordinary referent copies are selected by the
                    // checked `ReferenceResult` adjustment, not by this nominal
                    // field rule.
                    if !matches!(place_ty, Some(Ty::Ref(_)))
                        && self.checked_adjustments(e).iter().any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                        })
                    {
                        let copied = self.fresh_typed(
                            span(e),
                            Some(place_root),
                            self.checked_ty(e).unwrap_or(Ty::Error),
                        );
                        self.emit(MirInstr::CopyValue {
                            dest: copied,
                            value: loaded,
                        });
                        copied
                    } else {
                        loaded
                    }
                } else {
                    let base = if self.reference_result(object).is_some() {
                        self.lower_call_receiver(object).0
                    } else {
                        self.expr(object)
                    };
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::GetField {
                        dest: d,
                        base,
                        field: field.clone(),
                    });
                    d
                }
            }
            ExprKind::Index { object, index } => {
                // An indexed reference-bearing aggregate element is a storage
                // place whose checked type is `ref T`; load through the stored
                // handle exactly like a direct reference field.  Ordinary
                // indexing remains the register-based operation below. A
                // checker-selected nominal accessor must stay on that dispatch
                // path: projecting the nominal struct as raw indexed storage
                // would lose its concrete `__getitem__$N` target.
                if matches!(self.checked_place_ty(e), Some(Ty::Ref(_)))
                    && self.resolved_callable(e).is_none()
                    && !matches!(self.checked_ty(object), Some(Ty::Struct(..)))
                    && let Some(place) = self.try_place(e)
                {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit_interior_invalidations(e, None);
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                // Dereferencing an origin-bearing pointer reads its source
                // place; the checker fixed the offset to 0. A stably bound
                // pointer substitutes the owner place directly, keeping the
                // owner touched (and so droppable) at each access; otherwise
                // the access reads through the runtime handle.
                if let Some(place) = self.pointer_deref_place(object) {
                    let d = self.fresh(span(e), Some(place.root));
                    self.emit(MirInstr::LoadPlace { dest: d, place });
                    return d;
                }
                if self.is_origin_bearing_pointer(object) {
                    let reference = self.expr(object);
                    let d = self.fresh(span(e), None);
                    self.emit(MirInstr::ReadRef { dest: d, reference });
                    return d;
                }
                let has_call = self.checked_call_contract(e).is_some();
                let (base, base_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let (idx, index_place) = self.lower_call_argument(index);
                let call = self.subscript_call_contract(e, &[(index.source_span(), idx)]);
                let intrinsic = call
                    .is_none()
                    .then(|| self.intrinsic_index_dispatch(object))
                    .flatten();
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(index, None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Index {
                    dest: d,
                    base,
                    index: idx,
                    base_place,
                    index_place,
                    call,
                    intrinsic,
                });
                d
            }

            // --- Aggregates ----------------------------------------------------
            ExprKind::ListLit(elems) => {
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    for element in elems {
                        let value = self.expr(element);
                        self.insert_nominal_collection(
                            element,
                            collection,
                            &target,
                            &insert,
                            vec![value],
                        );
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // The unchecked CFG helper has no semantic facts. Keep it
                // syntax-total by emitting an ordinary constructor call; the
                // production checked path above always carries an exact target
                // and insertion method.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("List"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::BraceLit(entries) => {
                if let Some((target, Some(insert))) = self.collection_plan(e) {
                    let collection = self.begin_nominal_collection(e, &target);
                    let dictionary = dict_elements(&target).is_some();
                    for (key, value) in entries {
                        let key = self.expr(key);
                        let mut arguments = vec![key];
                        if dictionary {
                            arguments.push(
                                self.expr(
                                    value
                                        .as_ref()
                                        .expect("checked dictionary display has paired values"),
                                ),
                            );
                        }
                        self.insert_nominal_collection(e, collection, &target, &insert, arguments);
                    }
                    return self.finish_nominal_collection(e, collection, &target);
                }
                // As above, this is only the syntax-only CFG compatibility
                // path. A verified program never guesses its collection kind.
                let dictionary = entries.first().is_none_or(|(_, value)| value.is_some());
                let d = self.fresh(span(e), None);
                let regs = if dictionary {
                    entries
                        .iter()
                        .flat_map(|(key, value)| {
                            let key = self.expr(key);
                            let value = value.as_ref().map(|value| self.expr(value));
                            std::iter::once(key).chain(value)
                        })
                        .collect::<Vec<_>>()
                } else {
                    entries
                        .iter()
                        .map(|(key, _)| self.expr(key))
                        .collect::<Vec<_>>()
                };
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named(if dictionary { "Dict" } else { "Set" }),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => self.comprehension(e, *kind, key.as_deref(), value, clauses),
            ExprKind::TupleLit(elems) => {
                if let Some((target, None)) = self.collection_plan(e)
                    && let Ty::Struct(name, _) = &target
                {
                    let regs = self.args(elems);
                    let dest = self.fresh_typed(span(e), None, target.clone());
                    self.emit(MirInstr::Call {
                        dest,
                        func: FuncRef::named(name),
                        raises: None,
                        args: regs.clone(),
                        kwargs: Vec::new(),
                        arg_places: vec![None; regs.len()],
                        kwarg_places: Vec::new(),
                        capture_accesses: Vec::new(),
                        param_arg_regs: Vec::new(),
                    });
                    return dest;
                }
                // Syntax-only lowering cannot select a variadic specialization,
                // but it still emits an ordinary public constructor call. The
                // private `MakeTuple` opcode is reserved for `__RuntimeTuple`.
                let regs = self.args(elems);
                let d = self.fresh(span(e), None);
                self.emit(MirInstr::Call {
                    dest: d,
                    func: FuncRef::named("Tuple"),
                    raises: None,
                    args: regs.clone(),
                    kwargs: Vec::new(),
                    arg_places: vec![None; regs.len()],
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                });
                d
            }

            // Walrus `:=` reaches MIR after type checking. Preserve an explicit
            // unsupported boundary rather than assigning accidental semantics.
            ExprKind::Named { name, value } => {
                let value = self.expr(value);
                let var = self.var(name);
                self.emit(MirInstr::DefVar {
                    var,
                    src: value,
                    binding_ty: None,
                });
                value
            }
            // Ternary `a if cond else b` — a value-producing branch (like the
            // short-circuit lowering, but both arms assign the result).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => self.ternary(cond, then_branch, else_branch, span(e)),
            // Chained comparison `a < b < c` — each operand evaluated once, folded
            // into short-circuiting `and`s.
            ExprKind::Compare { first, rest } => self.compare_chain(first, rest, span(e)),
            // Slice `object[lower:upper:step]` → a new List/String.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                let has_call = self.checked_call_contract(e).is_some();
                let (obj, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let lower = lower.as_ref().map(|b| self.expr(b));
                let upper = upper.as_ref().map(|b| self.expr(b));
                let step = step.as_ref().map(|b| self.expr(b));
                let call = self.subscript_call_contract(e, &[]);
                let intrinsic = call
                    .is_none()
                    .then(|| self.intrinsic_slice_dispatch(object))
                    .flatten();
                let kind = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            descriptors.first().copied().flatten()
                        }
                        _ => None,
                    })
                    .expect("checked slice has a selected descriptor");
                let d = self.fresh(span(e), None);
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::Slice {
                    dest: d,
                    object: obj,
                    kind,
                    lower,
                    upper,
                    step,
                    object_place,
                    arg_places: vec![None],
                    call,
                    intrinsic,
                });
                d
            }
            ExprKind::MultiIndex { object, args } => {
                let has_call = self.checked_call_contract(e).is_some();
                let (object, object_place) = if has_call {
                    self.lower_call_receiver(object)
                } else {
                    (self.expr(object), self.simple_place(object))
                };
                let descriptors = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::SliceDescriptors { descriptors, .. } => {
                            Some(descriptors)
                        }
                        _ => None,
                    })
                    .expect("checked multi-subscript has descriptor metadata");
                let mut arg_places = Vec::with_capacity(args.len());
                let mut parameter_sources = Vec::new();
                let lowered_args = args
                    .iter()
                    .zip(descriptors)
                    .map(|(argument, descriptor)| match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            debug_assert!(descriptor.is_none());
                            let (register, place) = self.lower_call_argument(value);
                            arg_places.push(place);
                            parameter_sources.push((value.source_span(), register));
                            MirSubscriptArg::Index(register)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            arg_places.push(None);
                            MirSubscriptArg::Slice {
                                kind: descriptor.expect("slice argument has descriptor kind"),
                                lower: lower.as_ref().map(|value| self.expr(value)),
                                upper: upper.as_ref().map(|value| self.expr(value)),
                                step: step.as_ref().map(|value| self.expr(value)),
                            }
                        }
                    })
                    .collect();
                let call = self.subscript_call_contract(e, &parameter_sources);
                let dest = self.fresh(span(e), None);
                for argument in args {
                    if let crate::ast::SubscriptArg::Index(argument) = argument {
                        self.emit_interior_invalidations(argument, None);
                    }
                }
                self.emit_interior_invalidations(e, None);
                self.emit(MirInstr::MultiIndex {
                    dest,
                    object,
                    args: lowered_args,
                    object_place,
                    arg_places,
                    call,
                });
                dest
            }
            // These are flagged `Unsupported`/rejected by the *checker*, so a checked
            // program never reaches MIR lowering with them. A bare `TypeApply` is a
            // type used as a value (only valid as a static-method receiver, handled
            // in the `MethodCall` arm above).
            ExprKind::TString { parts, .. } => {
                let mut result = self.fresh(span(e), None);
                self.emit(MirInstr::Const {
                    dest: result,
                    k: Const::Str(String::new()),
                });
                for part in parts {
                    let piece = match part {
                        TStringPart::Literal(text) => {
                            let register = self.fresh(span(e), None);
                            self.emit(MirInstr::Const {
                                dest: register,
                                k: Const::Str(text.clone()),
                            });
                            register
                        }
                        TStringPart::Expr(value) => {
                            let argument = self.expr(value);
                            // Interpolation's implicit `String(value)` call has
                            // no source expression of its own.  Give the
                            // synthetic result its checked intrinsic type here
                            // instead of asking declaration-based MIR closure to
                            // rediscover the return type of the builtin.
                            let register = self.fresh_typed(span(value), None, Ty::String);
                            self.emit(MirInstr::Call {
                                dest: register,
                                func: FuncRef::named("String"),
                                raises: None,
                                args: vec![argument],
                                kwargs: Vec::new(),
                                arg_places: vec![None],
                                kwarg_places: Vec::new(),
                                capture_accesses: Vec::new(),
                                param_arg_regs: Vec::new(),
                            });
                            register
                        }
                    };
                    let joined = self.fresh(span(e), None);
                    self.emit(MirInstr::BinOp {
                        op: InfixOp::Add,
                        dest: joined,
                        a: result,
                        b: piece,
                        resolved: None,
                    });
                    result = joined;
                }
                result
            }
            ExprKind::TypeApply { name, .. }
                if self.checked_adjustments(e).iter().any(|adjustment| {
                    matches!(adjustment, crate::SemanticAdjustment::VariantProject { .. })
                }) =>
            {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("checked Variant projection carries a tag");
                let mut place = self.resolved_place(name);
                if place.root_ty.is_none() {
                    place.root_ty = Some(Ty::Variant(
                        self.checked_adjustments(e)
                            .into_iter()
                            .find_map(|adjustment| match adjustment {
                                crate::SemanticAdjustment::VariantProject {
                                    alternatives, ..
                                } => Some(alternatives),
                                _ => None,
                            })
                            .unwrap_or_default(),
                    ));
                }
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                place.project(Proj::Variant(index), ty);
                let dest = self.fresh(span(e), Some(place.root));
                self.emit(MirInstr::LoadPlace { dest, place });
                dest
            }
            ExprKind::TypeApply { name, .. } if self.nested_info(e).is_some() => {
                let info = self
                    .nested_info(e)
                    .expect("guard established a checked nested declaration");
                let dest = self.load_nested_closure(name, &info, span(e));
                // The closure slot carries the declaration's generic callable
                // type; this expression carries the checker's concrete Origin
                // substitution and must win at the MIR value boundary.
                if let Some(specialized) = self.checked_ty(e) {
                    self.f.reg_types.insert(dest.0, specialized);
                }
                dest
            }
            ExprKind::TypeApply { .. } if self.resolved_callable(e).is_some() => self.constant(
                e,
                Const::Function(
                    self.resolved_callable(e)
                        .expect("checked callable TypeApply has a lowered target"),
                ),
            ),
            ExprKind::TypeValue(_) | ExprKind::TypeApply { .. } => {
                let dest = self.fresh(span(e), None);
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked expression reached MIR lowering: {:?}",
                    e.kind
                )));
                self.emit(MirInstr::Const {
                    dest,
                    k: Const::None,
                });
                dest
            }
        }
    }

    /// If `name(...)` is a SIMD construction — `SIMD[DType.<dt>, width](elems)` or
    /// a scalar alias (`Int32(x)`, `Float32(x)`, …) — resolve its dtype/width and
    /// emit a [`MirInstr::MakeSimd`], returning its result register. Otherwise
    /// `None`, and the caller lowers it as an ordinary call.
    pub(super) fn try_simd_call(&mut self, e: &Expr, args: &[Expr]) -> Option<Reg> {
        let (dtype, width) = self
            .checked_adjustments(e)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::ConstructSimd { dtype, width } => {
                    usize::try_from(width).ok().map(|width| (dtype, width))
                }
                _ => None,
            })?;
        let elems = self.args(args);
        let d = self.fresh(span(e), None);
        self.emit(MirInstr::MakeSimd {
            dest: d,
            dtype,
            width,
            elems,
        });
        Some(d)
    }

    /// Lower a call to a nested `def` through the same closure-environment path as
    /// a first-class closure value. This preserves reference handles across sibling
    /// calls and recursion; it does not rely on call-return write-back.
    pub(super) fn lower_nested_call(
        &mut self,
        e: &Expr,
        info: &NestedInfo,
        param_args: &[ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Reg {
        let name = match &e.kind {
            ExprKind::Call { name, .. } => name.as_str(),
            _ => unreachable!("nested direct call has call syntax"),
        };
        let callee = self.load_nested_closure(name, info, span(e));
        let callable_ty = info
            .callable_ty
            .clone()
            .or_else(|| self.f.reg_types.get(&callee.0).cloned());
        let param_arg_regs = self.param_arg_regs(param_args);
        let param_decls = callable_ty
            .as_ref()
            .map(generic_callable_param_decls)
            .unwrap_or_default();
        let (arg_regs, arg_places) = self.lower_call_arguments(args);
        let (kw_regs, kwarg_places) = self.lower_call_keywords(kwargs);
        let d = self.fresh(span(e), None);
        self.emit_call_invalidations(e, args, kwargs);
        let callee_place = self
            .owner_vars
            .contains_key(&info.binding)
            .then(|| self.binding_place(info.binding, name));
        let capture_accesses = self.checked_call_capture_accesses(e);
        let (instantiated_contract, instantiated_args) = self
            .instantiated_callable_contract(e)
            .map_or((None, Vec::new()), |(contract, arguments)| {
                (Some(contract), arguments)
            });
        self.emit(MirInstr::CallIndirect {
            dest: d,
            callee,
            // The checked owner already selects this exact lifted closure.
            // `resolved` is reserved for nominal/trait `__call__` dispatch;
            // attaching that abstract target here can disagree with an erased
            // variadic closure ABI even though execution never consults it.
            resolved: None,
            raises: self.checked_raises(e),
            args: arg_regs,
            kwargs: kw_regs,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
        });
        self.emit_nested_closure_argument_keepalives(args, kwargs);
        let mut owners = Vec::new();
        let mut seen = HashSet::new();
        for capture in &info.captures {
            self.collect_capture_keepalives(capture, &mut owners, &mut seen);
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
        d
    }

    pub(super) fn collect_capture_keepalives(
        &self,
        capture: &NestedCapture,
        owners: &mut Vec<VarId>,
        seen: &mut HashSet<crate::origin::OwnerId>,
    ) {
        if capture.kind == crate::ast::CaptureKind::Move || !seen.insert(capture.binding) {
            return;
        }
        if let Some(var) = self.owner_vars.get(&capture.binding).copied()
            && !owners.contains(&var)
        {
            owners.push(var);
        }
        // A captured closure slot can itself retain reference captures. Keep
        // those owners alive transitively; owned copy/move environment entries
        // are self-contained and deliberately stop the walk.
        if let Some(callable) = self.nested.get(&capture.binding) {
            for nested in &callable.captures {
                if matches!(
                    nested.kind,
                    crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(nested, owners, seen);
                }
            }
        }
    }

    /// A capture-bearing nested callable passed to another non-escaping call
    /// can leave its environment handle in an SSA register. Keep the referenced
    /// owner storage alive through that consuming call without creating a
    /// persistent access loan (Mojo permits intervening owner mutation).
    pub(super) fn emit_nested_closure_argument_keepalives(
        &mut self,
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) {
        let mut owners = Vec::new();
        for expression in args
            .iter()
            .chain(kwargs.iter().map(|argument| &argument.value))
        {
            let expression = match &expression.kind {
                ExprKind::Named { value, .. } => value.as_ref(),
                _ => expression,
            };
            let ExprKind::Identifier(_) = &expression.kind else {
                continue;
            };
            let Some(info) = self.nested_info(expression) else {
                continue;
            };
            let mut seen = HashSet::new();
            let callable_capture = NestedCapture {
                name: info.source_name.clone(),
                binding: info.binding,
                ty: info.callable_ty.clone().unwrap_or(Ty::Param {
                    name: "$capture".to_string(),
                    bounds: Vec::new(),
                    callable_bound: None,
                }),
                kind: crate::ast::CaptureKind::Read,
            };
            self.collect_capture_keepalives(&callable_capture, &mut owners, &mut seen);
            for capture in info.captures {
                if matches!(
                    capture.kind,
                    crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref
                ) {
                    self.collect_capture_keepalives(&capture, &mut owners, &mut seen);
                }
            }
        }
        for var in owners {
            self.emit(MirInstr::KeepAlive { var });
        }
    }

    pub(super) fn emit_nested_closure(
        &mut self,
        info: &NestedInfo,
        at: SourceSpan,
        forward_existing_environment: bool,
    ) -> Reg {
        let captures = info
            .captures
            .iter()
            .map(|capture| MirClosureCapture {
                place: self.binding_place(capture.binding, &capture.name),
                mode: if forward_existing_environment {
                    // In a lifted body these names are already references into
                    // the declaration-created environment. Recursion and calls
                    // to inherited siblings forward those handles; they must
                    // never repeat a copy/move capture operation.
                    MirCaptureMode::Reference
                } else {
                    match capture.kind {
                        crate::ast::CaptureKind::Copy => MirCaptureMode::Copy,
                        crate::ast::CaptureKind::Move => MirCaptureMode::Move,
                        crate::ast::CaptureKind::Read
                        | crate::ast::CaptureKind::Mut
                        | crate::ast::CaptureKind::Ref => MirCaptureMode::Reference,
                    }
                },
            })
            .collect();
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at, None, ty.clone()),
            None => self.fresh(at, None),
        };
        self.emit(MirInstr::MakeClosure {
            dest,
            function: info.mangled.clone(),
            captures,
        });
        dest
    }

    pub(super) fn load_nested_closure(
        &mut self,
        name: &str,
        info: &NestedInfo,
        at: SourceSpan,
    ) -> Reg {
        if !info.materialized_here && !self.owner_vars.contains_key(&info.binding) {
            // A lifted body has no direct access to an outer frame's closure slot.
            // Its inherited/self callable is reconstructed from the environment
            // parameters forwarded into this frame; direct declarations never use
            // this path after their statement has materialized them.
            return self.emit_nested_closure(info, at, true);
        }
        let var = self.binding_var(info.binding, name);
        if let Some(ty) = &info.callable_ty {
            self.var_types.entry(var).or_insert_with(|| ty.clone());
        }
        let dest = match &info.callable_ty {
            Some(ty) => self.fresh_typed(at.clone(), Some(var), ty.clone()),
            None => self.fresh(at.clone(), Some(var)),
        };
        if let Some(loan) = self.aliases.get(&var).cloned() {
            let mut place = loan.place;
            place.through = Some(var);
            self.emit(MirInstr::LoadPlace { dest, place });
        } else if self.runtime_aliases.contains(&var) {
            let handle = self.fresh(at, Some(var));
            let mut place = MirPlace::root(var, self.var_types.get(&var).cloned());
            place.through = Some(var);
            self.emit(MirInstr::MakeRef {
                dest: handle,
                place,
            });
            self.emit(MirInstr::ReadRef {
                dest,
                reference: handle,
            });
        } else {
            self.emit(MirInstr::UseVar {
                dest,
                var,
                // Calling a closure borrows its declaration-created environment;
                // neither loading it for a call nor a repeated call consumes or
                // duplicates that environment.
                mode: UseMode::BorrowShared,
            });
        }
        dest
    }

    /// Emit a `Const` writing a fresh register.
    pub(super) fn constant(&mut self, e: &Expr, k: Const) -> Reg {
        let constant_ty = match &k {
            Const::Int(_) => Some(Ty::Int),
            Const::Float(_) => Some(Ty::Float64),
            Const::IntLiteral(_) => Some(Ty::IntLiteral),
            Const::FloatLiteral(_) => Some(Ty::FloatLiteral),
            Const::Bool(_) => Some(Ty::Bool),
            Const::Str(_) => Some(Ty::String),
            Const::None => Some(Ty::None),
            Const::Function(_) => self.checked_ty(e),
        };
        let d = match constant_ty {
            Some(ty) => self.fresh_typed(span(e), None, ty),
            None => self.fresh(span(e), None),
        };
        self.emit(MirInstr::Const { dest: d, k });
        d
    }

    pub(super) fn materialize_register(
        &mut self,
        value: Reg,
        target: &Ty,
        source: SourceSpan,
    ) -> Reg {
        let Some(found) = self.f.reg_types.get(&value.0) else {
            return value;
        };
        let compatible = match (found, target) {
            (Ty::IntLiteral, Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }) => true,
            (Ty::FloatLiteral, Ty::Float64) => true,
            (Ty::FloatLiteral, Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            _ => false,
        };
        if !compatible {
            return value;
        }
        let dest = self.fresh_typed(source, None, target.clone());
        self.emit(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
        dest
    }
}
