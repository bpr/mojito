//! Terminator lowering: branches, return edges, escapes, and
//! top-level binding releases.

use super::*;

impl<'a> FnLowering<'a> {
    pub(super) fn lower_term(
        &mut self,
        ctx: &mut Context,
        term: &MirTerm,
    ) -> Result<(), PlironError> {
        match term {
            MirTerm::Jump(target) => {
                let jump = BrOp::new(ctx, self.block(*target)?, vec![]);
                self.append(ctx, jump.get_operation(), None);
                Ok(())
            }
            MirTerm::Branch {
                cond,
                then_b,
                else_b,
            } => {
                let condition = self.reg_value(ctx, *cond, ScalarTy::Bool)?;
                let then_block = self.block(*then_b)?;
                let else_block = self.block(*else_b)?;
                let branch = CondBrOp::new(ctx, condition, then_block, vec![], else_block, vec![]);
                self.append(ctx, branch.get_operation(), Some(*cond));
                Ok(())
            }
            MirTerm::Return(value) => self.lower_return_edge(ctx, value.as_ref().copied(), &[]),
            MirTerm::ReturnWithCleanup { value, cleanup } => {
                self.lower_return_edge(ctx, value.as_ref().copied(), cleanup)
            }
            MirTerm::FallOff => {
                let Some(target) = self.falloff_target else {
                    return Err(self.unsupported("`FallOff` outside a try region".into(), None));
                };
                let jump = BrOp::new(ctx, target, vec![]);
                self.append(ctx, jump.get_operation(), None);
                Ok(())
            }
            MirTerm::EscapeJump { target, cleanup } => {
                self.lower_escape_edge(ctx, *target, cleanup)
            }
        }
    }

    /// A return terminator: with no finalbody in the way, enclosing
    /// `Try.cleanup` lists run inner to outer, then the return's own carried
    /// roots, then the value returns. Crossing a finalbody (or overriding
    /// one's pending outcome) instead stages the value, registers an exit
    /// site, and routes outward through the pending machinery.
    pub(super) fn lower_return_edge(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let crosses_finally = self.try_frames.iter().any(|frame| frame.finally.is_some());
        if !crosses_finally && self.finally_overrides.is_empty() {
            self.emit_scope_exit_cleanups(ctx, cleanup)?;
            return self.lower_return(ctx, value);
        }
        // A value-less return inside a value-returning function is
        // checker-guaranteed unreachable fall-off scaffolding.
        let signature = &self.signatures[self.name];
        let value_returning = match &signature.outcome {
            Some(outcome) => !matches!(outcome.ok, LowerTy::ZeroSized),
            None => signature.returns_value || signature.sret.is_some(),
        };
        if value.is_none() && value_returning {
            let unreachable = UnreachableOp::new(ctx);
            self.append(ctx, unreachable.get_operation(), None);
            return Ok(());
        }
        self.stage_return_value(ctx, value)?;
        let code = 2 + self.exit_sites.len() as u32;
        self.exit_sites.push(ExitSiteInfo {
            action: ExitAction::Return {
                cleanup: cleanup.to_vec(),
            },
            overrides: self.finally_overrides.clone(),
            terminal: None,
        });
        self.emit_exit_crossing(ctx, code)
    }

    /// A `break`/`continue` escaping to an enclosing-function block: the
    /// edge's own cleanup runs first (the VM's `run_region`), then the exit
    /// crosses enclosing frames — pending on finalbodies on the way — until
    /// the function-level target. An escape inside a finalbody resolves the
    /// overridden pending outcome at the site (the VM runs the pending
    /// return's roots before propagating the jump).
    pub(super) fn lower_escape_edge(
        &mut self,
        ctx: &mut Context,
        target: usize,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        let crosses_finally = self.try_frames.iter().any(|frame| frame.finally.is_some());
        if !crosses_finally && self.finally_overrides.is_empty() {
            self.emit_scope_exit_cleanups(ctx, &[])?;
            let Some(&block) = self.function_blocks.get(target) else {
                return Err(self.unsupported(format!("escape to missing block bb{target}"), None));
            };
            let jump = BrOp::new(ctx, block, vec![]);
            self.append(ctx, jump.get_operation(), None);
            return Ok(());
        }
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let code = 2 + self.exit_sites.len() as u32;
        self.exit_sites.push(ExitSiteInfo {
            action: ExitAction::Escape { target },
            overrides: Vec::new(),
            terminal: None,
        });
        self.emit_exit_crossing(ctx, code)
    }

    /// The function-exit half of a return terminator (scope-exit cleanups
    /// already ran): store/copy the value per the return ABI and return.
    pub(super) fn lower_return(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
    ) -> Result<(), PlironError> {
        if self.name == "__toplevel__" {
            self.emit_toplevel_binding_releases(ctx)?;
        }
        // Some compiler-private/std-library shells do not themselves conform
        // to `Deinitable` even though their concrete fields do (StringDict's
        // nested List index is the canonical case). MIR therefore has no
        // source-level `DropVar`, but native allocations still need recursive
        // frame cleanup. Explicit drops and moves have already cleared the
        // same flags, so this closes only still-live local slots.
        for var in (self.func.n_params..self.func.n_vars).rev() {
            if self.drop_flags.contains_key(&(var as u32)) {
                self.lower_drop_var(ctx, var as u32)?;
            }
        }
        // A `deinit` receiver consumes its residual fields at function exit.
        // `ConsumeVar` skips the whole-value destructor, so collection APIs
        // which dismantled their elements still release surviving shell
        // fields (for example StringDict's bucket index) exactly once.
        let deinit_params: Vec<u32> = self
            .func
            .deinit_params
            .iter()
            .enumerate()
            .filter_map(|(var, deinit)| deinit.then_some(var as u32))
            .collect();
        for var in deinit_params.into_iter().rev() {
            self.lower_consume_var(ctx, var, true)?;
        }
        self.emit_frame_exit_error_releases(ctx)?;
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            return self.lower_raising_return(ctx, value, &outcome);
        }
        // A value-less return inside a value-returning function is
        // checker-guaranteed unreachable fall-off scaffolding.
        let ret_lower = self.return_value_lower()?;
        let lowered = match (value, ret_lower) {
            (Some(reg), Some(LowerTy::Aggregate { layout, .. })) => {
                // Copy the returned aggregate into the sret out-pointer and
                // return void; the caller owns it.
                let sret = self
                    .sret_ptr
                    .expect("aggregate-returning functions receive an sret pointer");
                let ptr = self.reg_ptr(ctx, reg)?;
                self.mem_copy(ctx, sret, ptr, layout.size, reg);
                self.owned_temps.remove(&reg.0);
                None
            }
            (Some(reg), Some(LowerTy::Scalar(expected))) => {
                Some(self.reg_value(ctx, reg, expected)?)
            }
            (Some(_), Some(LowerTy::ZeroSized)) => None,
            (None, Some(_)) => {
                let unreachable = UnreachableOp::new(ctx);
                self.append(ctx, unreachable.get_operation(), None);
                return Ok(());
            }
            (_, None) => None,
        };
        let ret = ReturnOp::new(ctx, lowered);
        self.append(ctx, ret.get_operation(), value);
        Ok(())
    }

    /// Release `__toplevel__`'s heap-carrying bindings at its exit. Module
    /// scope admits only declarations, so the runtime values of `comptime`
    /// bindings are pure materialization residue: the VM abandons them to
    /// its arena (no destructor ever runs), and every later use reads a
    /// compile-time folded copy. The native release is the same invisible
    /// bookkeeping as the owned-temporary rule — stdlib-authored destructor
    /// chains are pure frees; a chain that would run a user destructor
    /// rejects rather than diverging from the VM's silence.
    pub(super) fn emit_toplevel_binding_releases(
        &mut self,
        ctx: &mut Context,
    ) -> Result<(), PlironError> {
        for var in (0..self.func.n_vars as u32).rev() {
            let Some(ty) = self.func.var_tys.get(&var).cloned() else {
                continue;
            };
            if !matches!(self.var_lower_ty(var)?, LowerTy::Aggregate { .. })
                || !(self.needs_drop(&ty) || self.owns_heap(&ty))
            {
                continue;
            }
            if self.chain_runs_user_lifecycle(&ty, "__deinit__") {
                return Err(self.unsupported(
                    format!("module-level binding of `{ty}` whose teardown runs a user destructor"),
                    None,
                ));
            }
            let Some(flag) = self.drop_flags.get(&var).copied() else {
                return Err(self.unsupported(
                    format!("module-level binding of `{ty}` without a guarded slot"),
                    None,
                ));
            };
            let ptr = self.var_slots[var as usize];
            let cont = self.begin_flag_guard(ctx, flag);
            let traced = self.trace_lifecycle;
            self.trace_lifecycle = false;
            let released = self.emit_drop_value(ctx, ptr, &ty, false);
            self.trace_lifecycle = traced;
            released?;
            self.set_drop_flag(ctx, var, false);
            self.end_flag_guard(ctx, cont);
        }
        Ok(())
    }

    /// Whether `ty`'s teardown/copy chain can reach a user-authored
    /// lifecycle method (`__deinit__`/`__copyinit__`/`__moveinit__`),
    /// walking struct fields and pointer element types (a container reaches
    /// pointed-to elements through its compiled chain). Stdlib-authored
    /// chains are exempt: pure frees/relocations, nothing user-observable.
    pub(super) fn chain_runs_user_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        match ty {
            Ty::Struct(name, _) => {
                let template = name.split("$mono").next().unwrap_or(name);
                let stdlib = template.starts_with("__module$std$")
                    || mojito_symbol::symbol::is_stdlib_string_struct(name)
                    || matches!(
                        template,
                        "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
                    );
                if !stdlib && self.declarations.contains_key(&format!("{name}.{method}")) {
                    return true;
                }
                self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                    decl.fields
                        .iter()
                        .any(|(_, field)| self.chain_runs_user_lifecycle(field, method))
                })
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.chain_runs_user_lifecycle(element, method)),
            Ty::Pointer { element, .. } => self.chain_runs_user_lifecycle(element, method),
            _ => false,
        }
    }

    /// A normal return from a raising function: store the ok payload into the
    /// outcome, tag it `MJ_TAG_OK`, and return void.
    pub(super) fn lower_raising_return(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
        outcome: &OutcomeAbi,
    ) -> Result<(), PlironError> {
        let outcome_ptr = self
            .outcome_ptr
            .expect("raising functions receive an outcome pointer");
        match (&outcome.ok, value) {
            (LowerTy::ZeroSized, _) => {}
            (_, None) => {
                // A value-less return inside a value-returning function is
                // checker-guaranteed unreachable fall-off scaffolding.
                let unreachable = UnreachableOp::new(ctx);
                self.append(ctx, unreachable.get_operation(), None);
                return Ok(());
            }
            (LowerTy::Scalar(expected), Some(reg)) => {
                let value = self.reg_value(ctx, reg, *expected)?;
                let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(reg));
            }
            (LowerTy::Aggregate { layout, .. }, Some(reg)) => {
                let size = layout.size;
                let ptr = self.reg_ptr(ctx, reg)?;
                let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                self.mem_copy(ctx, address, ptr, size, reg);
                // The caller owns the payload now.
                self.owned_temps.remove(&reg.0);
            }
        }
        let tag = self.tag_constant(ctx, mojito_native::native::rt_abi::MJ_TAG_OK);
        let store = StoreOp::new(ctx, tag, outcome_ptr);
        self.append(ctx, store.get_operation(), None);
        let ret = ReturnOp::new(ctx, None);
        self.append(ctx, ret.get_operation(), None);
        Ok(())
    }
}
