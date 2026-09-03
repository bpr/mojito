//! `Try` region lowering: finally dispatch, exit crossings, staged
//! returns, and scope-exit cleanups.

use super::*;

impl<'a> FnLowering<'a> {
    /// A structurally lowered `try`: flatten the region mini-CFGs into this
    /// function's flat block list with explicit edges. The body lowers with a
    /// fresh landing block as its raise-edge target; every body exit (normal
    /// completion, raise, return, escape) runs the flag-guarded `cleanup`
    /// drops exactly like the VM's `exec_try`; the handler binds the staged
    /// error; `orelse` runs only on normal completion.
    pub(super) fn lower_try(
        &mut self,
        ctx: &mut Context,
        body: &[MirBlock],
        handler: Option<&(Option<u32>, Vec<MirBlock>)>,
        orelse: Option<&[MirBlock]>,
        finalbody: Option<&[MirBlock]>,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let after = BasicBlock::new(ctx, None, vec![]);
        after.insert_at_back(region, ctx);
        let landing = BasicBlock::new(ctx, None, vec![]);
        landing.insert_at_back(region, ctx);
        let normal_exit = BasicBlock::new(ctx, None, vec![]);
        normal_exit.insert_at_back(region, ctx);

        // A `finally`-bearing try gets its pending-outcome machinery up
        // front: kind/error staging, the forwarding entry every pending edge
        // jumps to, the post-finalbody dispatch, a normal-entry (kind 0)
        // block, and the error-entry (kind 1) block raises route through.
        let finally_idx = match finalbody {
            Some(_) => {
                let entry = BasicBlock::new(ctx, None, vec![]);
                entry.insert_at_back(region, ctx);
                let dispatch = BasicBlock::new(ctx, None, vec![]);
                dispatch.insert_at_back(region, ctx);
                let error_entry = BasicBlock::new(ctx, None, vec![]);
                error_entry.insert_at_back(region, ctx);
                let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
                let kind_slot = self.entry_typed_alloca(ctx, i32_handle);
                let pending_err = self.entry_alloca(ctx, 24, 8);
                self.finally_states.push(FinallyState {
                    entry,
                    dispatch,
                    error_entry,
                    kind_slot,
                    pending_err,
                    codes: Vec::new(),
                    error_possible: false,
                    after,
                });
                Some(self.finally_states.len() - 1)
            }
            None => None,
        };

        // Body: raises inside it land on this try's landing block.
        self.try_frames.push(TryFrame {
            landing,
            cleanup: cleanup.to_vec(),
            finally: finally_idx,
            pends_error: handler.is_none() && finally_idx.is_some(),
        });
        let body_entry = self.lower_region(ctx, body, normal_exit)?;
        self.try_frames.pop();
        // Enter the try from the enclosing block.
        let enter = BrOp::new(ctx, body_entry, vec![]);
        self.append(ctx, enter.get_operation(), None);

        // Handler and `else` regions lower under a pseudo-frame when a
        // `finally` exists: their raises stage the pending error and their
        // returns/escapes still pend on this finalbody, but the body-edge
        // cleanup does not run twice.
        let pseudo_frame = finally_idx.map(|idx| TryFrame {
            landing: self.finally_states[idx].error_entry,
            cleanup: Vec::new(),
            finally: Some(idx),
            pends_error: true,
        });
        // Where a completed handler/orelse continues: into the finalbody
        // with a normal pending outcome, or straight to the continuation.
        let normal_entry = match finally_idx {
            Some(idx) => {
                let block = BasicBlock::new(ctx, None, vec![]);
                block.insert_at_back(region, ctx);
                let saved = self.current;
                self.current = Some(block);
                let tag = self.tag_constant(ctx, 0);
                let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
                self.append(ctx, store.get_operation(), None);
                let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
                block
            }
            None => after,
        };

        // Raise edge: cleanup drops, then the handler (binding the staged
        // error) or propagation. The handler region lowers before `orelse` —
        // position-space region ids must follow the body → handler → orelse
        // → finalbody order `record_last_uses` used.
        self.current = Some(landing);
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        match handler {
            Some((error_var, handler_blocks)) => {
                if self.trace_lifecycle {
                    self.emit_trace_err_slot(ctx, mojito_native::native::rt_abi::TRACE_CATCH);
                }
                let err_slot = self.ensure_err_slot(ctx);
                match error_var {
                    Some(var) => {
                        // A still-initialized previous binding (a loop
                        // rebinding the same handler var) frees first — the
                        // VM abandons the overwritten value to its arena.
                        if let Some(&flag) = self.drop_flags.get(var) {
                            let cont = self.begin_flag_guard(ctx, flag);
                            let slot = self.var_slots[*var as usize];
                            self.emit_release_storage(ctx, slot, &Ty::Error)?;
                            self.end_flag_guard(ctx, cont);
                        }
                        // The staged error moves into the bound slot; its
                        // ordinary drop frees the message.
                        let slot = self.var_slots[*var as usize];
                        self.mem_copy(ctx, slot, err_slot, 24, Reg(u32::MAX));
                        self.set_drop_flag(ctx, *var, true);
                    }
                    None => {
                        // No binder: the caught error is dropped on entry.
                        self.emit_release_storage(ctx, err_slot, &Ty::Error)?;
                    }
                }
                if let Some(frame) = pseudo_frame.as_ref() {
                    self.try_frames.push(TryFrame {
                        landing: frame.landing,
                        cleanup: Vec::new(),
                        finally: frame.finally,
                        pends_error: true,
                    });
                }
                let handler_entry = self.lower_region(ctx, handler_blocks, normal_entry)?;
                if pseudo_frame.is_some() {
                    self.try_frames.pop();
                }
                let jump = BrOp::new(ctx, handler_entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
            }
            None => match finally_idx {
                // No handler: the raise pends on the finalbody, or
                // propagates to the enclosing observer.
                Some(idx) => {
                    let jump = BrOp::new(ctx, self.finally_states[idx].error_entry, vec![]);
                    self.append(ctx, jump.get_operation(), None);
                }
                None => {
                    let target = self.raise_edge_target(ctx)?;
                    let jump = BrOp::new(ctx, target, vec![]);
                    self.append(ctx, jump.get_operation(), None);
                }
            },
        }

        // Normal completion: cleanup drops, then `orelse` (only here), then
        // the finalbody or the continuation.
        self.current = Some(normal_exit);
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        let normal_target = match orelse {
            Some(orelse_blocks) => {
                if let Some(frame) = pseudo_frame.as_ref() {
                    self.try_frames.push(TryFrame {
                        landing: frame.landing,
                        cleanup: Vec::new(),
                        finally: frame.finally,
                        pends_error: true,
                    });
                }
                let entry = self.lower_region(ctx, orelse_blocks, normal_entry)?;
                if pseudo_frame.is_some() {
                    self.try_frames.pop();
                }
                entry
            }
            None => normal_entry,
        };
        let jump = BrOp::new(ctx, normal_target, vec![]);
        self.append(ctx, jump.get_operation(), None);

        // The finalbody itself, then the pending-outcome dispatch.
        if let (Some(final_blocks), Some(idx)) = (finalbody, finally_idx) {
            // Error entry: stage the raise as this try's pending outcome.
            let saved = self.current;
            self.current = Some(self.finally_states[idx].error_entry);
            let err_slot = self.ensure_err_slot(ctx);
            let pending_err = self.finally_states[idx].pending_err;
            self.mem_copy(ctx, pending_err, err_slot, 24, Reg(u32::MAX));
            let tag = self.tag_constant(ctx, 1);
            let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
            self.append(ctx, store.get_operation(), None);
            let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
            self.append(ctx, jump.get_operation(), None);
            self.current = saved;

            // The finalbody lowers once, outside this try's raise protection
            // (its own raise/return/escape overrides the pending outcome and
            // resolves it on the way out).
            self.finally_overrides.push(idx);
            let fin_entry = self.lower_region(ctx, final_blocks, self.finally_states[idx].dispatch);
            self.finally_overrides.pop();
            let fin_entry = fin_entry?;
            let saved = self.current;
            self.current = Some(self.finally_states[idx].entry);
            let jump = BrOp::new(ctx, fin_entry, vec![]);
            self.append(ctx, jump.get_operation(), None);

            // Dispatch: forward the pending outcome now that the finalbody
            // completed normally.
            self.current = Some(self.finally_states[idx].dispatch);
            self.emit_finally_dispatch(ctx, idx)?;
            self.current = saved;
        }

        self.current = Some(after);
        Ok(())
    }

    /// The post-finalbody dispatch of `finally_states[idx]`: switch on the
    /// pending kind — normal continues to the try's continuation, a pending
    /// error re-raises toward the enclosing observer, and a pending exit
    /// site crosses outward (running enclosing cleanups, pending on the next
    /// finalbody, or reaching its terminal).
    pub(super) fn emit_finally_dispatch(
        &mut self,
        ctx: &mut Context,
        idx: usize,
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let kind_slot = self.finally_states[idx].kind_slot;
        let after = self.finally_states[idx].after;
        let pending_err = self.finally_states[idx].pending_err;
        let codes: Vec<u32> = self.finally_states[idx].codes.clone();
        let error_possible = self.finally_states[idx].error_possible;
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let kind = LoadOp::new(ctx, kind_slot, i32_handle);
        self.append(ctx, kind.get_operation(), None);
        let kind = kind.get_result(ctx);

        // Pending error case (kind 1): restage and re-raise outward.
        let mut next = self.current.expect("dispatch emission is inside a block");
        if error_possible {
            let error_case = BasicBlock::new(ctx, None, vec![]);
            error_case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(error_case);
                let err_slot = self.ensure_err_slot(ctx);
                self.mem_copy(ctx, err_slot, pending_err, 24, Reg(u32::MAX));
                let target = self.raise_edge_target(ctx)?;
                let jump = BrOp::new(ctx, target, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let one = self.tag_constant(ctx, 1);
            let is_error = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, one);
            self.append(ctx, is_error.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(
                ctx,
                is_error.get_result(ctx),
                error_case,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        // One case per pending exit-site code.
        for code in codes {
            self.current = Some(next);
            let case = BasicBlock::new(ctx, None, vec![]);
            case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(case);
                self.emit_exit_crossing(ctx, code)?;
                self.current = saved;
            }
            let expected = self.tag_constant(ctx, code);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, expected);
            self.append(ctx, matches.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(ctx, matches.get_result(ctx), case, vec![], rest, vec![]);
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        // Everything else is the normal completion.
        self.current = Some(next);
        let jump = BrOp::new(ctx, after, vec![]);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// Route exit-site `code` outward from the current frame context: run
    /// each enclosing frame's cleanup drops inner to outer; the first
    /// `finally`-bearing frame records the code as pending and enters its
    /// finalbody; with none left, the site's terminal runs.
    pub(super) fn emit_exit_crossing(
        &mut self,
        ctx: &mut Context,
        code: u32,
    ) -> Result<(), PlironError> {
        let frames: Vec<(Vec<u32>, Option<usize>)> = self
            .try_frames
            .iter()
            .rev()
            .map(|frame| (frame.cleanup.clone(), frame.finally))
            .collect();
        for (cleanup, finally) in frames {
            for var in cleanup {
                self.lower_drop_var(ctx, var)?;
            }
            if let Some(idx) = finally {
                if !self.finally_states[idx].codes.contains(&code) {
                    self.finally_states[idx].codes.push(code);
                }
                let tag = self.tag_constant(ctx, code);
                let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
                self.append(ctx, store.get_operation(), None);
                let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
                return Ok(());
            }
        }
        let terminal = self.site_terminal(ctx, code)?;
        let jump = BrOp::new(ctx, terminal, vec![]);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// The terminal block of exit site `code - 2`: a return runs the site's
    /// carried cleanup roots, resolves any pending outcomes the site
    /// overrode, and returns the staged value; an escape jumps to its
    /// function-level target.
    pub(super) fn site_terminal(
        &mut self,
        ctx: &mut Context,
        code: u32,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        let site = (code - 2) as usize;
        if let Some(block) = self.exit_sites[site].terminal {
            return Ok(block);
        }
        let region = self.region.expect("lowering is inside a function");
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        self.exit_sites[site].terminal = Some(block);
        let saved = self.current;
        self.current = Some(block);
        match &self.exit_sites[site].action {
            ExitAction::Return { cleanup } => {
                let cleanup = cleanup.clone();
                let overrides = self.exit_sites[site].overrides.clone();
                for var in cleanup {
                    self.lower_drop_var(ctx, var)?;
                }
                // The VM merges an overridden return's cleanup roots after
                // the overriding return's own (distinct-union; flags make
                // re-listed roots no-ops), innermost override first.
                for idx in overrides.into_iter().rev() {
                    self.emit_pending_resolution(ctx, idx)?;
                }
                self.emit_staged_return(ctx)?;
            }
            ExitAction::Escape { target } => {
                let target = *target;
                let Some(&target_block) = self.function_blocks.get(target) else {
                    return Err(
                        self.unsupported(format!("escape to missing block bb{target}"), None)
                    );
                };
                let jump = BrOp::new(ctx, target_block, vec![]);
                self.append(ctx, jump.get_operation(), None);
            }
        }
        self.current = saved;
        Ok(block)
    }

    /// Resolve the pending outcome of `finally_states[idx]` after an
    /// override: a pending return's carried roots still leave scope, a
    /// pending error's message frees (no user destructor — the VM's
    /// discarded error is arena-reclaimed), a pending normal is nothing.
    pub(super) fn emit_pending_resolution(
        &mut self,
        ctx: &mut Context,
        idx: usize,
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let kind_slot = self.finally_states[idx].kind_slot;
        let pending_err = self.finally_states[idx].pending_err;
        let codes: Vec<u32> = self.finally_states[idx].codes.clone();
        let error_possible = self.finally_states[idx].error_possible;
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let kind = LoadOp::new(ctx, kind_slot, i32_handle);
        self.append(ctx, kind.get_operation(), None);
        let kind = kind.get_result(ctx);
        let join = BasicBlock::new(ctx, None, vec![]);
        join.insert_at_back(region, ctx);

        // kind 1: free the discarded pending error's message.
        let mut next = self.current.expect("resolution emission is inside a block");
        if error_possible {
            let error_case = BasicBlock::new(ctx, None, vec![]);
            error_case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(error_case);
                self.emit_release_storage(ctx, pending_err, &Ty::Error)?;
                let jump = BrOp::new(ctx, join, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let one = self.tag_constant(ctx, 1);
            let is_error = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, one);
            self.append(ctx, is_error.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(
                ctx,
                is_error.get_result(ctx),
                error_case,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        for code in codes {
            // Only pending returns carry roots to resolve; a pending escape
            // resolved its overrides at its own site.
            let site = (code - 2) as usize;
            let ExitAction::Return { cleanup } = &self.exit_sites[site].action else {
                continue;
            };
            let cleanup = cleanup.clone();
            let inner_overrides = self.exit_sites[site].overrides.clone();
            self.current = Some(next);
            let case = BasicBlock::new(ctx, None, vec![]);
            case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(case);
                for var in cleanup {
                    self.lower_drop_var(ctx, var)?;
                }
                for inner in inner_overrides.into_iter().rev() {
                    self.emit_pending_resolution(ctx, inner)?;
                }
                let jump = BrOp::new(ctx, join, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let expected = self.tag_constant(ctx, code);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, expected);
            self.append(ctx, matches.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(ctx, matches.get_result(ctx), case, vec![], rest, vec![]);
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }
        self.current = Some(next);
        let jump = BrOp::new(ctx, join, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join);
        Ok(())
    }

    /// Stage a return's value at its site, before any finalbody runs: the ok
    /// payload of a raising function writes into the outcome, an aggregate
    /// writes through the sret pointer, a scalar parks in the per-function
    /// staging slot.
    pub(super) fn stage_return_value(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
    ) -> Result<(), PlironError> {
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            let outcome_ptr = self
                .outcome_ptr
                .expect("raising functions receive an outcome pointer");
            match (&outcome.ok, value) {
                (LowerTy::ZeroSized, _) | (_, None) => {}
                (LowerTy::Scalar(expected), Some(reg)) => {
                    let staged = self.reg_value(ctx, reg, *expected)?;
                    let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                    let store = StoreOp::new(ctx, staged, address);
                    self.append(ctx, store.get_operation(), Some(reg));
                }
                (LowerTy::Aggregate { layout, .. }, Some(reg)) => {
                    let size = layout.size;
                    let ptr = self.reg_ptr(ctx, reg)?;
                    let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                    self.mem_copy(ctx, address, ptr, size, reg);
                    self.owned_temps.remove(&reg.0);
                }
            }
            return Ok(());
        }
        let ret_lower = self.return_value_lower()?;
        match (ret_lower, value) {
            (Some(LowerTy::Aggregate { layout, .. }), Some(reg)) => {
                let sret = self
                    .sret_ptr
                    .expect("aggregate-returning functions receive an sret pointer");
                let ptr = self.reg_ptr(ctx, reg)?;
                self.mem_copy(ctx, sret, ptr, layout.size, reg);
                self.owned_temps.remove(&reg.0);
            }
            (Some(LowerTy::Scalar(expected)), Some(reg)) => {
                let staged = self.reg_value(ctx, reg, expected)?;
                let slot = match self.pending_ret {
                    Some(slot) => slot,
                    None => {
                        let handle = expected.handle(ctx);
                        let slot = self.entry_typed_alloca(ctx, handle);
                        self.pending_ret = Some(slot);
                        slot
                    }
                };
                let store = StoreOp::new(ctx, staged, slot);
                self.append(ctx, store.get_operation(), Some(reg));
            }
            _ => {}
        }
        Ok(())
    }

    /// The function-exit half of a staged return: read the staged value per
    /// the return ABI and return.
    pub(super) fn emit_staged_return(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        self.emit_frame_exit_error_releases(ctx)?;
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            let outcome_ptr = self
                .outcome_ptr
                .expect("raising functions receive an outcome pointer");
            let _ = outcome;
            let tag = self.tag_constant(ctx, mojito_native::native::rt_abi::MJ_TAG_OK);
            let store = StoreOp::new(ctx, tag, outcome_ptr);
            self.append(ctx, store.get_operation(), None);
            let ret = ReturnOp::new(ctx, None);
            self.append(ctx, ret.get_operation(), None);
            return Ok(());
        }
        let ret_lower = self.return_value_lower()?;
        let value = match ret_lower {
            Some(LowerTy::Scalar(scalar)) => {
                let slot = self
                    .pending_ret
                    .expect("scalar returns crossing a finalbody stage their value");
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, slot, handle);
                self.append(ctx, load.get_operation(), None);
                Some(load.get_result(ctx))
            }
            _ => None,
        };
        let ret = ReturnOp::new(ctx, value);
        self.append(ctx, ret.get_operation(), None);
        Ok(())
    }

    /// Lower one `try` sub-region mini-CFG into fresh pliron blocks (the
    /// region's local block ids swap in as `self.blocks`; its position-space
    /// ids continue `next_region_block` exactly as `record_last_uses`
    /// assigned them). `FallOff` jumps to `falloff`. Returns the region's
    /// entry block; `self.current` is restored.
    pub(super) fn lower_region(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        falloff: Ptr<BasicBlock>,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let ids_start = self.next_region_block;
        self.next_region_block += blocks.len();
        let mut locals = Vec::with_capacity(blocks.len());
        for _ in blocks {
            let block = BasicBlock::new(ctx, None, vec![]);
            block.insert_at_back(region, ctx);
            locals.push(block);
        }
        let entry = locals[0];
        let saved_blocks = std::mem::replace(&mut self.blocks, locals);
        let saved_falloff = self.falloff_target.replace(falloff);
        let saved_current = self.current;
        let saved_position = self.position;
        let mut result = Ok(());
        'blocks: for (i, block) in blocks.iter().enumerate() {
            self.current = Some(self.blocks[i]);
            for (index, instr) in block.instrs.iter().enumerate() {
                self.position = (ids_start + i, index);
                if let Err(error) = self
                    .lower_instr(ctx, instr)
                    .and_then(|()| self.flush_owned_temps(ctx))
                {
                    result = Err(error);
                    break 'blocks;
                }
            }
            self.position = (ids_start + i, usize::MAX);
            if let Err(error) = self.lower_term(ctx, &block.term) {
                result = Err(error);
                break 'blocks;
            }
        }
        self.blocks = saved_blocks;
        self.falloff_target = saved_falloff;
        self.current = saved_current;
        self.position = saved_position;
        result.map(|()| entry)
    }

    /// Drops crossing a return or escape edge: each enclosing `try`'s
    /// cleanup list (inner to outer — the VM runs `Try.cleanup` whenever a
    /// body region is left), then the edge's own carried cleanup roots. All
    /// drops are flag-guarded, so listings that already died are no-ops.
    pub(super) fn emit_scope_exit_cleanups(
        &mut self,
        ctx: &mut Context,
        edge_cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let frames: Vec<Vec<u32>> = self
            .try_frames
            .iter()
            .rev()
            .map(|frame| frame.cleanup.clone())
            .collect();
        for cleanup in frames {
            for var in cleanup {
                self.lower_drop_var(ctx, var)?;
            }
        }
        for &var in edge_cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        Ok(())
    }

    /// The `(data, len)` byte pair of a string-carrying register: a
    /// compile-time literal, a runtime string (an `Error` message included),
    /// or a nominal String value.
    pub(super) fn string_bytes(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        if let Some(bytes) = self.str_consts.get(&reg.0).cloned() {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&reg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&reg.0) {
            Some(Ty::Struct(name, _)) if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            // An error value displays as its bare message (the VM's
            // `format_value` over `Value::Error`).
            Some(Ty::Error) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            _ => Err(self.unsupported_reg(format!("string value in register %r{}", reg.0), dest)),
        }
    }

    /// The compiled `__init__` a constructor call executes: the exact name,
    /// else the unique overload taking `argc + 1` parameters (counting
    /// `out self`) — the VM's `overload_name` policy over the compiled set.
    pub(super) fn constructor_init(&self, name: &str, argc: usize) -> Option<String> {
        let init = format!("{name}.__init__");
        if self.signatures.contains_key(&init) {
            return Some(init);
        }
        let mut matches = self.signatures.iter().filter(|(fname, signature)| {
            mojito_symbol::symbol::is_overload_of(fname, &init)
                && signature.params.len() == argc + 1
        });
        let first = matches.next()?.0.clone();
        matches.next().is_none().then_some(first)
    }
}
