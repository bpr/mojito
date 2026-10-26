//! Error-path plumbing: traces, error slots, raise edges, the propagate
//! block, and `raise` lowering.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// `mjrt_trace(kind, data, len)` — one ordered lifecycle event (test
    /// lane only; callers guard on `trace_lifecycle`).
    pub(super) fn emit_trace(&mut self, ctx: &mut Context, kind: u32, data: Value, len: Value) {
        let trace_ty = self.shared.ensure_rt(ctx, "mjrt_trace");
        let kind_value = self.tag_constant(ctx, kind);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_trace".try_into().expect("valid identifier")),
            trace_ty,
            vec![kind_value, data, len],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// A lifecycle event with a compile-time payload (a type name).
    pub(super) fn emit_trace_text(&mut self, ctx: &mut Context, kind: u32, text: &str) {
        // Lifecycle events name types as the VM logs them: the bare template
        // (`List`), never the backend's monomorphized instance spelling
        // (`List$mono$TInt`). Checker-specialized names (`Bag$t2[…]`) are
        // the runtime struct name on both sides and pass through.
        let text = text.split("$mono").next().unwrap_or(text);
        let global = self.shared.intern_string(ctx, text.as_bytes());
        let data = self.global_address(ctx, &global, Reg(u32::MAX));
        let len = self.uint_constant(ctx, text.len() as u64);
        self.emit_trace(ctx, kind, data, len);
    }

    /// A lifecycle event carrying the message of the `MjError` at `storage`.
    pub(super) fn emit_trace_error(&mut self, ctx: &mut Context, kind: u32, storage: Value) {
        let (data, size) = self.string_parts(ctx, storage, Reg(u32::MAX));
        self.emit_trace(ctx, kind, data, size);
    }

    /// The entry-block `MjError` staging slot of the error a raise out of
    /// the function hands out: the storage of its error slot
    /// ([`mojito_mir::mir::MirFunction::error_slot`]).
    pub(super) fn ensure_err_slot(&mut self, ctx: &mut Context) -> Value {
        if let Some(slot) = self.err_slot {
            return slot;
        }
        let slot = self.entry_alloca(ctx, 24, 8);
        self.err_slot = Some(slot);
        slot
    }

    /// The per-function propagate block of a raising function: move the
    /// staged error into the outcome's error slot, tag the outcome, and
    /// return. Each raise edge's own drops destroyed what it held before
    /// reaching it.
    pub(super) fn ensure_propagate_block(
        &mut self,
        ctx: &mut Context,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        if let Some(block) = self.propagate_block {
            return Ok(block);
        }
        let Some(outcome) = self.signatures[self.name].outcome.clone() else {
            return Err(self.unsupported(
                "raise propagation out of a nonraising function".into(),
                None,
            ));
        };
        let outcome_ptr = self
            .outcome_ptr
            .expect("raising functions receive an outcome pointer");
        let err_slot = self.ensure_err_slot(ctx);
        let region = self.region.expect("lowering is inside a function");
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        let saved = self.current;
        self.current = Some(block);
        let err_address = self.offset_address(ctx, outcome_ptr, outcome.err_offset);
        self.mem_copy(ctx, err_address, err_slot, 24, Reg(u32::MAX));
        let tag = self.tag_constant(ctx, mojito_native::native::rt_abi::MJ_TAG_ERR);
        let store = StoreOp::new(ctx, tag, outcome_ptr);
        self.append(ctx, store.get_operation(), None);
        let ret = ReturnOp::new(ctx, None);
        self.append(ctx, ret.get_operation(), None);
        self.current = saved;
        self.propagate_block = Some(block);
        Ok(block)
    }

    /// Materialize the register a `raise` names as an owned `MjError` in
    /// `storage`: a compile-time or borrowed message copies into a fresh
    /// allocation, an owned runtime string or a String register MIR owns
    /// (which the raise consumes) transfers its allocation, any other nominal
    /// String copies its bytes, and an error value moves.
    pub(super) fn store_error_into(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        src: Reg,
    ) -> Result<(), PlironError> {
        if let Some(descriptor) = self.str_runtime.get(&src.0).copied() {
            let data = if descriptor.owned {
                descriptor.data
            } else {
                let data = self.emit_alloc(ctx, descriptor.len, 1, src);
                self.mem_copy_dynamic(ctx, data, descriptor.data, descriptor.len, src);
                data
            };
            self.store_string_fields(ctx, storage, data, descriptor.len, descriptor.len, src);
            return Ok(());
        }
        if let Some(bytes) = self.str_consts.get(&src.0).cloned() {
            let len = bytes.len() as u64;
            let len_value = self.uint_constant(ctx, len);
            let data = self.emit_alloc(ctx, len_value, 1, src);
            if len > 0 {
                let global = self.shared.intern_string(ctx, &bytes);
                let literal = self.global_address(ctx, &global, src);
                self.mem_copy(ctx, data, literal, len, src);
            }
            self.store_string_fields(ctx, storage, data, len_value, len_value, src);
            return Ok(());
        }
        match self.func.reg_types.get(&src.0).map(Ty::kind) {
            Some(TyKind::Struct(name, _))
                if mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                let ptr = self.reg_ptr(ctx, src)?;
                if self.mir_owned.contains(&src.0) {
                    // An owned register the raise consumes transfers its
                    // whole allocation.
                    let (data, size) = self.string_parts(ctx, ptr, src);
                    let cap = self.string_cap(ctx, ptr, src);
                    self.store_string_fields(ctx, storage, data, size, cap, src);
                } else {
                    let (data, size) = self.string_parts(ctx, ptr, src);
                    let copy = self.emit_alloc(ctx, size, 1, src);
                    self.mem_copy_dynamic(ctx, copy, data, size, src);
                    self.store_string_fields(ctx, storage, copy, size, size, src);
                }
                Ok(())
            }
            Some(TyKind::Error) => {
                let ptr = self.reg_ptr(ctx, src)?;
                self.mem_copy(ctx, storage, ptr, 24, src);
                Ok(())
            }
            // A nullary error struct (`raise StopIteration()`) carries no
            // runtime payload; its owned message is the VM's `Display` of the
            // value, `Name()`. Structs with fields keep rejecting: their
            // display embeds runtime field values.
            Some(TyKind::Struct(name, _))
                if self
                    .layout
                    .layout_of(&self.func.reg_types[&src.0])
                    .is_ok_and(|layout| layout.size == 0) =>
            {
                let message = format!("{name}()").into_bytes();
                let len = message.len() as u64;
                let len_value = self.uint_constant(ctx, len);
                let data = self.emit_alloc(ctx, len_value, 1, src);
                let global = self.shared.intern_string(ctx, &message);
                let literal = self.global_address(ctx, &global, src);
                self.mem_copy(ctx, data, literal, len, src);
                self.store_string_fields(ctx, storage, data, len_value, len_value, src);
                Ok(())
            }
            _ => Err(self.unsupported_reg(format!("raised value in register %r{}", src.0), src)),
        }
    }

    /// `base + offset`, skipping the GEP for offset 0.
    pub(super) fn offset_address(&mut self, ctx: &mut Context, base: Value, offset: u64) -> Value {
        if offset == 0 {
            base
        } else {
            self.gep_byte_unspanned(ctx, base, offset)
        }
    }

    /// Emit the i32 tag constant of a tagged outcome.
    pub(super) fn tag_constant(&mut self, ctx: &mut Context, tag: u32) -> Value {
        let i32_int = IntegerType::get(ctx, 32, Signedness::Signless);
        let attr = IntegerAttr::new(i32_int, APInt::from_u64(u64::from(tag), bw(32)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    pub(super) fn lower_error_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("Error construction contract".into(), dest));
        }
        // `Value::Error` owns a cloned message. Copy at construction rather
        // than retaining a descriptor into an argument whose last-use
        // cleanup may run before a later `raise` consumes this value.
        let (source, len) = self.string_bytes(ctx, args[0], dest)?;
        let data = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, data, source, len, dest);
        self.str_runtime.insert(
            dest.0,
            RuntimeStr {
                data,
                len,
                owned: true,
            },
        );
        // Error construction is compiler-internal and its only supported
        // consumer is a `raise`; `store_error_into` transfers this allocation.
        // Do not schedule ordinary temporary cleanup between construction
        // and the raise edge.
        Ok(())
    }

    /// A `lit.error_return`: jump to the raising function's propagate
    /// block, the raise having stored its error into the function's error
    /// slot first ([`Self::stage_function_error`]).
    pub(super) fn lower_error_return(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        let target = self.ensure_propagate_block(ctx)?;
        let jump = BrOp::new(ctx, target, Vec::new());
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// The store of the raised register `src` into the function's error
    /// slot, which a raise out of the function makes before its
    /// `lit.error_return`: the error, materialized as an owned `MjError`
    /// in the staging slot the propagate block hands out, logging the
    /// raise.
    pub(super) fn stage_function_error(
        &mut self,
        ctx: &mut Context,
        src: Reg,
    ) -> Result<(), PlironError> {
        let err_slot = self.ensure_err_slot(ctx);
        self.store_raised(ctx, err_slot, src)
    }

    /// The death of the function's error slot, holding an error a `return`
    /// a `finally` copied over the raise replaced: the staged `MjError` is
    /// released.
    pub(super) fn drop_function_error(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        let err_slot = self.ensure_err_slot(ctx);
        self.emit_release_storage(ctx, err_slot, &Ty::error())
    }

    /// A `try.raise`: jump to the landing block of the `try` labelled
    /// `label`, carrying `values` into the handler's arguments. The raise
    /// stored its error into the `try`'s slot first ([`Self::stage_error`]).
    pub(super) fn lower_try_raise(
        &mut self,
        ctx: &mut Context,
        label: TryLabel,
        values: &[Reg],
    ) -> Result<(), PlironError> {
        let landing = self.raise_target(label)?;
        let operands = self.carry(ctx, landing, values)?;
        let jump = BrOp::new(ctx, self.carrier_block(landing), operands);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// The store of the raised register `src` into the slot `var` of the
    /// `try` it raises to, which catches it. A slot holding an `Error` (or
    /// a String, which a raise converts to one) holds the raised error as
    /// an owned `MjError`, as the staging slot does, and logs the catch;
    /// a typed error is the value itself, a raise edge's `MjError`, whose
    /// message names only the type, released on arrival.
    pub(super) fn stage_error(
        &mut self,
        ctx: &mut Context,
        var: u32,
        src: Reg,
    ) -> Result<(), PlironError> {
        let slot = self.var_slots[var as usize];
        let message = match self.func.var_tys.get(&var).map(Ty::kind) {
            Some(TyKind::Error) => true,
            Some(TyKind::Struct(name, _)) => mojito_symbol::symbol::is_stdlib_string_struct(name),
            _ => false,
        };
        if message {
            self.store_raised(ctx, slot, src)?;
            if self.trace_lifecycle {
                self.emit_trace_error(ctx, mojito_native::native::rt_abi::TRACE_CATCH, slot);
            }
            return Ok(());
        }
        let ty = self
            .func
            .var_tys
            .get(&var)
            .cloned()
            .unwrap_or_else(Ty::error);
        if !self
            .layout
            .layout_of(&ty)
            .is_ok_and(|layout| layout.size == 0)
        {
            return Err(
                self.unsupported_reg(format!("a caught error of type `{ty}` with fields"), src)
            );
        }
        if self.edge_errors.contains(&src.0) {
            let storage = self.reg_ptr(ctx, src)?;
            self.emit_release_storage(ctx, storage, &Ty::error())?;
        }
        Ok(())
    }

    /// Materialize the raised register `src` as an owned `MjError` in
    /// `storage`: a raise edge's error is the error its call raised,
    /// copied whole and traced once, where it was raised; any other value
    /// converts as [`Self::store_error_into`] converts it, logging the
    /// raise.
    fn store_raised(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        src: Reg,
    ) -> Result<(), PlironError> {
        if self.edge_errors.contains(&src.0) {
            let error = self.reg_ptr(ctx, src)?;
            self.mem_copy(ctx, storage, error, 24, src);
            return Ok(());
        }
        self.store_error_into(ctx, storage, src)?;
        // The VM's lifecycle log records only `Value::Error` raises; a
        // raised error struct (`raise StopIteration()`) stays silent there.
        let struct_raise = matches!(self.func.reg_types.get(&src.0).map(Ty::kind), Some(TyKind::Struct(name, _))
            if !mojito_symbol::symbol::is_stdlib_string_struct(name));
        if self.trace_lifecycle && !struct_raise {
            self.emit_trace_error(ctx, mojito_native::native::rt_abi::TRACE_RAISE, storage);
        }
        Ok(())
    }

    /// The landing carrier of the nearest enclosing `try` body carrying
    /// `label`, which a raise naming it enters, as `lit.try.raise`
    /// branches to its `try`'s handler.
    fn raise_target(&self, label: TryLabel) -> Result<usize, PlironError> {
        self.try_frames
            .iter()
            .rev()
            .find(|frame| frame.label == label)
            .map(|frame| frame.landing)
            .ok_or_else(|| {
                self.unsupported(
                    format!("a raise naming try{}, which does not enclose it", label.0),
                    None,
                )
            })
    }
}
