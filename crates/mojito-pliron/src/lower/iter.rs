//! Iterator lowering: `GetIter`, pack iteration, `HasNext`/`Next` and
//! their raising/reference variants.

use super::*;

impl<'a> FnLowering<'a> {
    /// `GetIter`: normalize the iterable variable through its checker-selected
    /// (and mono-retargeted) `__iter__` chain into the iterator variable.
    /// Receiver conventions mirror the VM: a borrowed (`ref`/`mut`) step
    /// aliases the current storage — for step 0 that is the source slot, the
    /// VM's reference-handle seam, so a borrowing iterator roots at the loop
    /// frame — a `read` step passes a plain byte copy (the VM's
    /// `current.clone()`, no lifecycle copy), and an owned (`var`) step
    /// consumes the current storage in place.
    pub(super) fn lower_get_iter(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        prepare: &[String],
    ) -> Result<(), PlironError> {
        // The compiler-private pack fallback (the VM's `remove(0)` loop):
        // the split slot keeps the pack layout; a backend-side shadow slot
        // holds the advance position. Handled before the identity check —
        // in-place normalization still zeroes the position.
        if prepare.is_empty()
            && let Some(elements) = self.pack_iter_elements(dest)
            && (source == dest
                || matches!(
                    self.func.var_tys.get(&source),
                    Some(Ty::RuntimePack(_) | Ty::Tuple(_))
                ))
        {
            return self.lower_pack_iter_init(ctx, source, dest, &elements);
        }
        if prepare.is_empty() && source == dest {
            // Identity normalization: the slot already holds the iterator.
            return Ok(());
        }
        let LowerTy::Aggregate {
            layout: dest_layout,
            ..
        } = self.var_lower_ty(dest)?
        else {
            return Err(self.unsupported("non-aggregate iterator variable".into(), None));
        };
        // A borrowed named source binds its slot to a reference handle; load
        // it to reach the iterable's storage, as the VM dereferences for
        // method resolution.
        let source_ty = self.func.var_tys.get(&source).cloned().ok_or_else(|| {
            self.unsupported(
                format!(
                    "untyped variable `{}`",
                    self.func
                        .var_names
                        .get(source as usize)
                        .map(String::as_str)
                        .unwrap_or("?")
                ),
                None,
            )
        })?;
        let (mut current, mut current_ty) = if let Ty::Ref(reference) = &source_ty {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, self.var_slots[source as usize], handle);
            self.append(ctx, load.get_operation(), None);
            (load.get_result(ctx), (*reference.referent).clone())
        } else {
            (self.var_slots[source as usize], source_ty)
        };
        // Whether `current` is a chain temporary this instruction owns (the
        // source variable owns its own storage).
        let mut owns_current = false;
        for selected in prepare {
            let Some(signature) = self.signatures.get(selected) else {
                return Err(self.unsupported(
                    format!("iterator preparation via uncompiled `{selected}`"),
                    None,
                ));
            };
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported(format!("raising iterator preparation `{selected}`"), None)
                );
            }
            let Some(receiver_param) = signature.params.first().cloned() else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without a receiver"),
                    None,
                ));
            };
            let Some(result_layout) = signature.sret else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without an aggregate result"),
                    None,
                ));
            };
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            let borrowed = signature.ref_params.first().copied().unwrap_or(false)
                || matches!(
                    self.declarations
                        .get(selected)
                        .and_then(|decl| decl.receiver_convention.as_ref()),
                    Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
                );
            let owned = signature.owned_params.first().copied().unwrap_or(false);
            let (receiver, release_current) = if borrowed || owned {
                // Aliased or consumed in place; a consumed chain temporary
                // needs no release (the callee destroyed it).
                (current, false)
            } else {
                let LowerTy::Aggregate { layout, .. } = receiver_param else {
                    return Err(self.unsupported(
                        format!("iterator preparation `{selected}` on a scalar receiver"),
                        None,
                    ));
                };
                let copy = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, copy, current, layout.size, Reg(u32::MAX));
                (copy, owns_current)
            };
            let result = self.entry_alloca(ctx, result_layout.size, result_layout.align);
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                func_ty,
                vec![result, receiver],
            );
            self.append(ctx, call.get_operation(), None);
            if release_current {
                // The VM's superseded intermediate drops silently (no user
                // destructor); free its heap invisibly or reject.
                if self.owns_heap(&current_ty) {
                    if self.releasable(&current_ty) {
                        self.emit_release_storage(ctx, current, &current_ty)?;
                    } else {
                        return Err(self.unsupported(
                            format!(
                                "iterator preparation abandoning `{current_ty}` with destructor work"
                            ),
                            None,
                        ));
                    }
                }
            }
            current = result;
            current_ty = self
                .declarations
                .get(selected)
                .map(|decl| decl.ret_ty.clone())
                .ok_or_else(|| {
                    self.unsupported(
                        format!("iterator preparation `{selected}` without declaration facts"),
                        None,
                    )
                })?;
            owns_current = true;
        }
        if prepare.is_empty() && self.owns_heap(&current_ty) {
            // A stepless split binds a plain clone of the source; a byte copy
            // of heap-owning storage would double-release at the two drops.
            return Err(self.unsupported(
                format!("borrowed iteration of `{current_ty}` without a preparation step"),
                None,
            ));
        }
        self.mem_copy(
            ctx,
            self.var_slots[dest as usize],
            current,
            dest_layout.size,
            Reg(u32::MAX),
        );
        self.set_drop_flag(ctx, dest, true);
        Ok(())
    }

    /// `HasNext`: the bounded protocol's pure length read — call the
    /// iterator's `__len__` and compare greater-than-zero. The receiver
    /// passes as a plain byte copy (the VM clones its value for the call).
    /// The pack element list of `iter`'s split slot, when the monomorphizer
    /// typed it for the compiler-private pack fallback.
    pub(super) fn pack_iter_elements(&self, iter: u32) -> Option<Vec<Ty>> {
        match self.func.var_tys.get(&iter) {
            Some(Ty::RuntimePack(elements) | Ty::Tuple(elements)) => Some(elements.clone()),
            _ => None,
        }
    }

    /// The backend-side advance position of a pack-fallback iterator slot
    /// (the slot itself keeps the pack layout), created on first use.
    pub(super) fn pack_position_slot(&mut self, ctx: &mut Context, iter: u32) -> Value {
        if let Some(slot) = self.pack_positions.get(&iter) {
            return *slot;
        }
        // A typed slot: mem2reg promotes an alloca at its element type, and
        // a byte-array slot would promote as `i8` under the i64 loads.
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let slot = self.entry_typed_alloca(ctx, i64_handle);
        self.pack_positions.insert(iter, slot);
        slot
    }

    /// Initialize a pack-fallback iterator: position zero, and — for a
    /// distinct destination slot — the pack bytes relocated (a raw move,
    /// the VM's iterator-slot pack copy).
    pub(super) fn lower_pack_iter_init(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        elements: &[Ty],
    ) -> Result<(), PlironError> {
        let position = self.pack_position_slot(ctx, dest);
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, position);
        self.append(ctx, store.get_operation(), None);
        if source != dest {
            let composed = self.struct_layout_of(elements, Reg(u32::MAX))?;
            let from = self.var_slots[source as usize];
            let to = self.var_slots[dest as usize];
            self.mem_copy(ctx, to, from, composed.layout.size, Reg(u32::MAX));
            // The iterator slot receives the pack by relocation. Its source
            // must be inert before later cleanup, especially when elements
            // own buffers or define destructors.
            self.set_drop_flag(ctx, source, false);
            self.set_drop_flag(ctx, dest, true);
        }
        Ok(())
    }

    pub(super) fn lower_has_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
        method: Option<&str>,
    ) -> Result<(), PlironError> {
        let Some(method) = method else {
            // The compiler-private pack fallback: the shadow position
            // against the static element count.
            if let Some(elements) = self.pack_iter_elements(iter) {
                let position_slot = self.pack_position_slot(ctx, iter);
                let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let position = LoadOp::new(ctx, position_slot, i64_handle);
                self.append(ctx, position.get_operation(), Some(dest));
                let count = self.int_constant(ctx, elements.len() as i64);
                let more =
                    ICmpOp::new(ctx, ICmpPredicateAttr::SLT, position.get_result(ctx), count);
                return self.define(ctx, dest, more.get_operation(), more.get_result(ctx));
            }
            return Err(self.unsupported_reg("method-free iterator length read".into(), dest));
        };
        let Some(signature) = self.signatures.get(method) else {
            return Err(
                self.unsupported_reg(format!("iterator length via uncompiled `{method}`"), dest)
            );
        };
        if signature.outcome.is_some() || signature.sret.is_some() {
            return Err(
                self.unsupported_reg(format!("iterator length contract of `{method}`"), dest)
            );
        }
        if signature.ret != RetKind::I64 {
            return Err(self.unsupported_reg(format!("iterator length result of `{method}`"), dest));
        }
        let Some(LowerTy::Aggregate { layout, .. }) = signature.params.first().cloned() else {
            return Err(
                self.unsupported_reg(format!("iterator length receiver of `{method}`"), dest)
            );
        };
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let receiver = self.entry_alloca(ctx, layout.size, layout.align);
        self.mem_copy(
            ctx,
            receiver,
            self.var_slots[iter as usize],
            layout.size,
            dest,
        );
        let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![receiver]);
        self.append(ctx, call.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let has_next = ICmpOp::new(ctx, ICmpPredicateAttr::SGT, call.get_result(ctx), zero);
        self.define(
            ctx,
            dest,
            has_next.get_operation(),
            has_next.get_result(ctx),
        )
    }

    /// `Next`: advance the iterator in place through its non-raising
    /// `__next__(mut self)`. The receiver operand is the iterator variable's
    /// own storage, so the mutation is the write-back; a reference result
    /// binds the returned place pointer, and the `CopyIteratorReference`
    /// adapter reads through it with the VM's lifecycle copy.
    pub(super) fn lower_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
        call: Option<&mojito_checked::checked::CheckedIteratorCall>,
    ) -> Result<(), PlironError> {
        let Some(call) = call else {
            // The compiler-private pack fallback: read the element at the
            // cursor position and advance (the VM's `remove(0)` pop, with
            // the position standing in for the shift).
            if let Some(elements) = self.pack_iter_elements(iter) {
                let Some(first) = elements.first() else {
                    // An empty pack's advance is dead code (`HasNext` is
                    // statically false); define a zeroed destination.
                    match lower_ty(
                        self.name,
                        self.func.reg_types.get(&dest.0).unwrap_or(&Ty::Int),
                        &self.layout,
                        self.reg_span(dest),
                    )? {
                        LowerTy::Scalar(_) => {
                            let zero = self.int_constant(ctx, 0);
                            self.reg_values.insert(dest.0, zero);
                        }
                        LowerTy::Aggregate { layout, .. } => {
                            let storage = self.entry_alloca(ctx, layout.size, layout.align);
                            self.reg_values.insert(dest.0, storage);
                        }
                        LowerTy::ZeroSized => {
                            self.erased.insert(dest.0);
                        }
                    }
                    return Ok(());
                };
                if elements.iter().any(|element| element != first) {
                    return Err(self.unsupported_reg("heterogeneous pack advance".into(), dest));
                }
                let composed = self.struct_layout_of(&elements, dest)?;
                let stride = if elements.len() > 1 {
                    composed.offsets[1] - composed.offsets[0]
                } else {
                    0
                };
                let slot = self.var_slots[iter as usize];
                let position_slot = self.pack_position_slot(ctx, iter);
                let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let position = LoadOp::new(ctx, position_slot, i64_handle);
                self.append(ctx, position.get_operation(), Some(dest));
                self.clear_pack_leaf_flag(ctx, iter, position.get_result(ctx));
                let stride_value = self.int_constant(ctx, stride as i64);
                let scaled = MulOp::new_with_overflow_flag(
                    ctx,
                    position.get_result(ctx),
                    stride_value,
                    no_overflow_flags(),
                );
                self.append(ctx, scaled.get_operation(), Some(dest));
                let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
                let address = GetElementPtrOp::new(
                    ctx,
                    slot,
                    vec![GepIndex::Value(scaled.get_result(ctx))],
                    i8_ty,
                );
                self.append(ctx, address.get_operation(), Some(dest));
                let one = self.int_constant(ctx, 1);
                let next = AddOp::new_with_overflow_flag(
                    ctx,
                    position.get_result(ctx),
                    one,
                    no_overflow_flags(),
                );
                self.append(ctx, next.get_operation(), Some(dest));
                let store = StoreOp::new(ctx, next.get_result(ctx), position_slot);
                self.append(ctx, store.get_operation(), Some(dest));
                let source = address.get_result(ctx);
                return match lower_ty(self.name, first, &self.layout, self.reg_span(dest))? {
                    LowerTy::Scalar(scalar) => {
                        let handle = scalar.handle(ctx);
                        let load = LoadOp::new(ctx, source, handle);
                        self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
                    }
                    LowerTy::Aggregate { ty, layout } => {
                        let storage = self.entry_alloca(ctx, layout.size, layout.align);
                        self.mem_copy(ctx, storage, source, layout.size, dest);
                        self.mem_zero(ctx, source, layout.size);
                        self.reg_values.insert(dest.0, storage);
                        if self.owns_heap(&ty)
                            || self.stdlib_deinit_temp(&ty)
                            || self.needs_drop(&ty)
                        {
                            self.mark_owned_temp(dest, *ty)?;
                        }
                        Ok(())
                    }
                    LowerTy::ZeroSized => {
                        self.erased.insert(dest.0);
                        Ok(())
                    }
                };
            }
            return Err(self.unsupported_reg("method-free iterator advance".into(), dest));
        };
        let signature = self.iterator_next_signature(&call.target, dest)?;
        let receiver = self.var_slots[iter as usize];
        if let Some(outcome) = signature.outcome.clone() {
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            return self.emit_bounded_raising_call(
                ctx,
                dest,
                CallOpCallable::Direct(callee),
                signature.func_ty,
                outcome,
                vec![receiver],
            );
        }
        if call.result_adapter.is_some() && signature.ret == RetKind::Ptr {
            // The abstract call promised a value; the concrete target returns
            // a reference — read through it and lifecycle-copy the element.
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            let call_op = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![receiver]);
            self.append(ctx, call_op.get_operation(), Some(dest));
            let element = call_op.get_result(ctx);
            return match lower_ty(
                self.name,
                &call.result_ty,
                &self.layout,
                self.reg_span(dest),
            )? {
                LowerTy::Scalar(scalar) => {
                    let handle = scalar.handle(ctx);
                    let load = LoadOp::new(ctx, element, handle);
                    self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
                }
                LowerTy::Aggregate { ty, layout } => {
                    self.copy_aggregate(ctx, dest, &ty, layout, element)
                }
                LowerTy::ZeroSized => {
                    self.erased.insert(dest.0);
                    Ok(())
                }
            };
        }
        self.emit_bound_call(ctx, dest, &call.target, vec![receiver])
    }

    /// `TryNext`: advance through the raising `__next__` over the tagged
    /// outcome. The error edge is statically the exhaustion edge — the
    /// checker pins `call.raises == Some(exhaustion)`, so any raise out of
    /// the callee is exactly the caught `StopIteration` — it releases the
    /// caught error's message and zeroes the ok payload, leaving `dest`
    /// inert. `yielded` is the ok-tag comparison.
    pub(super) fn lower_try_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        yielded: Reg,
        iter: u32,
        call: &mojito_checked::checked::CheckedIteratorCall,
    ) -> Result<(), PlironError> {
        let signature = self.iterator_next_signature(&call.target, dest)?;
        let Some(outcome) = signature.outcome.clone() else {
            return Err(self.unsupported_reg(
                format!(
                    "non-raising `__next__` `{}` on the raising path",
                    call.target
                ),
                dest,
            ));
        };
        if outcome.ok_is_reference {
            let mangled = signature.mangled.clone();
            let func_ty = signature.func_ty;
            return self.lower_try_next_reference(
                ctx, dest, yielded, iter, call, &outcome, mangled, func_ty,
            );
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        let receiver = self.var_slots[iter as usize];
        let call_op = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![storage, receiver],
        );
        self.append(ctx, call_op.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let ok_tag = self.tag_constant(ctx, mojito_native::native::rt_abi::MJ_TAG_OK);
        let is_ok = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), ok_tag);
        self.define(ctx, yielded, is_ok.get_operation(), is_ok.get_result(ctx))?;
        self.conditional_values
            .insert(dest.0, is_ok.get_result(ctx));
        let region = self.region.expect("lowering is inside a function");
        let exhausted_block = BasicBlock::new(ctx, None, vec![]);
        exhausted_block.insert_at_back(region, ctx);
        let join_block = BasicBlock::new(ctx, None, vec![]);
        join_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_ok.get_result(ctx),
            join_block,
            vec![],
            exhausted_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        self.current = Some(exhausted_block);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.emit_release_storage(ctx, err_address, &Ty::Error)?;
        let ok_size = match &outcome.ok {
            LowerTy::ZeroSized => 0,
            _ => {
                self.layout
                    .layout_of(&call.result_ty)
                    .map_err(|error| {
                        self.unsupported_reg(format!("iterator element layout ({error})"), dest)
                    })?
                    .size
            }
        };
        if ok_size > 0 {
            let ok_address = self.offset_address(ctx, storage, outcome.ok_offset);
            self.mem_zero(ctx, ok_address, ok_size);
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join_block);
        match outcome.ok {
            LowerTy::Scalar(scalar) => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { .. } => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                // Deliberately not an owned temporary: the following
                // `DefVar` copies the element out, and the zeroed exhausted
                // bytes must never release.
                self.reg_values.insert(dest.0, address);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// `TryNext` over a reference-yielding raising `__next__`: the ok payload
    /// is a place pointer into the iterator's element storage. The ok edge
    /// reads through it and copies the element out (the VM's
    /// `CopyIteratorReference` adapter); the exhausted edge releases the
    /// caught error and leaves zeroed element bytes, like the value form.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_try_next_reference(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        yielded: Reg,
        iter: u32,
        call: &mojito_checked::checked::CheckedIteratorCall,
        outcome: &OutcomeAbi,
        mangled: String,
        func_ty: TypedHandle<FuncType>,
    ) -> Result<(), PlironError> {
        let element = lower_ty(
            self.name,
            &call.result_ty,
            &self.layout,
            self.reg_span(dest),
        )?;
        let mut element_layout = self.layout.layout_of(&call.result_ty).map_err(|error| {
            self.unsupported_reg(format!("iterator element layout ({error})"), dest)
        })?;
        // A `for ref x` contract's temp slot holds the handle, not the
        // element (see `yields_reference` below).
        if call.reference_result.is_some() && call.result_adapter.is_none() {
            element_layout = Layout::new(8, 8);
        }
        let callee: Identifier = mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        let temp = self.entry_alloca(ctx, element_layout.size, element_layout.align);
        let receiver = self.var_slots[iter as usize];
        let call_op = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![storage, receiver],
        );
        self.append(ctx, call_op.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let ok_tag = self.tag_constant(ctx, mojito_native::native::rt_abi::MJ_TAG_OK);
        let is_ok = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), ok_tag);
        self.define(ctx, yielded, is_ok.get_operation(), is_ok.get_result(ctx))?;
        self.conditional_values
            .insert(dest.0, is_ok.get_result(ctx));
        let region = self.region.expect("lowering is inside a function");
        let ok_block = BasicBlock::new(ctx, None, vec![]);
        ok_block.insert_at_back(region, ctx);
        let exhausted_block = BasicBlock::new(ctx, None, vec![]);
        exhausted_block.insert_at_back(region, ctx);
        let join_block = BasicBlock::new(ctx, None, vec![]);
        join_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_ok.get_result(ctx),
            ok_block,
            vec![],
            exhausted_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        // A `for ref x` contract keeps the yielded reference itself (the
        // destination is a handle written through by the loop body); the
        // adapter contract copies the element out.
        let yields_reference = call.reference_result.is_some() && call.result_adapter.is_none();
        self.current = Some(ok_block);
        let ok_address = self.offset_address(ctx, storage, outcome.ok_offset);
        let ptr_handle: TypeHandle = PointerType::get(ctx, 0).into();
        let place = LoadOp::new(ctx, ok_address, ptr_handle);
        self.append(ctx, place.get_operation(), Some(dest));
        if yields_reference {
            let store = StoreOp::new(ctx, place.get_result(ctx), temp);
            self.append(ctx, store.get_operation(), Some(dest));
        } else {
            match &element {
                LowerTy::Scalar(scalar) => {
                    let handle = scalar.handle(ctx);
                    let value = LoadOp::new(ctx, place.get_result(ctx), handle);
                    self.append(ctx, value.get_operation(), Some(dest));
                    let store = StoreOp::new(ctx, value.get_result(ctx), temp);
                    self.append(ctx, store.get_operation(), Some(dest));
                }
                LowerTy::Aggregate { ty, layout } => {
                    self.fork_value_into(ctx, temp, ty, *layout, place.get_result(ctx), dest)?;
                }
                LowerTy::ZeroSized => {}
            }
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(exhausted_block);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.emit_release_storage(ctx, err_address, &Ty::Error)?;
        if element_layout.size > 0 {
            self.mem_zero(ctx, temp, element_layout.size);
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join_block);
        if yields_reference {
            // The handle value (never read on the exhausted edge — the loop
            // has ended). A handle to pointer-typed storage joins
            // `pointer_slot_refs` like `MakeRef`.
            let load = LoadOp::new(ctx, temp, ptr_handle);
            if let Some(reference) = &call.reference_result
                && matches!(*reference.referent, Ty::Pointer { .. })
            {
                self.pointer_slot_refs.insert(dest.0);
            }
            return self.define(ctx, dest, load.get_operation(), load.get_result(ctx));
        }
        match element {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, temp, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { .. } => {
                // Deliberately not an owned temporary: the following `DefVar`
                // copies the element out, and the zeroed exhausted bytes must
                // never release.
                self.reg_values.insert(dest.0, temp);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// The compiled signature of an iterator `__next__` target, requiring the
    /// VM's `mut self` receiver contract.
    pub(super) fn iterator_next_signature(
        &self,
        target: &str,
        dest: Reg,
    ) -> Result<&FnSignature, PlironError> {
        if !matches!(
            self.declarations
                .get(target)
                .and_then(|decl| decl.receiver_convention.as_ref()),
            Some(mojito_ast::ast::ArgConvention::Mut)
        ) {
            return Err(self.unsupported_reg(
                format!("iterator `__next__` `{target}` without a `mut self` receiver"),
                dest,
            ));
        }
        self.signatures
            .get(target)
            .ok_or_else(|| self.unsupported_reg(format!("call to uncompiled `{target}`"), dest))
    }
}
