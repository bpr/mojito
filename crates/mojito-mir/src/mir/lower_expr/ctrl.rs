//! Control-flow expressions: short-circuit/ternary/compare chains,
//! collection plans, and comprehensions.

use super::*;

impl Flatten<'_> {
    /// A condition whose value is a `Boolable` struct (checker-marked
    /// `Truthiness`) converts through `Bool(x)` — the same call `Bool(x)`
    /// spells, which both backends dispatch to the struct's `__bool__`.
    pub(in crate::mir) fn truthiness(&mut self, condition: &Expr, value: Reg) -> Reg {
        let boolable = self
            .checked_adjustments(condition)
            .iter()
            .any(|adjustment| {
                matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::Truthiness
                )
            });
        if !boolable {
            return value;
        }
        self.bool_conversion(condition.source_span(), value)
    }

    /// The `Bool(x)` call both backends dispatch to a struct's `__bool__`.
    pub(in crate::mir) fn bool_conversion(&mut self, span: SourceSpan, value: Reg) -> Reg {
        let dest = self.fresh_typed(span, None, Ty::Bool);
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
        });
        dest
    }

    /// Lower `a and b` / `a or b` into control flow so the right operand is only
    /// evaluated when needed (Python/Mojo short-circuit semantics). The result is
    /// carried in a synthetic variable across the branch and read back in the
    /// merge block. (Preserving the short-circuit — vs an eager `BinOp` — matters
    /// both for observable side effects and for Stage 6 ownership, where a moved
    /// operand on the not-taken side must not count as moved.)
    pub(in crate::mir) fn short_circuit(
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
    pub(in crate::mir) fn ternary(
        &mut self,
        cond: &Expr,
        then_e: &Expr,
        else_e: &Expr,
        sp: SourceSpan,
    ) -> Reg {
        let rc = self.expr(cond);
        let rc = self.truthiness(cond, rc);
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
    pub(in crate::mir) fn compare_chain(
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
    pub(in crate::mir) fn collection_plan(
        &self,
        expression: &Expr,
    ) -> Option<(Ty, Option<String>)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructCollection {
                    target,
                    insert,
                } => Some((target, insert)),
                _ => None,
            })
    }

    /// Return the fixed-size array literal constructor selected by the
    /// checker: the concrete target type and the exact lowered `__init__`
    /// overload symbol of the variadic literal constructor.
    pub(in crate::mir) fn array_literal_plan(&self, expression: &Expr) -> Option<(Ty, String)> {
        self.checked_adjustments(expression)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                mojito_checked::checked::SemanticAdjustment::ConstructArrayLiteral {
                    target,
                    constructor,
                } => Some((target, constructor)),
                _ => None,
            })
    }

    /// Install the caller-side loans a callee's transfer effects imply at
    /// this call. The checker recorded the substituted sources per
    /// occurrence; the destination actual's root var receives the merged
    /// `EstablishLoans`, so ownership analysis sees mutation conflicts and
    /// the drops pass keeps the loan roots alive while the carrier lives.
    pub(in crate::mir) fn install_call_transfers(
        &mut self,
        e: &Expr,
        recv_place: Option<&MirPlace>,
        arg_places: &[Option<MirPlace>],
    ) {
        let Some(transfers) = self.call_transfers.get(&e.source_span()).cloned() else {
            return;
        };
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
        for transfer in transfers {
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
                });
            }
            if loans.is_empty() || loans.len() == before {
                continue;
            }
            let marker = self.fresh_typed(e.source_span(), Some(dest_root), Ty::None);
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

    /// Construct an empty nominal collection and bind it to a synthetic slot so
    /// each checked mutating insertion can use the ordinary method-call ABI.
    pub(in crate::mir) fn begin_nominal_collection(
        &mut self,
        expression: &Expr,
        target: &Ty,
    ) -> VarId {
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
    pub(in crate::mir) fn insert_nominal_collection(
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
            reference_result: None,
            result_adapter: None,
            args: args.clone(),
            kwargs: Vec::new(),
            recv_place: Some(MirPlace::root(collection, Some(target.clone()))),
            recv_writes: true,
            arg_places: vec![None; args.len()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
        });
    }

    pub(in crate::mir) fn finish_nominal_collection(
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
    pub(in crate::mir) fn comprehension_clauses(
        &mut self,
        clauses: &[mojito_ast::ast::ComprehensionClause],
        bindings: &[mojito_checked::checked::CheckedComprehensionBinding],
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
            mojito_ast::ast::ComprehensionClause::If(condition) => {
                let value = self.expr(condition);
                let condition = self.truthiness(condition, value);
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
            mojito_ast::ast::ComprehensionClause::For { var, iter, .. } => {
                let iterator_name = format!("$compiter{}", self.vars.len());
                let iterator = self.var(&iterator_name);
                let iterator_ty = self.checked_ty(iter);
                let protocol = self
                    .checked_adjustments(iter)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        mojito_checked::checked::SemanticAdjustment::Iterate(protocol) => {
                            Some(protocol)
                        }
                        _ => None,
                    })
                    .unwrap_or(mojito_checked::checked::IterationProtocol {
                        mode: if matches!(iter.kind, ExprKind::Transfer(_)) {
                            mojito_checked::checked::IterationMode::Owned
                        } else {
                            mojito_checked::checked::IterationMode::Borrowed
                        },
                        binding: None,
                        borrowed_origin: None,
                        yield_interior: Vec::new(),
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
                    self.borrow_iteration_source(
                        iterator,
                        iter.source_span(),
                        place,
                        value_ty,
                        origin,
                    );
                } else {
                    if let Some(ty) = iterator_ty.clone() {
                        self.var_types.insert(iterator, ty);
                    }
                    let iterator_value = self.expr(iter);
                    self.emit(MirInstr::DefVar {
                        var: iterator,
                        src: iterator_value,
                        binding_ty: iterator_ty.clone(),
                    });
                }
                // The same retained-source/iterator-object slot split as the
                // statement path: a borrowed source stays in its own slot, so
                // `GetIter` normalization cannot clobber the storage (or the
                // reference handle) the iterator still refers to.
                let borrowed = protocol.borrowed_origin.is_some();
                let split_source = matches!(
                    protocol.mode,
                    mojito_checked::checked::IterationMode::Borrowed
                ) && (borrowed || !protocol.prepare.is_empty());
                let iterator_object = if split_source {
                    self.var(&format!("$compiterobj{}", self.vars.len()))
                } else {
                    iterator
                };
                self.emit(MirInstr::GetIter {
                    source: iterator,
                    dest: iterator_object,
                    mode: protocol.mode,
                    prepare: protocol.prepare.clone(),
                });
                if borrowed && split_source {
                    self.reestablish_source_loans(iterator, iterator_object);
                }

                let header = self.new_block();
                let body = self.new_block();
                let exit = self.new_block();
                let binding_index = clauses[..index]
                    .iter()
                    .filter(|clause| {
                        matches!(clause, mojito_ast::ast::ComprehensionClause::For { .. })
                    })
                    .count();
                let binding = bindings
                    .get(binding_index)
                    .expect("checked comprehension binder metadata");
                let yield_ty = protocol
                    .next
                    .as_ref()
                    .map(|call| call.result_ty.clone())
                    .unwrap_or_else(|| binding.plan.yielded_ty.clone());
                let element_value =
                    self.fresh_typed(iter.source_span(), Some(iterator_object), yield_ty);
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = header;
                let has_next = self.fresh(iter.source_span(), Some(iterator_object));
                if let Some(exhaustion) = protocol.exhaustion.clone() {
                    self.emit(MirInstr::TryNext {
                        dest: element_value,
                        yielded: has_next,
                        iter: iterator_object,
                        call: *protocol
                            .next
                            .clone()
                            .expect("raising iterator has checked __next__ contract"),
                        exhaustion,
                    });
                } else {
                    self.emit(MirInstr::HasNext {
                        dest: has_next,
                        iter: iterator_object,
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
                        iter: iterator_object,
                        call: protocol.next.as_deref().cloned(),
                    });
                }
                let binding_var = self.var(&format!("$comp{}${}", var, binding.owner.0));
                // Retain the raw `__next__` result in a compiler-owned slot, then
                // adapt it to the comprehension target with the same checked
                // matrix the statement path uses via `BindIteration`.
                let raw_var = self.var(&format!("$compyield{}${}", var, binding.owner.0));
                self.var_types
                    .insert(raw_var, binding.plan.yielded_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: raw_var,
                    src: element_value,
                    binding_ty: Some(binding.plan.yielded_ty.clone()),
                });
                self.bind_iteration_result(
                    &binding.plan,
                    raw_var,
                    binding_var,
                    iterator_object,
                    binding.owner,
                );
                self.comprehension_clauses(clauses, bindings, index + 1, plan);
                self.f.blocks[self.cur].term = MirTerm::Jump(header);
                self.cur = exit;
                if split_source && !borrowed {
                    // An owned temporary source is used only by `GetIter` before
                    // the loop, so a liveness anchor at the exit keeps it live
                    // through the loop (a borrowing iterator still refers to its
                    // storage); then it is destroyed exactly once, after the
                    // loop. A borrowed source is kept live by its loan and owned
                    // (dropped) by its enclosing scope. Mirrors the statement
                    // loop's exit anchor.
                    self.emit(MirInstr::KeepAlive { var: iterator });
                    self.emit(MirInstr::DropVar { var: iterator });
                }
            }
        }
    }

    pub(in crate::mir) fn comprehension(
        &mut self,
        expression: &Expr,
        _kind: mojito_ast::ast::CollectionKind,
        key: Option<&Expr>,
        value: &Expr,
        clauses: &[mojito_ast::ast::ComprehensionClause],
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
}
