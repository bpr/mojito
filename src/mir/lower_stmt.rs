//! Statement, place, subscript-assignment, `try`-region, and terminator lowering,
//! including the `lower_stmt`/`lower_instr` dispatchers.
//! Extracted from `mir.rs`; see `docs/symbol-map.md`.

use super::*;

impl Flatten<'_> {
    /// Lower one straight-line HIR instruction into `self.cur`. `outer_map` is the
    /// enclosing **function**'s HIR→MIR block map, used to resolve a `try`'s
    /// escape targets (`break`/`continue` to an outer loop); most arms ignore it.
    pub(super) fn lower_instr(
        &mut self,
        i: &HirInstr,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        match i {
            HirInstr::Bind {
                dest,
                expr,
                binding_ty,
                binding,
            } => {
                let mut index = HashMap::new();
                index_hir_expression(&expr.syntax, expr, &mut index);
                self.active_semantics.push(index);
                let mut src = self.expr(&expr.syntax);
                if let Some(target) = binding_ty.as_ref() {
                    src = self.materialize_register(src, target, expr.source_span());
                }
                let writes_through_reference =
                    self.aliases.contains_key(dest) || self.runtime_aliases.contains(dest);
                if !writes_through_reference
                    && let Some(ty) = self
                        .f
                        .reg_types
                        .get(&src.0)
                        .cloned()
                        .or_else(|| expr.ty.clone())
                        .or_else(|| binding_ty.clone())
                {
                    self.var_types.insert(*dest, ty);
                }
                if let Some(binding) = binding {
                    self.owner_vars.insert(*binding, *dest);
                }
                // The initializer is evaluated before replacing the old
                // binding.  Whole-owner invalidation therefore sits here,
                // immediately before the write to the destination slot.
                self.emit_interior_invalidations(&expr.syntax, Some(*dest));
                if let Some(loan) = self.aliases.get(dest).cloned() {
                    let mut place = loan.place;
                    place.through = Some(*dest);
                    self.emit(MirInstr::Store { place, src });
                } else if self.runtime_aliases.contains(dest) {
                    let handle = self.fresh(expr.source_span(), Some(*dest));
                    self.emit(MirInstr::MakeRef {
                        dest: handle,
                        place: {
                            let mut place =
                                MirPlace::root(*dest, self.var_types.get(dest).cloned());
                            place.through = Some(*dest);
                            place
                        },
                    });
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: src,
                    });
                } else {
                    self.emit(MirInstr::DefVar {
                        var: *dest,
                        src,
                        binding_ty: binding_ty.clone(),
                    });
                    let aggregate_loans = self.aggregate_borrows(expr);
                    if let Some(first) = aggregate_loans.first() {
                        let marker =
                            self.fresh_typed(expr.source_span(), Some(first.place.root), Ty::None);
                        self.emit(MirInstr::EstablishLoans {
                            reference: *dest,
                            loans: aggregate_loans.clone(),
                            marker,
                            dest_interior: None,
                        });
                    }
                    if aggregate_loans.is_empty() {
                        self.aggregate_loans.remove(dest);
                    } else {
                        self.aggregate_loans.insert(*dest, aggregate_loans);
                    }
                }
                self.active_semantics.pop();
            }
            HirInstr::BorrowIter { dest, expr, origin } => {
                let place = self.place_hir(expr);
                let value_ty = expr
                    .ty
                    .clone()
                    .or_else(|| place.ty.clone())
                    .expect("checked borrowed iterator place has a type");
                self.borrow_iteration_source(*dest, expr.source_span(), place, value_ty, origin);
            }
            HirInstr::Eval(e) => {
                let _ = self.expr_hir(e); // evaluated for its effect; result discarded
            }
            HirInstr::Stmt(s) => self.lower_hir_stmt(s, outer_map),
            // A `try` whose enclosing loops are function-level: lower each sub-region
            // seeded with those loops (`loop_targets`, HIR function block ids), so an
            // outward `break`/`continue` becomes an `EscapeJump` resolved via
            // `outer_map`.
            HirInstr::Try { stmt, loop_targets } => {
                let mut index = HashMap::new();
                for (syntax, expression) in statement_expression_roots(&stmt.syntax)
                    .into_iter()
                    .zip(&stmt.expressions)
                {
                    index_hir_expression(syntax, expression, &mut index);
                }
                self.active_semantics.push(index);
                if let StmtKind::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } = &stmt.syntax.kind
                {
                    self.emit_try(
                        TryRegions {
                            body,
                            except,
                            orelse,
                            finalbody,
                            handler_binding: stmt.binding,
                        },
                        loop_targets,
                        outer_map,
                    );
                } else {
                    self.emit(MirInstr::Unsupported(
                        "malformed HIR try instruction".to_string(),
                    ));
                }
                self.active_semantics.pop();
            }
            HirInstr::Drop(var) => {
                self.emit(MirInstr::DropVar { var: *var });
            }
            HirInstr::KeepAlive(var) => {
                self.emit(MirInstr::KeepAlive { var: *var });
            }
            HirInstr::FinishIter { iter, call } => {
                // Consume the exhausted linear iterator through its named
                // destructor: an ordinary method call whose moved receiver is
                // the iterator slot's value, so drop elaboration sees the slot
                // consumed rather than needing drop glue it cannot have.
                let span = SourceSpan::new(None, DUMMY_SPAN);
                let recv = match self.var_types.get(iter).cloned() {
                    Some(ty) => self.fresh_typed(span.clone(), Some(*iter), ty),
                    None => self.fresh(span.clone(), Some(*iter)),
                };
                self.emit(MirInstr::UseVar {
                    dest: recv,
                    var: *iter,
                    mode: UseMode::Move,
                });
                let dest = self.fresh_typed(span, None, call.result_ty.clone());
                self.emit(MirInstr::MethodCall {
                    dest,
                    recv,
                    method: "_finish".to_string(),
                    resolved: Some(call.target.clone()),
                    raises: call.raises.clone(),
                    reference_result: call.reference_result.clone(),
                    result_adapter: call.result_adapter,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    recv_place: None,
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                });
            }
            // Iterator protocol: compute into a register, then store to the target
            // variable (so the header's branch can read `has_next` as a `UseVar`,
            // and the body binds the loop variable).
            HirInstr::GetIter {
                source,
                dest,
                protocol,
            } => {
                self.emit(MirInstr::GetIter {
                    source: *source,
                    dest: *dest,
                    mode: protocol.mode,
                    prepare: protocol.prepare.clone(),
                });
                // A borrowed source is normalized into its own iterator slot
                // (`source != dest`); re-establish its source loans on the
                // long-lived iterator variable so they stay live through the
                // whole loop, rejecting mutation of the source during iteration.
                if protocol.borrowed_origin.is_some() && source != dest {
                    self.reestablish_source_loans(*source, *dest);
                }
            }
            HirInstr::HasNext { iter, dest, method } => {
                let r = self.fresh(SourceSpan::new(None, DUMMY_SPAN), None);
                self.emit(MirInstr::HasNext {
                    dest: r,
                    iter: *iter,
                    method: method.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *dest,
                    src: r,
                    binding_ty: None,
                });
            }
            HirInstr::Next {
                iter,
                raw,
                call,
                element_ty,
            } => {
                let r = self.fresh_typed(
                    SourceSpan::new(None, DUMMY_SPAN),
                    Some(*iter),
                    call.as_ref()
                        .map(|call| call.result_ty.clone())
                        .unwrap_or_else(|| element_ty.clone()),
                );
                self.emit(MirInstr::Next {
                    dest: r,
                    iter: *iter,
                    call: call.clone(),
                });
                // The raw `__next__` result is retained untouched in a
                // compiler-owned slot; `BindIteration` adapts it to the loop
                // target only on the yielded edge.
                self.emit(MirInstr::DefVar {
                    var: *raw,
                    src: r,
                    binding_ty: Some(element_ty.clone()),
                });
            }
            HirInstr::TryNext {
                iter,
                raw,
                yielded,
                call,
                exhaustion,
                element_ty,
            } => {
                let element = self.fresh_typed(
                    SourceSpan::new(None, DUMMY_SPAN),
                    Some(*iter),
                    call.result_ty.clone(),
                );
                let has_element = self.fresh(SourceSpan::new(None, DUMMY_SPAN), Some(*iter));
                self.emit(MirInstr::TryNext {
                    dest: element,
                    yielded: has_element,
                    iter: *iter,
                    call: call.clone(),
                    exhaustion: exhaustion.clone(),
                });
                self.emit(MirInstr::DefVar {
                    var: *raw,
                    src: element,
                    binding_ty: Some(element_ty.clone()),
                });
                self.emit(MirInstr::DefVar {
                    var: *yielded,
                    src: has_element,
                    binding_ty: Some(Ty::Bool),
                });
            }
            HirInstr::BindIteration {
                raw,
                dest,
                iter,
                plan,
                binding,
            } => {
                self.bind_iteration_result(plan, *raw, *dest, *iter, *binding);
            }
        }
    }

    /// Adapt one raw `__next__` result (`raw`) into the user-visible loop target
    /// (`dest`) per the checked [`CheckedIterationBinding`]. This runs only in the
    /// yielded body, so moves and lifecycle copies never execute on the
    /// `StopIteration` edge. Source ownership (borrowed vs consuming `__iter__`)
    /// is already fixed upstream; here only the target convention matters.
    pub(super) fn bind_iteration_result(
        &mut self,
        plan: &crate::checked::CheckedIterationBinding,
        raw: VarId,
        dest: VarId,
        iterator: VarId,
        binding: crate::origin::OwnerId,
    ) {
        use crate::checked::IterationBindingAction;
        let span = SourceSpan::new(None, DUMMY_SPAN);
        self.owner_vars.insert(binding, dest);
        match plan.action {
            // A value result binds directly into the loop variable's own
            // storage: `var` owns and may transfer it onward; an immutable or
            // `ref` target (`BorrowValue`) instead drops it at each iteration's
            // end. Both simply move the freshly yielded value into `dest`; the
            // binding's declared mutability enforces the difference.
            IterationBindingAction::MoveValue | IterationBindingAction::BorrowValue => {
                let value = self.fresh_typed(span.clone(), Some(raw), plan.binding_ty.clone());
                self.emit(MirInstr::UseVar {
                    dest: value,
                    var: raw,
                    mode: UseMode::Move,
                });
                self.var_types.insert(dest, plan.binding_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: dest,
                    src: value,
                    binding_ty: Some(plan.binding_ty.clone()),
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
                let owned = self.fresh_typed(span.clone(), None, plan.binding_ty.clone());
                self.emit(MirInstr::CopyValue {
                    dest: owned,
                    value: read,
                });
                self.var_types.insert(dest, plan.binding_ty.clone());
                self.emit(MirInstr::DefVar {
                    var: dest,
                    src: owned,
                    binding_ty: Some(plan.binding_ty.clone()),
                });
            }
            // Immutable/`ref` target over a reference result: keep the yielded
            // handle as the binding so body accesses read/write through it.
            // `MakeRef` forwards `raw`'s handle unchanged.
            IterationBindingAction::BorrowReference => {
                let handle = self.fresh_typed(span.clone(), Some(raw), plan.binding_ty.clone());
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
                });
                // The borrowed handle aliases the iterated source: re-establish
                // the iterator's source loans on the binding so a structural
                // invalidation names the user's variable, not just the
                // compiler's iterator slot.
                self.reestablish_source_loans(iterator, dest);
            }
        }
    }

    /// Bind a borrowed iteration source into its retained-source slot: a genuine
    /// reference into the source place (never a value copy), with the owner
    /// dependency established at the granularity the checker proved — an interior
    /// element generation when the origin ends in an `Interior` segment, a
    /// whole-place shared loan otherwise. The reference itself always designates
    /// the whole retained source; granularity lives only in the loan. Shared by
    /// `for` statements (`HirInstr::BorrowIter`) and comprehension clauses.
    pub(super) fn borrow_iteration_source(
        &mut self,
        dest: VarId,
        span: SourceSpan,
        place: MirPlace,
        value_ty: Ty,
        origin: &crate::origin::OriginPlace,
    ) {
        let canonical = self
            .mir_interior_origin(origin, Some(place.root))
            .expect("checked borrowed iteration origin has a MIR owner");
        let interior = canonical
            .path
            .iter()
            .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
            .then_some(canonical);
        let mut ref_origin = origin.clone();
        if let Some(first_interior) = ref_origin
            .path
            .iter()
            .position(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
        {
            ref_origin.path.truncate(first_interior);
        }
        // The slot holds only a handle: `GetIter` reads it to normalize the
        // iterator (re-rooting a `ref self` `__iter__` at the source), after
        // which dropping the handle is a no-op. The source is never copied.
        let ref_ty = Ty::Ref(crate::origin::RefTy {
            referent: Box::new(value_ty),
            origin: crate::origin::Origin::Place(ref_origin),
            mutability: crate::origin::Mutability::Immutable,
        });
        self.var_types.insert(dest, ref_ty.clone());
        let handle = self.fresh_typed(span.clone(), Some(place.root), ref_ty.clone());
        self.emit(MirInstr::MakeRef {
            dest: handle,
            place: place.clone(),
        });
        self.emit(MirInstr::DefVar {
            var: dest,
            src: handle,
            binding_ty: Some(ref_ty),
        });
        let loans = vec![MirLoan {
            place,
            mutable: false,
            interior,
        }];
        let marker = self.fresh_typed(span, Some(loans[0].place.root), Ty::None);
        self.emit(MirInstr::EstablishLoans {
            reference: dest,
            loans: loans.clone(),
            marker,
            dest_interior: None,
        });
        self.aggregate_loans.insert(dest, loans);
    }

    /// Copy a normalized borrowed source's loans onto the long-lived iterator
    /// slot so the source dependency survives for the whole loop even though the
    /// retained-source slot's last read is the normalization itself. Shared by
    /// `for` statements and comprehension clauses.
    pub(super) fn reestablish_source_loans(&mut self, source: VarId, iterator: VarId) {
        if let Some(loans) = self.aggregate_loans.get(&source).cloned()
            && let Some(first) = loans.first()
        {
            let marker = self.fresh_typed(
                SourceSpan::new(None, DUMMY_SPAN),
                Some(first.place.root),
                Ty::None,
            );
            self.emit(MirInstr::EstablishLoans {
                reference: iterator,
                loans: loans.clone(),
                marker,
                dest_interior: None,
            });
            self.aggregate_loans.insert(iterator, loans);
        }
    }

    /// Decompose a place expression (`x`, `p.a.b`, `xs[i]`, `p.items[i].x`) into a
    /// [`MirPlace`] — a root variable plus a projection chain — flattening any
    /// subscript index into a register **once**. The checker guarantees the root
    /// is a variable (or `self`), so a non-variable root is unreachable.
    pub(super) fn place(&mut self, e: &Expr) -> MirPlace {
        match &e.kind {
            ExprKind::Identifier(name) => self.expression_place_root(name, e),
            ExprKind::Member { object, field } => {
                let mut p = self.place(object);
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                p
            }
            ExprKind::Index { object, index } => {
                let mut p = self.place(object);
                // Compiler-private inline uninit storage: `storage[0]` is the
                // payload place (`Proj::UninitPayload`), not a dynamic index.
                let object_ty = self.checked_ty(object);
                if let Some(element) = object_ty
                    .as_ref()
                    .and_then(crate::types::uninit_storage_element)
                {
                    p.project(Proj::UninitPayload, element.clone());
                    return p;
                }
                let idx = self.expr(index); // evaluated once, before the store
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Index(idx), ty);
                } else {
                    p.proj.push(Proj::Index(idx));
                }
                p
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })
                    .expect("only checked Variant projection is a place TypeApply");
                let mut p = self.resolved_place(name);
                let ty = self
                    .checked_place_ty(e)
                    .or_else(|| self.checked_ty(e))
                    .expect("checked Variant projection has a payload type");
                p.project(Proj::Variant(index), ty);
                p
            }
            other => {
                self.emit(MirInstr::Unsupported(format!(
                    "invalid assignment place reached MIR lowering: {other:?}"
                )));
                MirPlace::root(self.var("$invalid_place"), None)
            }
        }
    }

    /// Lower `receiver[arguments] OP= rhs` from the complete checked accessor
    /// contracts. A value getter evaluates raw receiver/index sources, then the
    /// RHS, then getter-specific adaptations and the getter; the result is sent
    /// through independently adapted setter arguments. A mutable-reference
    /// getter instead establishes the lvalue before the RHS and finishes with a
    /// direct `WriteRef`, exactly as current Mojo does. In both paths each source
    /// expression and slice bound is evaluated once.
    /// Apply a nominal-subscript element's in-place dunder (`c[i] += v` →
    /// `element.__iadd__(v)`). `current` is the element value read from the getter
    /// (value getter) or through the reference handle (`ReadRef`, ref getter). It
    /// is bound to a fresh mutable temporary so the `mut self` call commits through
    /// `recv_place`, then the mutated element is read back and returned for the
    /// setter or `WriteRef` step. This reuses the variable in-place mechanism, so
    /// the VM is unchanged.
    pub(super) fn emit_augmented_inplace(
        &mut self,
        current: Reg,
        contract: &crate::checked::CheckedCallContract,
        rhs: Reg,
        operand_ty: &Ty,
        op: InfixOp,
        span: SourceSpan,
    ) -> Reg {
        let tmp = self.fresh_var();
        self.var_types.insert(tmp, operand_ty.clone());
        self.emit(MirInstr::DefVar {
            var: tmp,
            src: current,
            binding_ty: Some(operand_ty.clone()),
        });
        let recv = self.fresh_typed(span.clone(), Some(tmp), operand_ty.clone());
        self.emit(MirInstr::UseVar {
            dest: recv,
            var: tmp,
            mode: UseMode::BorrowMut,
        });
        self.emit_checked_call_boundary(contract, span.clone());
        let dest = self.fresh(span.clone(), None);
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method: op
                .inplace_dunder()
                .expect("augmented in-place operator has an in-place dunder")
                .to_string(),
            resolved: Some(contract.target.clone()),
            raises: contract.raises.clone(),
            reference_result: contract.reference_result.clone(),
            result_adapter: contract.result_adapter,
            args: vec![rhs],
            kwargs: Vec::new(),
            recv_place: Some(MirPlace::root(tmp, Some(operand_ty.clone()))),
            arg_places: vec![None],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: contract.param_decls.clone(),
        });
        let updated = self.fresh_typed(span, Some(tmp), operand_ty.clone());
        self.emit(MirInstr::UseVar {
            dest: updated,
            var: tmp,
            mode: UseMode::Move,
        });
        updated
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
        place: &Expr,
        op: InfixOp,
        rhs_expression: &Expr,
    ) -> bool {
        let Some(contract) = self.augmented_in_place_contract(place) else {
            return false;
        };
        let (recv, recv_place) = self.lower_call_receiver(place);
        let (args, arg_places) = self.lower_call_arguments(std::slice::from_ref(rhs_expression));
        let dest = self.fresh(place.source_span(), None);
        self.emit_interior_invalidations(place, None);
        self.emit_checked_call_boundary(&contract, place.source_span());
        let method = op
            .inplace_dunder()
            .expect("augmented in-place operator has an in-place dunder")
            .to_string();
        self.emit(MirInstr::MethodCall {
            dest,
            recv,
            method,
            resolved: Some(contract.target.clone()),
            raises: contract.raises.clone(),
            reference_result: contract.reference_result.clone(),
            result_adapter: contract.result_adapter,
            args,
            kwargs: Vec::new(),
            recv_place,
            arg_places,
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: contract.param_decls.clone(),
        });
        self.emit_nested_closure_argument_keepalives(std::slice::from_ref(rhs_expression), &[]);
        true
    }

    /// `receiver[index] = value` through a mutable-reference `__getitem__`
    /// when no `__setitem__` exists (the checker recorded an augmented
    /// reference write with no operator): the getter's handle is written
    /// directly, with no read-back or operator step.
    pub(super) fn lower_subscript_reference_set(
        &mut self,
        target: &Expr,
        rhs_expression: &Expr,
    ) -> bool {
        let Some(plan) = self.checked_augmented_subscript(target) else {
            return false;
        };
        let ExprKind::Index { object, index } = &target.kind else {
            return false;
        };
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let index_source = crate::checked::CheckedCallArgumentSource::Positional(0);
        let retain_index = Self::checked_call_source_requires_place(&plan.getter, index_source);
        let (raw_index, index_place) = self.lower_augmented_argument_source(index, retain_index);
        let getter_index = self.apply_checked_call_value_adjustments(
            &plan.getter,
            index_source,
            raw_index,
            index.source_span(),
        );
        let getter_call = self.mir_subscript_call_contract(
            plan.getter.clone(),
            &[(index.source_span(), getter_index)],
        );
        self.emit_checked_call_boundary(&plan.getter, target.source_span());
        let handle = self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
        self.emit(MirInstr::Index {
            dest: handle,
            base: receiver,
            index: getter_index,
            base_place: receiver_place,
            index_place: Self::checked_call_source_place(&plan.getter, index_source, &index_place),
            call: Some(getter_call),
            intrinsic: None,
        });
        let handle = self.peel_reference_handle_to(handle, &plan.operand_ty, target.source_span());
        let rhs = self.expr(rhs_expression);
        self.emit_interior_invalidations(target, None);
        self.emit(MirInstr::WriteRef {
            reference: handle,
            value: rhs,
        });
        true
    }

    pub(super) fn lower_augmented_subscript(
        &mut self,
        target: &Expr,
        op: InfixOp,
        rhs_expression: &Expr,
    ) -> bool {
        let Some(plan) = self.checked_augmented_subscript(target) else {
            return false;
        };
        let (descriptors, value_keyword) = self
            .checked_adjustments(target)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    set_value_keyword,
                } => Some((descriptors, set_value_keyword)),
                _ => None,
            })
            .expect("checked augmented subscript has descriptor metadata");

        match &target.kind {
            ExprKind::Index { object, index } => {
                debug_assert_eq!(descriptors, vec![None]);
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let index_source = crate::checked::CheckedCallArgumentSource::Positional(0);
                let retain_index =
                    Self::checked_call_source_requires_place(&plan.getter, index_source)
                        || plan.setter.as_ref().is_some_and(|setter| {
                            Self::checked_call_source_requires_place(setter, index_source)
                        });
                let (raw_index, index_place) =
                    self.lower_augmented_argument_source(index, retain_index);

                if plan.setter.is_none() {
                    let getter_index = self.apply_checked_call_value_adjustments(
                        &plan.getter,
                        index_source,
                        raw_index,
                        index.source_span(),
                    );
                    let getter_call = self.mir_subscript_call_contract(
                        plan.getter.clone(),
                        &[(index.source_span(), getter_index)],
                    );
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::Index {
                        dest: handle,
                        base: receiver,
                        index: getter_index,
                        base_place: receiver_place,
                        index_place: Self::checked_call_source_place(
                            &plan.getter,
                            index_source,
                            &index_place,
                        ),
                        call: Some(getter_call),
                        intrinsic: None,
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result = if let Some(inplace) = &plan.inplace {
                        self.emit_augmented_inplace(
                            current,
                            inplace,
                            rhs,
                            &plan.operand_ty,
                            op,
                            target.source_span(),
                        )
                    } else {
                        let result =
                            self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                        self.emit(MirInstr::BinOp {
                            op,
                            dest: result,
                            a: current,
                            b: rhs,
                            resolved: None,
                        });
                        result
                    };
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                // Value-getter ordering is raw receiver/index, RHS, accessor
                // adaptation/getter, operator, setter adaptation/setter.
                let rhs = self.expr(rhs_expression);
                let getter_index = self.apply_checked_call_value_adjustments(
                    &plan.getter,
                    index_source,
                    raw_index,
                    index.source_span(),
                );
                let getter_call = self.mir_subscript_call_contract(
                    plan.getter.clone(),
                    &[(index.source_span(), getter_index)],
                );
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::Index {
                    dest: current,
                    base: receiver,
                    index: getter_index,
                    base_place: receiver_place.clone(),
                    index_place: Self::checked_call_source_place(
                        &plan.getter,
                        index_source,
                        &index_place,
                    ),
                    call: Some(getter_call),
                    intrinsic: None,
                });
                let result = if let Some(inplace) = &plan.inplace {
                    self.emit_augmented_inplace(
                        current,
                        inplace,
                        rhs,
                        &plan.operand_ty,
                        op,
                        target.source_span(),
                    )
                } else {
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    result
                };

                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let setter_raw_index = self.reload_augmented_source(
                    raw_index,
                    &index_place,
                    Self::checked_call_source_mutates(&plan.getter, index_source),
                    index.source_span(),
                );
                let setter_index = self.apply_checked_call_value_adjustments(
                    setter,
                    index_source,
                    setter_raw_index,
                    index.source_span(),
                );
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(1)
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let setter_sources = [
                    (index.source_span(), setter_index),
                    (
                        plan.value_source
                            .clone()
                            .unwrap_or_else(|| target.source_span()),
                        value,
                    ),
                ];
                let setter_call = self.mir_subscript_call_contract(setter.clone(), &setter_sources);
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: vec![MirSubscriptArg::Index(setter_index)],
                    arg_places: vec![Self::checked_call_source_place(
                        setter,
                        index_source,
                        &index_place,
                    )],
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                let kind = descriptors
                    .first()
                    .copied()
                    .flatten()
                    .expect("augmented slice has a descriptor kind");
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let lower_reg = lower.as_ref().map(|bound| self.expr(bound));
                let upper_reg = upper.as_ref().map(|bound| self.expr(bound));
                let step_reg = step.as_ref().map(|bound| self.expr(bound));
                let getter_call = self.mir_subscript_call_contract(plan.getter.clone(), &[]);
                if plan.setter.is_none() {
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::Slice {
                        dest: handle,
                        object: receiver,
                        kind,
                        lower: lower_reg,
                        upper: upper_reg,
                        step: step_reg,
                        object_place: receiver_place,
                        arg_places: vec![None],
                        call: Some(getter_call),
                        intrinsic: None,
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result = if let Some(inplace) = &plan.inplace {
                        self.emit_augmented_inplace(
                            current,
                            inplace,
                            rhs,
                            &plan.operand_ty,
                            op,
                            target.source_span(),
                        )
                    } else {
                        let result =
                            self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                        self.emit(MirInstr::BinOp {
                            op,
                            dest: result,
                            a: current,
                            b: rhs,
                            resolved: None,
                        });
                        result
                    };
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                let rhs = self.expr(rhs_expression);
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::Slice {
                    dest: current,
                    object: receiver,
                    kind,
                    lower: lower_reg,
                    upper: upper_reg,
                    step: step_reg,
                    object_place: receiver_place.clone(),
                    arg_places: vec![None],
                    call: Some(getter_call),
                    intrinsic: None,
                });
                let result = if let Some(inplace) = &plan.inplace {
                    self.emit_augmented_inplace(
                        current,
                        inplace,
                        rhs,
                        &plan.operand_ty,
                        op,
                        target.source_span(),
                    )
                } else {
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    result
                };
                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(1)
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let setter_call = self.mir_subscript_call_contract(
                    setter.clone(),
                    &[(
                        plan.value_source
                            .clone()
                            .unwrap_or_else(|| target.source_span()),
                        value,
                    )],
                );
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: vec![MirSubscriptArg::Slice {
                        kind,
                        lower: lower_reg,
                        upper: upper_reg,
                        step: step_reg,
                    }],
                    arg_places: vec![None],
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            ExprKind::MultiIndex {
                object,
                args: source,
            } => {
                let (receiver, receiver_place) = self.lower_call_receiver(object);
                let mut source_places = Vec::with_capacity(source.len());
                let mut source_spans = Vec::with_capacity(source.len());
                let raw_args = source
                    .iter()
                    .zip(&descriptors)
                    .enumerate()
                    .map(|(position, (argument, descriptor))| match argument {
                        crate::ast::SubscriptArg::Keyword { .. }
                        | crate::ast::SubscriptArg::KeywordSlice { .. } => {
                            unreachable!("keyword subscript assignment is rejected at checking")
                        }
                        crate::ast::SubscriptArg::Index(index) => {
                            debug_assert!(descriptor.is_none());
                            let argument_source =
                                crate::checked::CheckedCallArgumentSource::Positional(position);
                            let retain_place = Self::checked_call_source_requires_place(
                                &plan.getter,
                                argument_source,
                            ) || plan.setter.as_ref().is_some_and(|setter| {
                                Self::checked_call_source_requires_place(setter, argument_source)
                            });
                            let (register, place) =
                                self.lower_augmented_argument_source(index, retain_place);
                            source_places.push(place);
                            source_spans.push(Some(index.source_span()));
                            MirSubscriptArg::Index(register)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            source_places.push(None);
                            source_spans.push(None);
                            MirSubscriptArg::Slice {
                                kind: descriptor.expect("slice argument has a descriptor kind"),
                                lower: lower.as_ref().map(|bound| self.expr(bound)),
                                upper: upper.as_ref().map(|bound| self.expr(bound)),
                                step: step.as_ref().map(|bound| self.expr(bound)),
                            }
                        }
                    })
                    .collect::<Vec<_>>();
                // Value getters defer every call-local argument adaptation
                // until after the RHS. Building the raw descriptor/index list
                // above has already performed each source evaluation once.
                let value_rhs = plan.setter.as_ref().map(|_| self.expr(rhs_expression));
                let getter_args = raw_args
                    .iter()
                    .enumerate()
                    .map(|(position, argument)| match argument {
                        MirSubscriptArg::Index(register) => MirSubscriptArg::Index(
                            self.apply_checked_call_value_adjustments(
                                &plan.getter,
                                crate::checked::CheckedCallArgumentSource::Positional(position),
                                *register,
                                source_spans[position]
                                    .clone()
                                    .unwrap_or_else(|| target.source_span()),
                            ),
                        ),
                        slice => slice.clone(),
                    })
                    .collect::<Vec<_>>();
                let getter_sources = getter_args
                    .iter()
                    .zip(&source_spans)
                    .filter_map(|(argument, source)| match (argument, source) {
                        (MirSubscriptArg::Index(register), Some(source)) => {
                            Some((source.clone(), *register))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let getter_places = source_places
                    .iter()
                    .enumerate()
                    .map(|(position, place)| {
                        Self::checked_call_source_place(
                            &plan.getter,
                            crate::checked::CheckedCallArgumentSource::Positional(position),
                            place,
                        )
                    })
                    .collect::<Vec<_>>();
                let getter_call =
                    self.mir_subscript_call_contract(plan.getter.clone(), &getter_sources);

                if plan.setter.is_none() {
                    self.emit_checked_call_boundary(&plan.getter, target.source_span());
                    let handle =
                        self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                    self.emit(MirInstr::MultiIndex {
                        dest: handle,
                        object: receiver,
                        args: getter_args,
                        object_place: receiver_place,
                        arg_places: getter_places,
                        kwargs: Vec::new(),
                        kwarg_places: Vec::new(),
                        call: Some(getter_call),
                    });
                    let handle = self.peel_reference_handle_to(
                        handle,
                        &plan.operand_ty,
                        target.source_span(),
                    );
                    let rhs = self.expr(rhs_expression);
                    let current =
                        self.fresh_typed(target.source_span(), None, plan.operand_ty.clone());
                    self.emit(MirInstr::ReadRef {
                        dest: current,
                        reference: handle,
                    });
                    let result = if let Some(inplace) = &plan.inplace {
                        self.emit_augmented_inplace(
                            current,
                            inplace,
                            rhs,
                            &plan.operand_ty,
                            op,
                            target.source_span(),
                        )
                    } else {
                        let result =
                            self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                        self.emit(MirInstr::BinOp {
                            op,
                            dest: result,
                            a: current,
                            b: rhs,
                            resolved: None,
                        });
                        result
                    };
                    self.emit(MirInstr::WriteRef {
                        reference: handle,
                        value: result,
                    });
                    return true;
                }

                let rhs = value_rhs.expect("value-returning augmented getter has an RHS");
                self.emit_checked_call_boundary(&plan.getter, target.source_span());
                let current =
                    self.fresh_typed(target.source_span(), None, plan.getter.result_ty.clone());
                self.emit(MirInstr::MultiIndex {
                    dest: current,
                    object: receiver,
                    args: getter_args,
                    object_place: receiver_place.clone(),
                    arg_places: getter_places,
                    kwargs: Vec::new(),
                    kwarg_places: Vec::new(),
                    call: Some(getter_call),
                });
                let result = if let Some(inplace) = &plan.inplace {
                    self.emit_augmented_inplace(
                        current,
                        inplace,
                        rhs,
                        &plan.operand_ty,
                        op,
                        target.source_span(),
                    )
                } else {
                    let result =
                        self.fresh_typed(target.source_span(), None, plan.result_ty.clone());
                    self.emit(MirInstr::BinOp {
                        op,
                        dest: result,
                        a: current,
                        b: rhs,
                        resolved: None,
                    });
                    result
                };
                let setter = plan
                    .setter
                    .as_ref()
                    .expect("value-returning augmented getter has a setter");
                let setter_receiver = self.reload_augmented_source(
                    receiver,
                    &receiver_place,
                    matches!(
                        plan.getter.receiver_convention,
                        Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                    ),
                    object.source_span(),
                );
                let setter_args = raw_args
                    .iter()
                    .enumerate()
                    .map(|(position, argument)| match argument {
                        MirSubscriptArg::Index(register) => {
                            let source_kind =
                                crate::checked::CheckedCallArgumentSource::Positional(position);
                            let raw = self.reload_augmented_source(
                                *register,
                                &source_places[position],
                                Self::checked_call_source_mutates(&plan.getter, source_kind),
                                source_spans[position]
                                    .clone()
                                    .unwrap_or_else(|| target.source_span()),
                            );
                            MirSubscriptArg::Index(
                                self.apply_checked_call_value_adjustments(
                                    setter,
                                    source_kind,
                                    raw,
                                    source_spans[position]
                                        .clone()
                                        .unwrap_or_else(|| target.source_span()),
                                ),
                            )
                        }
                        slice => slice.clone(),
                    })
                    .collect::<Vec<_>>();
                let setter_places = source_places
                    .iter()
                    .enumerate()
                    .map(|(position, place)| {
                        Self::checked_call_source_place(
                            setter,
                            crate::checked::CheckedCallArgumentSource::Positional(position),
                            place,
                        )
                    })
                    .collect::<Vec<_>>();
                let value_source = if value_keyword {
                    crate::checked::CheckedCallArgumentSource::Keyword(0)
                } else {
                    crate::checked::CheckedCallArgumentSource::Positional(source.len())
                };
                let value = self.apply_checked_call_value_adjustments(
                    setter,
                    value_source,
                    result,
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                );
                let mut setter_sources = setter_args
                    .iter()
                    .zip(&source_spans)
                    .filter_map(|(argument, source)| match (argument, source) {
                        (MirSubscriptArg::Index(register), Some(source)) => {
                            Some((source.clone(), *register))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                setter_sources.push((
                    plan.value_source
                        .clone()
                        .unwrap_or_else(|| target.source_span()),
                    value,
                ));
                let setter_call = self.mir_subscript_call_contract(setter.clone(), &setter_sources);
                self.emit_checked_call_boundary(setter, target.source_span());
                self.emit(MirInstr::MultiSet {
                    receiver: setter_receiver,
                    receiver_place,
                    args: setter_args,
                    arg_places: setter_places,
                    value,
                    value_place: None,
                    value_keyword,
                    call: setter_call,
                });
                true
            }
            _ => false,
        }
    }

    /// Lower a slice or multidimensional assignment through the checker-selected
    /// `__setitem__` implementation. Unlike an ordinary `MirPlace` projection,
    /// every slice remains a first-class descriptor argument and the receiver
    /// place is retained for `mut self` write-back.
    pub(super) fn lower_subscript_set(&mut self, target: &Expr, value_expression: &Expr) -> bool {
        if self.checked_call_contract(target).is_none() {
            return false;
        }
        if let ExprKind::Index { object, index } = &target.kind {
            let Some(value_keyword) = self.checked_adjustments(target).into_iter().find_map(
                |adjustment| match adjustment {
                    crate::SemanticAdjustment::SliceDescriptors {
                        descriptors,
                        set_value_keyword,
                    } if descriptors.as_slice() == [None] => Some(set_value_keyword),
                    _ => None,
                },
            ) else {
                return false;
            };
            let (receiver, receiver_place) = self.lower_call_receiver(object);
            let (argument_register, argument_place) = self.lower_call_argument(index);
            let argument = MirSubscriptArg::Index(argument_register);
            let (value, value_place) = self.lower_assignment_value(target, value_expression);
            let call = self
                .subscript_call_contract(
                    target,
                    &[
                        (index.source_span(), argument_register),
                        (value_expression.source_span(), value),
                    ],
                )
                .expect("checked nominal subscript setter has a call contract");
            self.emit_interior_invalidations(index, None);
            self.emit_interior_invalidations(value_expression, None);
            self.emit_interior_invalidations(target, None);
            self.emit(MirInstr::MultiSet {
                receiver,
                receiver_place,
                args: vec![argument],
                arg_places: vec![argument_place],
                value,
                value_place,
                value_keyword,
                call,
            });
            return true;
        }
        let (object, source_arguments): (&Expr, Option<&[crate::ast::SubscriptArg]>) =
            match &target.kind {
                ExprKind::Slice { object, .. } => (object, None),
                ExprKind::MultiIndex { object, args } => (object, Some(args)),
                _ => return false,
            };
        let Some((descriptors, value_keyword)) = self
            .checked_adjustments(target)
            .into_iter()
            .find_map(|adjustment| match adjustment {
                crate::SemanticAdjustment::SliceDescriptors {
                    descriptors,
                    set_value_keyword,
                } => Some((descriptors, set_value_keyword)),
                _ => None,
            })
        else {
            self.emit(MirInstr::Unsupported(
                "checked subscript assignment lacks descriptor metadata".to_string(),
            ));
            return true;
        };

        // Current Mojo evaluates the nominal receiver first, then indices and
        // bounds from left to right, and finally the assignment RHS.
        let (receiver, receiver_place) = self.lower_call_receiver(object);
        let mut arg_places = Vec::with_capacity(descriptors.len());
        let mut parameter_sources = Vec::new();
        let args = if let Some(arguments) = source_arguments {
            arguments
                .iter()
                .zip(descriptors)
                .map(|(argument, descriptor)| match argument {
                    crate::ast::SubscriptArg::Keyword { .. }
                    | crate::ast::SubscriptArg::KeywordSlice { .. } => {
                        unreachable!("keyword subscript assignment is rejected at checking")
                    }
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
                            kind: descriptor
                                .expect("slice assignment argument has descriptor kind"),
                            lower: lower.as_ref().map(|bound| self.expr(bound)),
                            upper: upper.as_ref().map(|bound| self.expr(bound)),
                            step: step.as_ref().map(|bound| self.expr(bound)),
                        }
                    }
                })
                .collect()
        } else {
            arg_places.push(None);
            let ExprKind::Slice {
                lower, upper, step, ..
            } = &target.kind
            else {
                unreachable!("single descriptor assignment is a Slice")
            };
            vec![MirSubscriptArg::Slice {
                kind: descriptors
                    .first()
                    .copied()
                    .flatten()
                    .expect("slice assignment has descriptor kind"),
                lower: lower.as_ref().map(|bound| self.expr(bound)),
                upper: upper.as_ref().map(|bound| self.expr(bound)),
                step: step.as_ref().map(|bound| self.expr(bound)),
            }]
        };
        let (value, value_place) = self.lower_assignment_value(target, value_expression);
        parameter_sources.push((value_expression.source_span(), value));
        let call = self
            .subscript_call_contract(target, &parameter_sources)
            .expect("checked nominal subscript setter has a call contract");
        if let Some(arguments) = source_arguments {
            for argument in arguments {
                if let crate::ast::SubscriptArg::Index(argument) = argument {
                    self.emit_interior_invalidations(argument, None);
                }
            }
        }
        self.emit_interior_invalidations(value_expression, None);
        self.emit_interior_invalidations(target, None);
        self.emit(MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            value_keyword,
            call,
        });
        true
    }

    pub(super) fn lower_assignment_value(
        &mut self,
        target: &Expr,
        value_expression: &Expr,
    ) -> (Reg, Option<MirPlace>) {
        let (mut value, value_place) = self.lower_call_argument(value_expression);
        if let Some(target_ty) = self.checked_ty(target) {
            value = self.materialize_register(value, &target_ty, value_expression.source_span());
        }
        (value, value_place)
    }

    /// Like [`place`](Self::place), but returns `None` for a non-place expression
    /// (a call result, a literal, …) instead of panicking — used at a method-call
    /// receiver, which may be a temporary. Only evaluates subscript indices when
    /// the whole chain is a place.
    /// Lower a `try` sub-region (`body`/`except`/`else`/`finally`) into a
    /// self-contained mini-CFG (block ids local, entry = 0) that **shares this
    /// function's register, variable, and span space** — so it addresses the same
    /// slots. The region's own control flow (`if`/`while`/`for`) becomes local
    /// blocks; the VM runs it recursively.
    pub(super) fn lower_region(
        &mut self,
        body: &[Stmt],
        ext_loops: &[(hir::BlockId, hir::BlockId, Vec<VarId>)],
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) -> Vec<MirBlock> {
        let checked: Vec<_> = self.checked_expressions.values().cloned().collect();
        let region_cfg = hir::Cfg::build_seeded_checked_with_declarations(
            self.vars.clone(),
            body,
            ext_loops,
            &checked,
            &self.checked_declarations,
        );
        let mut region = MirFunction {
            blocks: Vec::new(),
            n_regs: 0,
            n_vars: 0,
            var_names: Vec::new(),
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: self.returns_reference,
            var_tys: HashMap::new(),
            ret_ty: self.f.ret_ty.clone(),
            raises: self.f.raises,
            error_ty: self.f.error_ty.clone(),
            spans: std::mem::take(&mut self.f.spans), // accumulate into the shared table
            reg_types: std::mem::take(&mut self.f.reg_types),
        };
        let mut map: HashMap<hir::BlockId, MirBlockId> = HashMap::new();
        for hb in region_cfg.g.node_indices() {
            map.insert(hb, region.blocks.len());
            region.blocks.push(MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Return(None),
            });
        }
        // Region-local inference can discover new slots, but it must not
        // replace exact types already established in the enclosing frame. In
        // particular, same-spelled exception targets and outer locals occupy
        // different owner slots even though name-based region seeding sees both.
        let mut region_var_types = region_cfg.var_types.clone();
        region_var_types.extend(self.var_types.clone());
        {
            let mut fl = Flatten {
                call_transfers: self.call_transfers.clone(),
                f: &mut region,
                cur: 0,
                next_reg: self.next_reg,
                vars: region_cfg.vars.clone(),
                var_types: region_var_types,
                owner_vars: self.owner_vars.clone(),
                nested: self.nested.clone(), // a `try` region may call a nested `def`
                overloads: self.overloads.clone(),
                checked_expressions: self.checked_expressions.clone(),
                checked_declarations: self.checked_declarations.clone(),
                active_semantics: Vec::new(),
                aliases: self.aliases.clone(),
                runtime_aliases: self.runtime_aliases.clone(),
                aggregate_loans: self.aggregate_loans.clone(),
                transfer_domain_loans: self.transfer_domain_loans.clone(),
                reassigned_names: self.reassigned_names.clone(),
                returns_reference: self.returns_reference,
            };
            for hb in region_cfg.g.node_indices() {
                fl.cur = map[&hb];
                for instr in &region_cfg.g[hb].instrs {
                    fl.lower_instr(instr, outer_map);
                }
                let fallback = Terminator::FallOff;
                let term = region_cfg.g[hb].term.as_ref().unwrap_or(&fallback);
                // Region terminators resolve local jumps via the region's own `map`;
                // an `EscapeJump` resolves its outer-loop target via `outer_map`.
                let mterm = fl.lower_term(term, &map, outer_map);
                fl.f.blocks[fl.cur].term = mterm;
            }
            self.next_reg = fl.next_reg;
            self.vars = fl.vars.clone();
            self.var_types = fl.var_types.clone();
            self.owner_vars = fl.owner_vars.clone();
        }
        self.f.spans = std::mem::take(&mut region.spans);
        self.f.reg_types = std::mem::take(&mut region.reg_types);
        region.blocks
    }

    /// Lower a `try`'s sub-regions and emit the [`MirInstr::Try`]. `ext_loops` are
    /// the enclosing function loops (HIR block ids) a `break`/`continue` may escape
    /// to; `outer_map` resolves them to MIR blocks. Shared by the primary
    /// (`HirInstr::Try`) and fallback (`lower_stmt`) paths.
    pub(super) fn emit_try(
        &mut self,
        regions: TryRegions<'_>,
        ext_loops: &[(hir::BlockId, hir::BlockId, Vec<VarId>)],
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        let TryRegions {
            body,
            except,
            orelse,
            finalbody,
            handler_binding,
        } = regions;
        let body_blocks = self.lower_region(body, ext_loops, outer_map);
        let handler = match except {
            Some((name, ex_body)) => {
                let slot = name.as_ref().map(|name| {
                    handler_binding
                        .map(|binding| self.declare_binding_var(binding, name))
                        .unwrap_or_else(|| self.var(name))
                });
                if let Some(slot) = slot {
                    // The checker rejects a try whose body can raise more than
                    // one error type, so copying the first propagating raising
                    // fact types the handler binding without re-inference.
                    let error =
                        region_error_type(&body_blocks, &self.f.reg_types).unwrap_or(Ty::Error);
                    self.var_types.entry(slot).or_insert(error);
                }
                let blocks = self.lower_region(ex_body, ext_loops, outer_map);
                Some((slot, blocks))
            }
            None => None,
        };
        let orelse_blocks = orelse
            .as_ref()
            .map(|b| self.lower_region(b, ext_loops, outer_map));
        let finalbody_blocks = finalbody
            .as_ref()
            .map(|b| self.lower_region(b, ext_loops, outer_map));
        self.emit(MirInstr::Try {
            body: body_blocks,
            handler,
            orelse: orelse_blocks,
            finalbody: finalbody_blocks,
            cleanup: Vec::new(),
        });
    }

    /// A place for a call argument's *write-back* — a variable or a field chain,
    /// **without** any dynamic index (so building it emits nothing and avoids
    /// re-evaluating an index that the argument's value already consumed). Returns
    /// `None` for a temporary or an indexed place (write-back to those is refused by
    /// the VM). Distinct from [`Self::try_place`], which emits index evaluations.
    pub(super) fn simple_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => Some(self.expression_place_root(name, e)),
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.simple_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })?;
                let mut p = self.resolved_place(name);
                let ty = self.checked_place_ty(e).or_else(|| self.checked_ty(e))?;
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
    pub(super) fn pure_field_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => {
                // `Self.<name>` (a reified value-parameter read, e.g. `Self.size`)
                // resolves off the receiver `self`: `Self` in expression position is
                // an alias for `self`, and the backend's field navigation also
                // searches a struct's `value_params`. `Self` never appears bare in an
                // expression (only `Self.field`), so this alias is safe.
                let root = if name == "Self" { "self" } else { name };
                Some(self.expression_place_root(root, e))
            }
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.pure_field_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            _ => None,
        }
    }

    pub(super) fn try_place(&mut self, e: &Expr) -> Option<MirPlace> {
        match &e.kind {
            ExprKind::Identifier(name) => Some(self.expression_place_root(name, e)),
            ExprKind::Member { object, field } => {
                if self.is_slice_descriptor(object) {
                    return None;
                }
                let mut p = self.try_place(object)?;
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(Proj::Field(field.clone()), ty);
                } else {
                    p.proj.push(Proj::Field(field.clone()));
                }
                Some(p)
            }
            ExprKind::Index { object, index } => {
                let mut p = self.try_place(object)?;
                // A literal index into compiler-private heterogeneous Tuple
                // storage is part of the place's static identity. Keeping it
                // out of a register lets ownership distinguish element 0 from
                // element 1 while every dynamic/nominal subscript retains the
                // ordinary single-evaluation Index(Reg) path.
                let projection = match self.checked_ty(object) {
                    Some(Ty::Tuple(_)) => exact_nonnegative_index(index)
                        .map(Proj::ConstIndex)
                        .unwrap_or_else(|| Proj::Index(self.expr(index))),
                    _ => Proj::Index(self.expr(index)),
                };
                if let Some(ty) = self.checked_place_ty(e).or_else(|| self.checked_ty(e)) {
                    p.project(projection, ty);
                } else {
                    p.proj.push(projection);
                }
                Some(p)
            }
            ExprKind::TypeApply { name, .. } => {
                let index = self
                    .checked_adjustments(e)
                    .into_iter()
                    .find_map(|adjustment| match adjustment {
                        crate::SemanticAdjustment::VariantProject { index, .. } => Some(index),
                        _ => None,
                    })?;
                let mut p = self.resolved_place(name);
                let ty = self.checked_place_ty(e).or_else(|| self.checked_ty(e))?;
                p.project(Proj::Variant(index), ty);
                Some(p)
            }
            _ => None,
        }
    }

    /// Lower the "catch-all" straight-line statements. Every reachable case is
    /// handled; the categorization decisions are documented per arm. `outer_map`
    /// threads the enclosing function's block map for a fallback-path `try`.
    pub(super) fn lower_stmt(
        &mut self,
        s: &Stmt,
        statement_binding: Option<crate::origin::OwnerId>,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        match &s.kind {
            StmtKind::RefDecl { name, value } => {
                let reference = self.var(name);
                if let Some(binding) = statement_binding {
                    self.owner_vars.insert(binding, reference);
                }
                let mutable = self.checked_borrow_mutability(value).unwrap_or(true);
                if self.reference_result(value).is_some()
                    || !matches!(
                        value.kind,
                        ExprKind::Identifier(_)
                            | ExprKind::Member { .. }
                            | ExprKind::Index { .. }
                            | ExprKind::TypeApply { .. }
                    )
                {
                    let source = self.expr(value);
                    self.runtime_aliases.insert(reference);
                    // A reference-producing expression stores a runtime handle
                    // in this local slot. Carry its checked `Ty::Ref` onto the
                    // slot immediately: later aggregate construction may need
                    // to forward the handle before any ordinary read has had a
                    // chance to seed `var_types` incidentally.
                    let binding_ty = self
                        .f
                        .reg_types
                        .get(&source.0)
                        .filter(|ty| matches!(ty, Ty::Ref(_)))
                        .cloned()
                        .or_else(|| self.reference_result(value).map(Ty::Ref))
                        .or_else(|| self.checked_ty(value));
                    if let Some(ty) = binding_ty.clone() {
                        self.var_types.insert(reference, ty);
                    }
                    self.emit(MirInstr::DefVar {
                        var: reference,
                        src: source,
                        binding_ty,
                    });
                    let candidates: Vec<&Expr> = match &value.kind {
                        ExprKind::Call { args, kwargs, .. } => args
                            .iter()
                            .chain(kwargs.iter().map(|argument| &argument.value))
                            .collect(),
                        ExprKind::MethodCall {
                            object,
                            args,
                            kwargs,
                            ..
                        } => std::iter::once(object.as_ref())
                            .chain(args.iter())
                            .chain(kwargs.iter().map(|argument| &argument.value))
                            .collect(),
                        _ => Vec::new(),
                    };
                    let candidate_places: Vec<_> = candidates
                        .into_iter()
                        .filter_map(|candidate| {
                            self.simple_place(candidate)
                                .map(|place| (self.checked_owner(candidate), place))
                        })
                        .collect();
                    let checked_places = self.checked_reference_places(value);
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
                            .map(|(_, place)| place.clone())
                            .unwrap_or_else(|| {
                                MirPlace::root(
                                    canonical.root,
                                    self.var_types.get(&canonical.root).cloned(),
                                )
                            });
                        let interior = canonical
                            .path
                            .iter()
                            .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                            .then_some(canonical);
                        loans.push(MirLoan {
                            place,
                            mutable,
                            interior,
                        });
                    }
                    if let Some(first) = loans.first() {
                        let marker =
                            self.fresh_typed(s.source_span(), Some(first.place.root), Ty::None);
                        self.aggregate_loans.insert(reference, loans.clone());
                        self.emit(MirInstr::EstablishLoans {
                            reference,
                            loans,
                            marker,
                            dest_interior: None,
                        });
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
                let checked_places = self.checked_reference_places(value);
                let mut loans = Vec::new();
                for origin in checked_places {
                    let Some(canonical) = self.mir_interior_origin(&origin, Some(place.root))
                    else {
                        continue;
                    };
                    let interior = canonical
                        .path
                        .iter()
                        .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                        .then_some(canonical);
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior,
                    });
                }
                if loans.is_empty() {
                    loans.push(MirLoan {
                        place: place.clone(),
                        mutable,
                        interior: None,
                    });
                }
                // A substituted local alias has no runtime handle value, but
                // its slot is still the checked capability through which every
                // derived place is accessed. Retain that `ref T` declaration
                // type so MIR verification can prove `place.through` instead of
                // treating the analytical alias slot as untyped storage.
                if let Some(reference_ty) = statement_binding.and_then(|binding| {
                    self.checked_declarations
                        .iter()
                        .find(|declaration| declaration.binding == Some(binding))
                        .and_then(|declaration| declaration.ty.clone())
                        .filter(|ty| matches!(ty, Ty::Ref(_)))
                }) {
                    self.var_types.insert(reference, reference_ty);
                }
                self.aliases.insert(reference, loans[0].clone());
                self.aggregate_loans.insert(reference, loans.clone());
                let marker = self.fresh_typed(s.source_span(), Some(place.root), Ty::None);
                self.emit(MirInstr::EstablishLoans {
                    reference,
                    loans,
                    marker,
                    dest_interior: None,
                });
            }
            // --- Writes through a place (any nesting) --------------------------
            StmtKind::SetPlace { place, value } => {
                if self.lower_subscript_reference_set(place, value) {
                    return;
                }
                if self.lower_subscript_set(place, value) {
                    return;
                }
                let (src, _) = self.lower_assignment_value(place, value);
                self.emit_interior_invalidations(place, None);
                // A store through an origin-bearing pointer writes its source
                // place; the checker fixed the offset to 0 and required
                // mutable provenance. A stably bound pointer substitutes the
                // owner place; otherwise the store goes through the handle.
                if let ExprKind::Index { object, .. } = &place.kind {
                    if let Some(target) = self.pointer_deref_place(object) {
                        self.emit(MirInstr::Store { place: target, src });
                        return;
                    }
                    if self.is_origin_bearing_pointer(object) {
                        let reference = self.expr(object);
                        self.emit(MirInstr::WriteRef {
                            reference,
                            value: src,
                        });
                        return;
                    }
                }
                let p = self.place(place);
                let stores_reference = matches!(p.ty, Some(Ty::Ref(_)))
                    && self.checked_adjustments(value).iter().any(|adjustment| {
                        matches!(
                            adjustment,
                            crate::SemanticAdjustment::BorrowShared
                                | crate::SemanticAdjustment::BorrowMutable
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
                    let src = if matches!(p.ty, Some(Ty::Ref(_)))
                        && self.checked_adjustments(value).iter().any(|adjustment| {
                            matches!(adjustment, crate::SemanticAdjustment::CopyPlaceValue)
                        }) {
                        let copied = self.fresh(span(place), None);
                        self.emit(MirInstr::CopyValue {
                            dest: copied,
                            value: src,
                        });
                        copied
                    } else {
                        src
                    };
                    self.emit(MirInstr::Store { place: p, src });
                }
            }
            StmtKind::AugAssign { place, op, value } => {
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
                if let ExprKind::Identifier(name) = &place.kind {
                    // Opaque structured statements retain the source spelling,
                    // while HIR may give a same-spelled sibling declaration a
                    // distinct runtime slot. Resolve the checked owner for the
                    // write half just as `self.expr(place)` does for the read.
                    let var = self.expression_var(name, place);
                    let cur = self.expr(place);
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    if self.runtime_aliases.contains(&var) {
                        let handle = self.fresh(place.source_span(), Some(var));
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
                        });
                    }
                } else if let ExprKind::Index { object, .. } = &place.kind
                    && let Some(target) = self.pointer_deref_place(object)
                {
                    // `p[0] OP= e` through a stably bound pointer: owner-place
                    // load and store, exactly like an alias write-back.
                    let cur = self.fresh(span(place), Some(target.root));
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: target.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store {
                        place: target,
                        src: res,
                    });
                } else if let ExprKind::Index { object, .. } = &place.kind
                    && self.is_origin_bearing_pointer(object)
                {
                    // `p[0] OP= e` through an origin-bearing pointer: read and
                    // write through the handle, evaluated once.
                    let reference = self.expr(object);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else if matches!(self.checked_place_ty(place), Some(Ty::Ref(_))) {
                    // A projected ref-valued slot (for example an element of
                    // `List[ref T]`) is two distinct places: the container slot
                    // stores the handle, while augmented assignment reads and
                    // writes the referent. Preserve the handle explicitly so a
                    // nominal container's index dunder cannot feed `ref` itself
                    // into the arithmetic operation.
                    let reference = self.reference_handle(place);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::ReadRef {
                        dest: cur,
                        reference,
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::WriteRef {
                        reference,
                        value: res,
                    });
                } else {
                    let p = self.place(place);
                    let cur = self.fresh(span(place), None);
                    self.emit(MirInstr::LoadPlace {
                        dest: cur,
                        place: p.clone(),
                    });
                    let rhs = self.expr(value);
                    let res = self.fresh(span(place), None);
                    self.emit(MirInstr::BinOp {
                        op: *op,
                        dest: res,
                        a: cur,
                        b: rhs,
                        resolved: None,
                    });
                    self.emit_interior_invalidations(place, None);
                    self.emit(MirInstr::Store { place: p, src: res });
                }
            }

            // --- Simple effectful statements -----------------------------------
            StmtKind::Raise(e) => {
                let src = self.expr(e);
                self.emit(MirInstr::Raise { src });
            }
            // `comptime N = e` is an ordinary `Int` binding at runtime.
            StmtKind::Comptime { name, value, .. } => {
                let src = self.expr(value);
                let var = statement_binding
                    .map(|binding| self.declare_binding_var(binding, name))
                    .unwrap_or_else(|| self.var(name));
                // A comptime statement remains a fallback HIR statement rather
                // than `HirInstr::Bind`; copy its checked expression type when
                // present, or the already-typed initializer register for
                // synthetic/compatibility paths. This makes closure capture
                // places typed without relying on an unrelated later use.
                let binding_ty = self
                    .checked_ty(value)
                    .or_else(|| self.f.reg_types.get(&src.0).cloned());
                if let Some(ty) = binding_ty.clone() {
                    self.var_types.insert(var, ty);
                }
                self.emit(MirInstr::DefVar {
                    var,
                    src,
                    binding_ty,
                });
            }
            // `pass` has no runtime effect. Imports were consumed by linking and
            // are no-ops in a lowered module body.
            StmtKind::Pass | StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}

            // `try`/`except`/`else`/`finally` — each part lowers to a mini-CFG that
            // shares this function's slots; the VM runs them with `exec_try`
            // semantics. `cleanup` (the exceptional-edge drops) is filled by the
            // drop-elaboration pass.
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                // A `break`/`continue` that leaves the `try` (targeting an enclosing
                // loop) needs the outer loop's target block, which the self-contained
                // mini-CFG region can't name — refuse cleanly rather than build an
                // ill-formed region. (A `return` crossing out is fine: it surfaces as
                // a `Flow::Return` the block driver handles.)
                let crosses = region_crosses_control(body)
                    || except
                        .as_ref()
                        .is_some_and(|(_, b)| region_crosses_control(b))
                    || orelse.as_ref().is_some_and(|b| region_crosses_control(b))
                    || finalbody
                        .as_ref()
                        .is_some_and(|b| region_crosses_control(b));
                if crosses {
                    self.emit(MirInstr::Unsupported(
                        "try with break/continue crossing the try boundary".into(),
                    ));
                    return;
                }
                // Fallback path (a `try` whose enclosing loops are region-local, so
                // the HIR left it as an opaque `Stmt`): no escapable loops.
                self.emit_try(
                    TryRegions {
                        body,
                        except,
                        orelse,
                        finalbody,
                        handler_binding: statement_binding,
                    },
                    &[],
                    outer_map,
                );
            }
            // A direct nested declaration creates its closure exactly here. Copy
            // and move captures therefore snapshot/transfer before any following
            // statement can mutate or use the source binding. Later calls and
            // first-class uses load this internal closure slot.
            StmtKind::Def { name, .. } => {
                let info = statement_binding
                    .and_then(|binding| self.nested.get(&binding))
                    .filter(|info| info.materialized_here)
                    .cloned();
                if let Some(info) = info {
                    let src = self.emit_nested_closure(&info, s.source_span(), false);
                    let var = self.declare_binding_var(info.binding, name);
                    if let Some(ty) = &info.callable_ty {
                        self.var_types.entry(var).or_insert_with(|| ty.clone());
                    }
                    self.emit(MirInstr::DefVar {
                        var,
                        src,
                        binding_ty: info.callable_ty,
                    });
                } else {
                    self.emit(MirInstr::Unsupported(
                        "nested def/struct/trait declaration".into(),
                    ));
                }
            }
            // A nested `def` we couldn't lift because it nests another declaration,
            // or a nested `struct`/`trait`, stays a clean `Unsupported`.
            StmtKind::Struct { .. } | StmtKind::Trait { .. } => self.emit(MirInstr::Unsupported(
                "nested def/struct/trait declaration".into(),
            )),

            // Tuple unpacking `a, b = t`: evaluate the tuple once, then bind each
            // target from its element (a NAME → `DefVar`; a place → `Store`).
            StmtKind::Unpack { targets, value, .. } => {
                let plan = self
                    .tuple_unpack_plan(value)
                    .expect("checked tuple unpack carries an extraction plan");
                assert_eq!(
                    plan.len(),
                    targets.len(),
                    "checked tuple unpack arity matches its targets"
                );
                let base_place = self.simple_place(value);
                let tuple = self.expr(value);
                for (i, (target, extraction)) in targets.iter().zip(plan).enumerate() {
                    let idx = self.fresh_typed(span(target), None, Ty::Int);
                    self.emit(MirInstr::Const {
                        dest: idx,
                        k: Const::Int(i as i64),
                    });
                    let raw_ty = extraction
                        .reference
                        .clone()
                        .map(Ty::Ref)
                        .unwrap_or_else(|| extraction.ty.clone());
                    let raw = self.fresh_typed(span(target), None, raw_ty.clone());
                    let call = extraction.accessor.clone().map(|target| MirSubscriptCall {
                        target,
                        raises: None,
                        result_ty: raw_ty.clone(),
                        receiver_requires_place: extraction.reference.is_some(),
                        receiver_convention: extraction
                            .reference
                            .as_ref()
                            .map(|_| crate::ast::ArgConvention::Ref),
                        arguments: Vec::new(),
                        capture_accesses: Vec::new(),
                        reference_result: extraction.reference.clone(),
                        param_arg_regs: Vec::new(),
                        param_decls: Vec::new(),
                    });
                    let intrinsic = call
                        .is_none()
                        .then(|| self.intrinsic_index_dispatch(value))
                        .flatten();
                    self.emit(MirInstr::Index {
                        dest: raw,
                        base: tuple,
                        index: idx,
                        base_place: base_place.clone(),
                        index_place: None,
                        call,
                        intrinsic,
                    });
                    let elem = if extraction.reference.is_some() {
                        let value = self.fresh_typed(
                            span(target),
                            base_place.as_ref().map(|place| place.root),
                            extraction.ty,
                        );
                        self.emit(MirInstr::ReadRef {
                            dest: value,
                            reference: raw,
                        });
                        value
                    } else {
                        raw
                    };
                    match &target.kind {
                        ExprKind::Identifier(name) => {
                            let var = self.expression_var(name, target);
                            self.emit_interior_invalidations(target, Some(var));
                            let binding_ty = self
                                .checked_place_ty(target)
                                .or_else(|| self.checked_ty(target));
                            if let Some(ty) = binding_ty.clone() {
                                self.var_types.insert(var, ty);
                            }
                            self.emit(MirInstr::DefVar {
                                var,
                                src: elem,
                                binding_ty,
                            });
                        }
                        _ => {
                            let place = self.place(target);
                            self.emit_interior_invalidations(target, Some(place.root));
                            self.emit(MirInstr::Store { place, src: elem });
                        }
                    }
                }
            }

            // --- Unreachable after the checker ---------------------------------
            // Parse-only statements are flagged `Unsupported`, so a checked program
            // never reaches MIR with them.
            StmtKind::With { .. } | StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. } => {
                self.emit(MirInstr::Unsupported(format!(
                    "unchecked statement reached MIR lowering: {:?}",
                    s.kind
                )));
            }
            // These are lowered by `hir::Lower` directly (to instrs/terminators), so
            // they never arrive here wrapped in a `HirInstr::Stmt`.
            StmtKind::If { .. }
            | StmtKind::While { .. }
            | StmtKind::For { .. }
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Return(_)
            | StmtKind::VarDecl { .. }
            | StmtKind::Assign { .. }
            | StmtKind::Expr(_) => {
                self.emit(MirInstr::Unsupported(format!(
                    "malformed HIR statement instruction: {:?}",
                    s.kind
                )));
            }
        }
    }

    pub(super) fn lower_hir_stmt(
        &mut self,
        statement: &crate::hir::HirStmt,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) {
        let mut index = HashMap::new();
        for (syntax, expression) in statement_expression_roots(&statement.syntax)
            .into_iter()
            .zip(&statement.expressions)
        {
            index_hir_expression(syntax, expression, &mut index);
        }
        self.active_semantics.push(index);
        self.lower_stmt(&statement.syntax, statement.binding, outer_map);
        self.active_semantics.pop();
    }

    pub(super) fn lower_return_value(&mut self, expression: &hir::HirExpr) -> Reg {
        if self.returns_reference {
            // Returning an ordinary place borrows that place. Returning a place
            // whose *storage* is already `ref T`, or forwarding another
            // reference-producing expression, instead returns the existing
            // handle. Borrowing the ref-valued slot would manufacture `ref ref T`
            // at runtime.
            let forwards_handle = matches!(
                expression.place.as_ref().map(|place| &place.ty),
                Some(Ty::Ref(_))
            ) || expression.adjustments.iter().any(|adjustment| {
                matches!(
                    adjustment,
                    crate::SemanticAdjustment::ReferenceResult { .. }
                )
            });
            if forwards_handle {
                self.reference_handle_hir(expression)
            } else {
                let place = self
                    .projected_reference_place_hir(expression)
                    .unwrap_or_else(|| self.place_hir(expression));
                let dest = self.fresh(expression.source_span(), Some(place.root));
                self.emit(MirInstr::MakeRef { dest, place });
                dest
            }
        } else {
            let value = self.expr_hir(expression);
            match self.f.ret_ty.clone() {
                Some(target) => self.materialize_register(value, &target, expression.source_span()),
                None => value,
            }
        }
    }

    /// Lower a HIR block terminator; the branch/return operands are flattened into
    /// `self.cur` first, then the `MirTerm` references their result registers.
    pub(super) fn lower_term(
        &mut self,
        t: &Terminator,
        map: &HashMap<hir::BlockId, MirBlockId>,
        outer_map: &HashMap<hir::BlockId, MirBlockId>,
    ) -> MirTerm {
        match t {
            Terminator::Jump(b) => MirTerm::Jump(map[b]),
            Terminator::Branch {
                cond,
                then_b,
                else_b,
            } => {
                let c = self.expr_hir(cond); // evaluated at the end of this block
                MirTerm::Branch {
                    cond: c,
                    then_b: map[then_b],
                    else_b: map[else_b],
                }
            }
            Terminator::Return(expression) => {
                MirTerm::Return(expression.as_ref().map(|e| self.lower_return_value(e)))
            }
            Terminator::ReturnWithCleanup { value, cleanup } => {
                // Preserve source evaluation order: materialize the return value
                // before destroying loop-owned iterators. In particular, `return
                // item^` must transfer the yielded element before the iterator's
                // residual storage is released.
                let value = value.as_ref().map(|e| self.lower_return_value(e));
                for var in cleanup {
                    // Keep the cleanup root live into this return block. Without
                    // this marker, edge-based last-use elaboration can destroy a
                    // loop iterator at block entry (before the return value and
                    // the current yielded element have been handled).
                    self.emit(MirInstr::KeepAlive { var: *var });
                }
                MirTerm::ReturnWithCleanup {
                    value,
                    cleanup: cleanup.clone(),
                }
            }
            Terminator::FallOff => MirTerm::FallOff,
            // An outward `break`/`continue`: the target is an enclosing-function
            // block, resolved via `outer_map` (`cleanup` filled by drop elaboration).
            Terminator::EscapeJump(b) => MirTerm::EscapeJump {
                target: outer_map[b],
                cleanup: Vec::new(),
            },
        }
    }
}
