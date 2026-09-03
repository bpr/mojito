//! Error-path plumbing: traces, error slots, raise edges, the propagate
//! block, and `Raise` lowering.

use super::*;

impl<'a> FnLowering<'a> {
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
        // (`List$mono$TInt`). Checker-specialized names (`Tuple$t2[…]`) are
        // the runtime struct name on both sides and pass through.
        let text = text.split("$mono").next().unwrap_or(text);
        let global = self.shared.intern_string(ctx, text.as_bytes());
        let data = self.global_address(ctx, &global, Reg(u32::MAX));
        let len = self.uint_constant(ctx, text.len() as u64);
        self.emit_trace(ctx, kind, data, len);
    }

    /// A lifecycle event carrying the staged error's message.
    pub(super) fn emit_trace_err_slot(&mut self, ctx: &mut Context, kind: u32) {
        let err_slot = self.ensure_err_slot(ctx);
        let (data, size) = self.string_parts(ctx, err_slot, Reg(u32::MAX));
        self.emit_trace(ctx, kind, data, size);
    }

    /// Free the buffers of still-initialized error-typed locals on a normal
    /// return. Drop elaboration never drops a bound-but-unused handler
    /// error (the VM abandons it to its arena at frame end), so the frame
    /// exit releases it invisibly — error values are never buffer-shared
    /// (copies are deep), and borrowed parameters are excluded (their value
    /// belongs to the caller).
    pub(super) fn emit_frame_exit_error_releases(
        &mut self,
        ctx: &mut Context,
    ) -> Result<(), PlironError> {
        let mut vars: Vec<u32> = self.drop_flags.keys().copied().collect();
        vars.sort_unstable();
        for var in vars {
            if !matches!(self.func.var_tys.get(&var), Some(Ty::Error)) {
                continue;
            }
            let borrowed_param = (var as usize) < self.func.n_params
                && !self
                    .func
                    .owned_params
                    .get(var as usize)
                    .copied()
                    .unwrap_or(false);
            if borrowed_param {
                continue;
            }
            let flag = self.drop_flags[&var];
            let cont = self.begin_flag_guard(ctx, flag);
            let slot = self.var_slots[var as usize];
            self.emit_release_storage(ctx, slot, &Ty::Error)?;
            self.end_flag_guard(ctx, cont);
        }
        Ok(())
    }

    /// The entry-block MjError staging slot for in-flight errors.
    pub(super) fn ensure_err_slot(&mut self, ctx: &mut Context) -> Value {
        if let Some(slot) = self.err_slot {
            return slot;
        }
        let slot = self.entry_alloca(ctx, 24, 8);
        self.err_slot = Some(slot);
        slot
    }

    /// The innermost raise-edge target: the innermost enclosing `try`'s
    /// landing block, else the raising function's propagate block. The staged
    /// error must already sit in the err slot when jumping here.
    pub(super) fn raise_edge_target(
        &mut self,
        ctx: &mut Context,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        if let Some(frame) = self.try_frames.last() {
            // A raise landing on a handler-less `try`/`finally` body or a
            // handler/orelse pseudo-frame pends an error on the finalbody.
            if let Some(idx) = frame.finally
                && frame.pends_error
            {
                self.finally_states[idx].error_possible = true;
            }
            return Ok(frame.landing);
        }
        self.ensure_propagate_block(ctx)
    }

    /// The per-function propagate block of a raising function: free the heap
    /// buffers of still-initialized releasable locals (no user destructor
    /// runs — the VM abandons raising frames and its arena reclaims the
    /// memory invisibly; other droppable locals are a recorded leak residue),
    /// move the staged error into the outcome's error slot, tag the outcome,
    /// and return.
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
        let mut vars: Vec<u32> = self.drop_flags.keys().copied().collect();
        vars.sort_unstable();
        for var in vars {
            let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(var)? else {
                continue;
            };
            if !self.owns_heap(&ty) || !self.releasable(&ty) {
                continue;
            }
            let flag = self.drop_flags[&var];
            let cont = self.begin_flag_guard(ctx, flag);
            let slot = self.var_slots[var as usize];
            self.emit_release_storage(ctx, slot, &ty)?;
            self.end_flag_guard(ctx, cont);
        }
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
    /// allocation, an owned runtime string or String temporary transfers its
    /// allocation, a live nominal String copies its bytes (the VM clones the
    /// message and drops the String normally), and an error value moves.
    pub(super) fn store_error_into(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        src: Reg,
    ) -> Result<(), PlironError> {
        if let Some(descriptor) = self.str_runtime.get(&src.0).copied() {
            let data = if descriptor.owned {
                self.owned_temps.remove(&src.0);
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
        match self.func.reg_types.get(&src.0) {
            Some(Ty::Struct(name, _)) if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, src)?;
                if self.owned_temps.remove(&src.0).is_some() {
                    // The temporary transfers its whole allocation.
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
            Some(Ty::Error) => {
                let ptr = self.reg_ptr(ctx, src)?;
                self.mem_copy(ctx, storage, ptr, 24, src);
                self.owned_temps.remove(&src.0);
                Ok(())
            }
            // A nullary error struct (`raise StopIteration()`) carries no
            // runtime payload; its owned message is the VM's `Display` of the
            // value, `Name()`. Structs with fields keep rejecting: their
            // display embeds runtime field values.
            Some(ty @ Ty::Struct(name, _))
                if self
                    .layout
                    .layout_of(ty)
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
        // consumer is `Raise`; `store_error_into` transfers this allocation.
        // Do not schedule ordinary temporary cleanup between construction
        // and the raise edge.
        Ok(())
    }

    /// `Raise`: materialize the raised value as an owned error in the staging
    /// slot and jump to the innermost raise-edge target (a `try` landing
    /// block once regions lower; the raising function's propagate block
    /// otherwise). Lowering continues into a fresh unreachable block for the
    /// dead remainder of the MIR block.
    pub(super) fn lower_raise(&mut self, ctx: &mut Context, src: Reg) -> Result<(), PlironError> {
        let err_slot = self.ensure_err_slot(ctx);
        self.store_error_into(ctx, err_slot, src)?;
        // The VM's lifecycle log records only `Value::Error` raises; a raised
        // error struct (`raise StopIteration()`) stays silent there.
        let struct_raise = matches!(self.func.reg_types.get(&src.0), Some(Ty::Struct(name, _))
            if !mojito_symbol::symbol::is_stdlib_string_struct(name));
        if self.trace_lifecycle && !struct_raise {
            self.emit_trace_err_slot(ctx, mojito_native::native::rt_abi::TRACE_RAISE);
        }
        // A raise inside a finalbody overrides the pending outcome; the VM
        // runs a pending return's roots before propagating.
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let target = self.raise_edge_target(ctx)?;
        let jump = BrOp::new(ctx, target, vec![]);
        self.append(ctx, jump.get_operation(), Some(src));
        let region = self.region.expect("lowering is inside a function region");
        let dead = BasicBlock::new(ctx, None, vec![]);
        dead.insert_at_back(region, ctx);
        self.current = Some(dead);
        Ok(())
    }
}
