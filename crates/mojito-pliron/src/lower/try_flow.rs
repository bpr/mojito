//! `Try` region lowering: the body under its raise target, the landing
//! block that enters the handler, and the `else`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// `lit.try`, a control-flow node like `hlcf`'s: each part is a block of
    /// its own, entered and left by explicit edges. The body lowers with a
    /// landing block taking the handler's arguments as the target of the
    /// raises naming `label`, which stored the error into the `try`'s slot
    /// first; the landing block enters the handler. The body's `lit.try.yield` enters `orelse` with
    /// its arguments, or the exit with the results without one, and the
    /// handler's and `orelse`'s enter the exit with the results. MIR
    /// lowering copied the `finally` before each exit, so none remains
    /// here.
    pub(super) fn lower_try(
        &mut self,
        ctx: &mut Context,
        (label, results, body): (TryLabel, &[Reg], &[MirBlock]),
        (handler_args, handler): (&[Reg], Option<&[MirBlock]>),
        (else_args, orelse): (&[Reg], Option<&[MirBlock]>),
    ) -> Result<(), PlironError> {
        let exit = self.carrier(ctx, results)?;
        let landing = self.carrier(ctx, handler_args)?;
        let else_entry = orelse.map(|_| self.carrier(ctx, else_args)).transpose()?;

        // Body: raises naming this `try` land on its landing block.
        self.try_frames.push(TryFrame { label, landing });
        let body_entry = self.lower_part(ctx, body, else_entry.unwrap_or(exit));
        self.try_frames.pop();
        let body_entry = body_entry?;
        // Enter the try from the enclosing block.
        let enter = BrOp::new(ctx, body_entry, vec![]);
        self.append(ctx, enter.get_operation(), None);

        // Landing: the handler's arguments. A `try` no raise names has no
        // handler, and its landing block no predecessor.
        self.enter_exit(ctx, landing);
        if let Some(handler_blocks) = handler {
            let handler_entry = self.lower_part(ctx, handler_blocks, exit)?;
            let jump = BrOp::new(ctx, handler_entry, vec![]);
            self.append(ctx, jump.get_operation(), None);
        } else {
            let unreachable = UnreachableOp::new(ctx);
            self.append(ctx, unreachable.get_operation(), None);
        }

        // Normal completion: `orelse`, entered only from the body.
        if let (Some(orelse_blocks), Some(entry)) = (orelse, else_entry) {
            self.lower_region(ctx, orelse_blocks, entry, RegionEnd::TryPart(exit))?;
        }
        self.enter_exit(ctx, exit);
        Ok(())
    }

    /// Lower the `try` part `blocks` into a block of its own, entered with
    /// no values, whose `lit.try.yield` enters the carrier `completed`;
    /// returns that block. `self.current` is restored.
    pub(super) fn lower_part(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        completed: usize,
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
