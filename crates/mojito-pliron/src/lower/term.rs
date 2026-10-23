//! Terminator lowering: return edges, region exits, and top-level binding
//! releases.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// Lower `term`, which ends a block as `end` says: a return leaves the
    /// function, a raise to its raise-edge target, a region exit the region
    /// op it names, and a region end the region it sits in.
    pub(super) fn lower_term(
        &mut self,
        ctx: &mut Context,
        term: &MirTerm,
        end: RegionEnd,
    ) -> Result<(), PlironError> {
        match term {
            MirTerm::Return(value) => {
                self.release_leaving_temps(ctx, 0, term)?;
                self.lower_return(ctx, value.as_ref().copied())
            }
            MirTerm::Yield(_) | MirTerm::ElifCond { .. } | MirTerm::TryYield(_) => {
                self.lower_region_end(ctx, term, end)
            }
            MirTerm::Break { values, .. }
            | MirTerm::Continue { values, .. }
            | MirTerm::MatchNext(values)
            | MirTerm::MatchComplete(values) => self.lower_region_exit(ctx, term, values),
            MirTerm::Raise { src, label, values } => self.lower_raise(ctx, *src, *label, values),
            // `hlcf.unreachable`.
            MirTerm::Unreachable => {
                let unreachable = UnreachableOp::new(ctx);
                self.append(ctx, unreachable.get_operation(), None);
                Ok(())
            }
            // Concrete MIR carries none: the elaborator decides every
            // compile-time region.
            MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak(_)
            | MirTerm::ComptimeContinue(_) => Err(self.unsupported(
                format!(
                    "the terminator `{}` reached native lowering",
                    mojito_mir::mir::text::terminator_mnemonic(term)
                ),
                None,
            )),
        }
    }

    /// A return terminator: store/copy the value per the return ABI and
    /// return.
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
        // same flags, so this closes only still-live local slots. A
        // collector slot an inlined callee's call built, typed the pack
        // storage, is left out: a `read` collector's elements are the
        // caller's, and an owned one's drop is the body's own.
        for var in (self.func.n_params..self.func.n_vars).rev() {
            let collector = matches!(
                self.func.var_tys.get(&(var as u32)).map(Ty::kind),
                Some(TyKind::VariadicPack(_) | TyKind::Tuple(_) | TyKind::RuntimePack(_))
            );
            if !collector && self.drop_flags.contains_key(&(var as u32)) {
                let destructor = self
                    .func
                    .var_tys
                    .get(&(var as u32))
                    .and_then(|ty| self.type_destructor(ty));
                self.lower_drop_var(ctx, var as u32, destructor)?;
            }
        }
        // A `deinit` receiver's elaborated `ConsumeVar` consumes its residual
        // fields at its last use and clears its flag; this flag-guarded exit
        // consume is the backstop for a receiver the body never uses.
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
            (Some(reg), Some(LowerTy::Aggregate { ty, layout })) => {
                // Copy the returned aggregate into the sret out-pointer and
                // return void; the caller owns it.
                let sret = self
                    .sret_ptr
                    .expect("aggregate-returning functions receive an sret pointer");
                let ptr = self.reg_ptr(ctx, reg)?;
                self.copy_value(ctx, sret, ptr, &ty, layout, reg);
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
            let released = self.emit_drop_value(ctx, ptr, &ty);
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
        match ty.kind() {
            TyKind::Struct(name, _) => {
                let template = name.split("$mono").next().unwrap_or(name);
                let stdlib = template.starts_with("__module$std$")
                    || mojito_symbol::symbol::is_stdlib_string_struct(name)
                    || matches!(
                        template,
                        "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
                    );
                if !stdlib && self.lifecycle_symbol(name, method).is_some() {
                    return true;
                }
                self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                    decl.fields
                        .iter()
                        .any(|(_, field)| self.chain_runs_user_lifecycle(field, method))
                })
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.chain_runs_user_lifecycle(element, method)),
            TyKind::Pointer { element, .. } => self.chain_runs_user_lifecycle(element, method),
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
            (LowerTy::Aggregate { ty, layout }, Some(reg)) => {
                let (ty, layout) = (ty.clone(), *layout);
                let ptr = self.reg_ptr(ctx, reg)?;
                let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                self.copy_value(ctx, address, ptr, &ty, layout, reg);
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
