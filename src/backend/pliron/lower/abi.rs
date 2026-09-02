//! Call emission ABI: argument materialization, bound/raising call
//! shapes, and load/store/GEP/alloca primitives.

use super::*;

impl<'a> FnLowering<'a> {
    /// The bound operand value of one argument at its expected lowered type.
    /// A consuming (`owned`) parameter takes ownership — an owned temporary
    /// passed there transfers to the callee, which destroys it.
    pub(super) fn arg_value(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        expected: &LowerTy,
        owned: bool,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if owned {
            self.owned_temps.remove(&reg.0);
        }
        match expected {
            LowerTy::Scalar(scalar) => self.reg_value(ctx, reg, *scalar),
            LowerTy::Aggregate { ty, .. } => {
                // A list literal is checked as the fixed-size `Array[T, N]`
                // produced by its literal expression, then implicitly
                // converted at a consuming `List[T]` parameter boundary.
                // Both containers own the same contiguous element buffer,
                // but Array stores `{data, len}` while List stores
                // `{data, len, cap}`. Passing the Array address directly
                // makes the callee read an out-of-bounds, uninitialized cap;
                // O1 exposed that as allocator corruption. Materialize the
                // checked conversion explicitly and transfer the buffer.
                if owned
                    && matches!(ty.as_ref(), Ty::Struct(name, _) if name.split("$mono").next() == Some("List"))
                    && matches!(self.func.reg_types.get(&reg.0), Some(Ty::Struct(name, _)) if name.split("$mono").next() == Some("Array"))
                {
                    return self.materialize_owned_array_as_list(ctx, reg, dest);
                }
                // A literal argument entering a nominal-String parameter
                // materializes through the constructor bridge — the VM's
                // runtime coercion for generic parameters the checker could
                // not wrap at check time.
                if matches!(ty.as_ref(), Ty::Struct(name, _)
                        if crate::symbol::is_stdlib_string_struct(name))
                    && !matches!(self.func.reg_types.get(&reg.0), Some(Ty::Struct(..)))
                    && (self.str_consts.contains_key(&reg.0)
                        || self.str_runtime.contains_key(&reg.0)
                        || matches!(self.func.reg_types.get(&reg.0), Some(Ty::StringLiteral)))
                {
                    return self.materialize_string_argument(ctx, reg, ty, owned, dest);
                }
                self.reg_ptr(ctx, reg)
            }
            LowerTy::ZeroSized => Err(self.unsupported_reg("zero-sized argument".into(), dest)),
        }
    }

    /// Bind an immutable aggregate argument directly to its checked caller
    /// place. MIR's preceding `LoadPlace` is scaffolding for the VM value
    /// model; cloning it natively would run `__copyinit__` in addition to the
    /// call's own copy boundary.
    pub(super) fn place_backed_arg_value(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        expected: &LowerTy,
        owned: bool,
        place: Option<&MirPlace>,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if !owned && matches!(expected, LowerTy::Aggregate { .. }) {
            let place = place
                .cloned()
                .or_else(|| self.loaded_places.get(&reg.0).cloned());
            if let Some(place) = place {
                return Ok(self.place_address(ctx, &place, dest)?.0);
            }
        }
        self.arg_value(ctx, reg, expected, owned, dest)
    }

    /// Transfer an owning `{data, len}` Array temporary into the List
    /// descriptor `{data, len, cap=len}` selected by the checker.
    pub(super) fn materialize_owned_array_as_list(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let array = self.reg_ptr(ctx, reg)?;
        let ptr_ty = ScalarTy::Ptr.handle(ctx);
        let data = LoadOp::new(ctx, array, ptr_ty);
        self.append(ctx, data.get_operation(), Some(dest));
        let len_address = self.offset_address(ctx, array, 8);
        let int_ty = ScalarTy::Int.handle(ctx);
        let len = LoadOp::new(ctx, len_address, int_ty);
        self.append(ctx, len.get_operation(), Some(dest));
        let storage = self.entry_alloca(ctx, 24, 8);
        self.store_string_fields(
            ctx,
            storage,
            data.get_result(ctx),
            len.get_result(ctx),
            len.get_result(ctx),
            dest,
        );
        self.reg_values.insert(reg.0, storage);
        Ok(storage)
    }

    /// Materialize a literal-shaped register as an owned nominal String for
    /// a String-typed parameter slot. The register's storage becomes the
    /// materialized struct (its first 16 bytes still read as the literal
    /// descriptor); a borrowed materialization is released after the
    /// register's last use, an owned one transfers to the callee.
    pub(super) fn materialize_string_argument(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        string_ty: &Ty,
        owned: bool,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let (data, len) = self.writer_argument_text(ctx, reg, dest)?;
        let copy = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, copy, data, len, dest);
        let storage = self.entry_alloca(ctx, 24, 8);
        self.store_string_fields(ctx, storage, copy, len, len, dest);
        self.reg_values.insert(reg.0, storage);
        if !owned {
            self.mark_owned_temp(reg, string_ty.clone())?;
        }
        Ok(storage)
    }

    /// Emit the call to compiled `name` with fully bound operands, prepending
    /// fresh sret storage for an aggregate return and defining or erasing
    /// `dest` by the callee's result kind.
    pub(super) fn emit_bound_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        let signature = &self.signatures[name];
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let (func_ty, returns_value, sret, outcome) = (
            signature.func_ty,
            signature.returns_value,
            signature.sret,
            signature.outcome.clone(),
        );
        let abandoned = if signature.empty_body {
            signature
                .params
                .iter()
                .zip(&signature.owned_params)
                .zip(&operands)
                .filter_map(|((parameter, owned), operand)| {
                    (*owned).then_some((parameter.clone(), *operand))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.emit_call_shaped(
            ctx,
            dest,
            CallOpCallable::Direct(callee),
            func_ty,
            returns_value,
            sret,
            outcome,
            operands,
        )?;
        for (parameter, storage) in abandoned {
            if let LowerTy::Aggregate { ty, .. } = parameter {
                self.emit_release_storage(ctx, storage, &ty)?;
            }
        }
        Ok(())
    }

    /// Emit a direct or indirect call with fully bound operands under the
    /// shared result shape: a raising callee branches on its tagged outcome,
    /// an aggregate return takes prepended fresh sret storage, and `dest` is
    /// defined or erased by the result kind.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_call_shaped(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callable: CallOpCallable,
        func_ty: TypedHandle<FuncType>,
        returns_value: bool,
        sret: Option<Layout>,
        outcome: Option<OutcomeAbi>,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        if let Some(outcome) = outcome {
            return self.emit_raising_call(ctx, dest, callable, func_ty, outcome, operands);
        }
        if let Some(layout) = sret {
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            operands.insert(0, storage);
            let call = CallOp::new(ctx, callable, func_ty, operands);
            self.append(ctx, call.get_operation(), Some(dest));
            self.reg_values.insert(dest.0, storage);
            // The callee's return transferred ownership here; a discarded or
            // borrowed-only aggregate result is an owned temporary.
            if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
                && ((self.owns_heap(&ty) && self.releasable(&ty)) || self.stdlib_deinit_temp(&ty))
            {
                self.mark_owned_temp(dest, ty)?;
            }
            Ok(())
        } else {
            let call = CallOp::new(ctx, callable, func_ty, operands);
            if returns_value {
                self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
            } else {
                self.append(ctx, call.get_operation(), Some(dest));
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Call a raising function through its prepended outcome out-pointer and
    /// branch on the tag: the error edge stages the callee's error and jumps
    /// to the innermost raise-edge target; lowering continues in the ok
    /// block with the payload bound (so post-call effects like receiver
    /// write-back run only on success, matching the VM).
    pub(super) fn emit_raising_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callable: CallOpCallable,
        func_ty: TypedHandle<FuncType>,
        outcome: OutcomeAbi,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        // A reference-yielding raising callee's ok payload is the place
        // pointer; the `Scalar(Ptr)` extraction below defines the destination
        // as that handle — the checked `reference_result` contract.
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        operands.insert(0, storage);
        let call = CallOp::new(ctx, callable, func_ty, operands);
        self.append(ctx, call.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let err_tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_ERR);
        let is_err = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), err_tag);
        self.append(ctx, is_err.get_operation(), Some(dest));
        let region = self.region.expect("lowering is inside a function");
        let err_block = BasicBlock::new(ctx, None, vec![]);
        err_block.insert_at_back(region, ctx);
        let ok_block = BasicBlock::new(ctx, None, vec![]);
        ok_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_err.get_result(ctx),
            err_block,
            vec![],
            ok_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        self.current = Some(err_block);
        let err_slot = self.ensure_err_slot(ctx);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.mem_copy(ctx, err_slot, err_address, 24, dest);
        // A propagating call inside a finalbody overrides the pending
        // outcome, like a raise.
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let target = self.raise_edge_target(ctx)?;
        let jump = BrOp::new(ctx, target, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(ok_block);
        match outcome.ok {
            LowerTy::Scalar(scalar) => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, .. } => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                self.reg_values.insert(dest.0, address);
                // The ok payload transferred ownership here, like an sret
                // result.
                if (self.owns_heap(&ty) && self.releasable(&ty)) || self.stdlib_deinit_temp(&ty) {
                    self.mark_owned_temp(dest, *ty)?;
                }
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Call a raising `__next__` immediately after its checker-selected
    /// bounded `HasNext`. Exhaustion is unreachable under that protocol; keep
    /// the native failure explicit if a malformed iterator violates it, while
    /// allowing the enclosing helper to remain nonraising.
    pub(super) fn emit_bounded_raising_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callable: CallOpCallable,
        func_ty: TypedHandle<FuncType>,
        outcome: OutcomeAbi,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        operands.insert(0, storage);
        let call = CallOp::new(ctx, callable, func_ty, operands);
        self.append(ctx, call.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let err_tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_ERR);
        let is_err = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), err_tag);
        self.append(ctx, is_err.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            is_err.get_result(ctx),
            TrapCategory::UnhandledError,
            dest,
        )?;
        match outcome.ok {
            LowerTy::Scalar(scalar) => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, .. } => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                self.reg_values.insert(dest.0, address);
                if (self.owns_heap(&ty) && self.releasable(&ty)) || self.stdlib_deinit_temp(&ty) {
                    self.mark_owned_temp(dest, *ty)?;
                }
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Relocate the value at `address` into `dest` without invoking copy
    /// lifecycle. Storage-take instructions tombstone or abandon the source;
    /// cloning here would leak the original buffer when the old arena is
    /// released after a grow.
    pub(super) fn load_moved_from(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        dest: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, storage, address, layout.size, dest);
                self.reg_values.insert(dest.0, storage);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Load the value at `address` with checked type `ty` into `dest`:
    /// scalars load directly; aggregates copy out into fresh storage — the
    /// VM's clone-on-read place semantics. A heap-owning aggregate clones
    /// deeply (a byte copy would alias buffers both owners release), and a
    /// releasable clone is an owned temporary.
    pub(super) fn load_from(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        dest: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                if self.has_lifecycle_method(&ty, "__copyinit__")
                    || self.has_nested_lifecycle(&ty, "__copyinit__")
                {
                    return self.copy_aggregate(ctx, dest, &ty, layout, address);
                }
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                if self.owns_heap(&ty) {
                    self.fork_value_into(ctx, storage, &ty, layout, address, dest)?;
                    self.reg_values.insert(dest.0, storage);
                    // The fork's own allocations are exactly its duplicated
                    // String/Error buffers, which the invisible-release rule
                    // frees regardless of user copy constructors.
                    self.mark_owned_temp(dest, (*ty).clone())?;
                    return Ok(());
                }
                self.mem_copy(ctx, storage, address, layout.size, dest);
                self.reg_values.insert(dest.0, storage);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Store register `src` (checked type `ty`) to `address`.
    pub(super) fn store_to(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        src: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(src))? {
            LowerTy::Scalar(scalar) => {
                // A pending literal entering literal-typed storage converts
                // exactly (reject-never-wrap) rather than at the slot kind.
                let value = if matches!(ty, Ty::IntLiteral | Ty::FloatLiteral)
                    && let Some(literal) = self.pending_literals.get(&src.0).cloned()
                {
                    let constant = self.exact_literal_storage(ctx, &literal, ty, src)?;
                    self.reg_values.insert(src.0, constant);
                    constant
                } else {
                    self.reg_value(ctx, src, scalar)?
                };
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(src));
                Ok(())
            }
            LowerTy::Aggregate { ty, layout } => {
                let ptr = self.reg_ptr(ctx, src)?;
                // Owned string bytes cannot enter literal-typed storage:
                // the literal value model is drop-inert, so the buffer would
                // lose its releasing owner (the recorded literal-ownership
                // gap behind the struct-to-literal bridge rejection).
                if matches!(*ty, Ty::StringLiteral) && self.owned_temps.contains_key(&src.0) {
                    return Err(self.unsupported(
                        "owned string bytes entering drop-inert literal storage".into(),
                        self.reg_span(src),
                    ));
                }
                // An owned temporary transfers into the designated storage;
                // a borrowed heap-owning source clones instead — its byte
                // copy would alias buffers both owners release.
                if self.owned_temps.remove(&src.0).is_some() || !self.owns_heap(&ty) {
                    self.mem_copy(ctx, address, ptr, layout.size, src);
                    return Ok(());
                }
                self.fork_value_into(ctx, address, &ty, layout, ptr, src)
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    /// The storage pointer of an aggregate-valued register. A compile-time
    /// StringLiteral consumed as storage materializes on first use as a
    /// borrowed `MjStrDesc` over its interned constant bytes.
    pub(super) fn reg_ptr(&mut self, ctx: &mut Context, reg: Reg) -> Result<Value, PlironError> {
        if let Some(value) = self.reg_values.get(&reg.0) {
            return Ok(*value);
        }
        if let Some(bytes) = self.str_consts.get(&reg.0).cloned() {
            let storage = self.entry_alloca(ctx, 16, 8);
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, reg);
            let store_data = StoreOp::new(ctx, data, storage);
            self.append(ctx, store_data.get_operation(), Some(reg));
            let len_address = self.gep_byte(ctx, storage, 8, reg);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            let store_len = StoreOp::new(ctx, len, len_address);
            self.append(ctx, store_len.get_operation(), Some(reg));
            self.reg_values.insert(reg.0, storage);
            return Ok(storage);
        }
        Err(self.unsupported(
            format!("read of undefined aggregate register %r{}", reg.0),
            self.reg_span(reg),
        ))
    }

    /// `base + offset` bytes as an opaque pointer (a GEP over `i8`).
    pub(super) fn gep_byte(
        &mut self,
        ctx: &mut Context,
        base: Value,
        offset: u64,
        dest: Reg,
    ) -> Value {
        let gep = self.gep_byte_op(ctx, base, offset);
        self.append(ctx, gep.get_operation(), Some(dest));
        gep.get_result(ctx)
    }

    /// [`FnLowering::gep_byte`] without a span register (drop paths).
    pub(super) fn gep_byte_unspanned(
        &mut self,
        ctx: &mut Context,
        base: Value,
        offset: u64,
    ) -> Value {
        let gep = self.gep_byte_op(ctx, base, offset);
        self.append(ctx, gep.get_operation(), None);
        gep.get_result(ctx)
    }

    pub(super) fn gep_byte_op(
        &mut self,
        ctx: &mut Context,
        base: Value,
        offset: u64,
    ) -> GetElementPtrOp {
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let index = u32::try_from(offset).expect("aggregate offsets fit u32");
        GetElementPtrOp::new(ctx, base, vec![GepIndex::Constant(index)], i8_ty)
    }

    /// `llvm.memcpy.p0.p0.i64(dest, src, len, volatile=false)`.
    pub(super) fn mem_copy(
        &mut self,
        ctx: &mut Context,
        dest: Value,
        src: Value,
        len: u64,
        span_reg: Reg,
    ) {
        if len == 0 {
            return;
        }
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, ptr_ty, i64_ty, i1_ty], false);
        let len_value = self.uint_constant(ctx, len);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memcpy.p0.p0.i64".to_string()),
            fn_ty,
            vec![dest, src, len_value, volatile],
        );
        self.append(ctx, call.get_operation(), Some(span_reg));
    }

    /// Fresh typed scalar storage hoisted to the entry block. Scalar slots
    /// loaded and stored at their own type must carry that element type —
    /// mem2reg promotes an alloca at its element type, and a byte-array slot
    /// would promote as `i8` under typed loads.
    pub(super) fn entry_typed_alloca(&mut self, ctx: &mut Context, handle: TypeHandle) -> Value {
        let entry = self.entry.expect("lowering is inside a function");
        let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(1, bw(64)));
        let count = ConstantOp::new(ctx, Box::new(attr));
        let alloca = AllocaOp::new(ctx, handle, count.get_result(ctx));
        alloca.get_operation().insert_at_front(entry, ctx);
        count.get_operation().insert_at_front(entry, ctx);
        alloca.get_result(ctx)
    }

    /// Fresh byte storage hoisted to the top of the entry block, so blocks
    /// that execute repeatedly (loops) reuse one slot instead of growing the
    /// stack. Zero-sized storage still allocates one byte for a stable
    /// address.
    pub(super) fn entry_alloca(&mut self, ctx: &mut Context, size: u64, align: u64) -> Value {
        let entry = self.entry.expect("lowering is inside a function");
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(size.max(1), bw(64)));
        let count = ConstantOp::new(ctx, Box::new(attr));
        let alloca = AllocaOp::new(ctx, i8_ty, count.get_result(ctx));
        alloca.set_alignment(ctx, align as u32);
        // Prepend `[count, alloca]` so the storage precedes every use.
        alloca.get_operation().insert_at_front(entry, ctx);
        count.get_operation().insert_at_front(entry, ctx);
        alloca.get_result(ctx)
    }
}
