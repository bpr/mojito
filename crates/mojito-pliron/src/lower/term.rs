//! Terminator lowering: return edges and region exits.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

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
            MirTerm::Return(value) => self.lower_return(ctx, value.as_ref().copied()),
            MirTerm::Yield(_) | MirTerm::ElifCond { .. } | MirTerm::TryYield(_) => {
                self.lower_region_end(ctx, term, end)
            }
            MirTerm::Break { values, .. }
            | MirTerm::Continue { values, .. }
            | MirTerm::MatchNext(values)
            | MirTerm::MatchComplete(values) => self.lower_region_exit(ctx, term, values),
            MirTerm::TryRaise { label, values } => self.lower_try_raise(ctx, *label, values),
            MirTerm::ErrorReturn => self.lower_error_return(ctx),
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
    /// return. Drop elaboration destroyed what the return held before it,
    /// so the frame releases nothing here, as upstream's destructor
    /// insertion leaves a return.
    pub(super) fn lower_return(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
    ) -> Result<(), PlironError> {
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
