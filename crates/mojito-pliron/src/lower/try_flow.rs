//! `Try` region lowering: the body under its raise target, the landing
//! block that binds a raised error for the handler, and the `else`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// `lit.try`, a control-flow node like `hlcf`'s: each part is a block of
    /// its own, entered and left by explicit edges. The body lowers with a
    /// fresh landing block as the target of the raises naming `label`; the
    /// landing block binds the staged error and enters the handler;
    /// `orelse` runs only on normal completion. MIR lowering copied the
    /// `finally` before each exit, so none remains here.
    pub(super) fn lower_try(
        &mut self,
        ctx: &mut Context,
        label: TryLabel,
        body: &[MirBlock],
        handler: Option<&(Option<u32>, Vec<MirBlock>)>,
        orelse: Option<&[MirBlock]>,
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let after = BasicBlock::new(ctx, None, vec![]);
        after.insert_at_back(region, ctx);
        let landing = BasicBlock::new(ctx, None, vec![]);
        landing.insert_at_back(region, ctx);
        let normal_exit = BasicBlock::new(ctx, None, vec![]);
        normal_exit.insert_at_back(region, ctx);

        // Body: raises naming this `try` land on its landing block.
        self.try_frames.push(TryFrame { label, landing });
        let body_entry = self.lower_part(ctx, body, normal_exit)?;
        self.try_frames.pop();
        // Enter the try from the enclosing block.
        let enter = BrOp::new(ctx, body_entry, vec![]);
        self.append(ctx, enter.get_operation(), None);

        // Landing: the handler binds the staged error. A `try` no raise
        // names has no handler, and its landing block no predecessor.
        self.current = Some(landing);
        if let Some((error_var, handler_blocks)) = handler {
            if self.trace_lifecycle {
                self.emit_trace_err_slot(ctx, mojito_native::native::rt_abi::TRACE_CATCH);
            }
            let err_slot = self.ensure_err_slot(ctx);
            if let Some(var) = error_var {
                // A still-initialized previous binding (a loop rebinding the
                // same handler var) frees first — the VM abandons the
                // overwritten value to its arena.
                if let Some(&flag) = self.drop_flags.get(var) {
                    let cont = self.begin_flag_guard(ctx, flag);
                    let slot = self.var_slots[*var as usize];
                    self.emit_release_storage(ctx, slot, &Ty::error())?;
                    self.end_flag_guard(ctx, cont);
                }
                // The staged error moves into the bound slot; its ordinary
                // drop frees the message.
                let slot = self.var_slots[*var as usize];
                self.mem_copy(ctx, slot, err_slot, 24, Reg(u32::MAX));
                self.set_drop_flag(ctx, *var, true);
            } else {
                // No binder: the caught error is dropped on entry.
                self.emit_release_storage(ctx, err_slot, &Ty::error())?;
            }
            let handler_entry = self.lower_part(ctx, handler_blocks, after)?;
            let jump = BrOp::new(ctx, handler_entry, vec![]);
            self.append(ctx, jump.get_operation(), None);
        } else {
            let unreachable = UnreachableOp::new(ctx);
            self.append(ctx, unreachable.get_operation(), None);
        }

        // Normal completion: `orelse` (only here), then the continuation.
        self.current = Some(normal_exit);
        let normal_target = match orelse {
            Some(orelse_blocks) => self.lower_part(ctx, orelse_blocks, after)?,
            None => after,
        };
        let jump = BrOp::new(ctx, normal_target, vec![]);
        self.append(ctx, jump.get_operation(), None);

        self.current = Some(after);
        Ok(())
    }

    /// Lower the `try` part `blocks` into a block of its own, whose
    /// `lit.try.yield` branches to `completed`; returns that block.
    /// `self.current` is restored.
    pub(super) fn lower_part(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        completed: Ptr<BasicBlock>,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        let entry = self.region_entry(ctx)?;
        self.lower_region(ctx, blocks, entry, RegionEnd::TryPart(completed))?;
        Ok(self.carrier_block(entry))
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
        match self.func.reg_types.get(&reg.0).map(Ty::kind) {
            Some(TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            // An error value displays as its bare message (the VM's
            // `format_value` over `Value::Error`); a `StringLiteral` held in
            // storage is its descriptor, whose data and length lead it as a
            // String's do.
            Some(TyKind::Error | TyKind::StringLiteral) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            _ => Err(self.unsupported_reg(format!("string value in register %r{}", reg.0), dest)),
        }
    }
}
