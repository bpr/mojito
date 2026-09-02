//! The per-function driver: block walk plus the [`MirInstr`] dispatch.

use super::*;

impl<'a> FnLowering<'a> {
    pub(super) fn run(&mut self, ctx: &mut Context, func_op: FuncOp) -> Result<(), PlironError> {
        let entry = func_op.get_or_create_entry_block(ctx);
        let region = func_op
            .get_operation()
            .deref(ctx)
            .regions()
            .next()
            .expect("llvm.func has a body region");
        self.region = Some(region);
        self.entry = Some(entry);

        // One pliron block per MIR block (entry stays separate so MIR block 0
        // may have predecessors).
        for _ in 0..self.func.blocks.len() {
            let block = BasicBlock::new(ctx, None, vec![]);
            block.insert_at_back(region, ctx);
            self.blocks.push(block);
        }
        self.function_blocks = self.blocks.clone();
        self.next_region_block = self.func.blocks.len();

        // Entry: one alloca per variable slot, parameter stores, then a jump
        // to MIR block 0. An aggregate-returning function receives its sret
        // out-pointer as argument 0, shifting every parameter right by one;
        // aggregate parameter slots alias the incoming pointer directly
        // (write-through — `out`/`mut` receivers mutate caller storage), so
        // they allocate nothing.
        self.current = Some(entry);
        let signature = &self.signatures[self.name];
        let arg_offset = usize::from(signature.sret.is_some() || signature.outcome.is_some());
        if signature.outcome.is_some() {
            self.outcome_ptr = Some(entry.deref(ctx).get_argument(0));
        } else if signature.sret.is_some() {
            self.sret_ptr = Some(entry.deref(ctx).get_argument(0));
        }
        let param_tys: Vec<Option<LowerTy>> = (0..self.func.n_vars)
            .map(|var| {
                (var < self.func.n_params).then(|| self.signatures[self.name].params[var].clone())
            })
            .collect();
        let ref_params = self.signatures[self.name].ref_params.clone();
        let param_by_pointer = |var: usize, param_ty: &Option<LowerTy>| {
            matches!(param_ty, Some(LowerTy::Aggregate { .. }))
                || (param_ty.is_some() && ref_params.get(var).copied().unwrap_or(false))
        };
        let one = self.int_constant(ctx, 1);
        // Zero-sized parameters have no physical argument; later parameters'
        // argument indexes shift left past them.
        let physical_index = |var: usize| {
            arg_offset
                + param_tys[..var]
                    .iter()
                    .filter(|ty| !matches!(ty, Some(LowerTy::ZeroSized)))
                    .count()
        };
        for (var, param_ty) in param_tys.iter().enumerate() {
            match param_ty {
                // A zero-sized parameter's slot is never read (its uses
                // erase); a null placeholder keeps the slot indexes aligned.
                Some(LowerTy::ZeroSized) => {
                    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                    let null = ZeroOp::new(ctx, ptr_ty);
                    self.append(ctx, null.get_operation(), None);
                    self.var_slots.push(null.get_result(ctx));
                }
                // Aggregate and `mut`/`ref` parameter slots alias the
                // incoming pointer (write-through).
                _ if param_by_pointer(var, param_ty) => {
                    let incoming = entry.deref(ctx).get_argument(physical_index(var));
                    self.var_slots.push(incoming);
                }
                _ => {
                    let slot = match self.var_lower_ty(var as u32)? {
                        LowerTy::Scalar(scalar) => {
                            let handle = scalar.handle(ctx);
                            let alloca = AllocaOp::new(ctx, handle, one);
                            self.append(ctx, alloca.get_operation(), None);
                            alloca.get_result(ctx)
                        }
                        LowerTy::Aggregate { layout, .. } => {
                            self.entry_alloca(ctx, layout.size, layout.align)
                        }
                        LowerTy::ZeroSized => {
                            return Err(self.unsupported(
                                format!(
                                    "zero-sized variable `{}`",
                                    self.func
                                        .var_names
                                        .get(var)
                                        .map(String::as_str)
                                        .unwrap_or("?")
                                ),
                                None,
                            ));
                        }
                    };
                    self.var_slots.push(slot);
                }
            }
        }
        for (param, param_ty) in param_tys.iter().take(self.func.n_params).enumerate() {
            if param_by_pointer(param, param_ty) || matches!(param_ty, Some(LowerTy::ZeroSized)) {
                continue;
            }
            let value = entry.deref(ctx).get_argument(physical_index(param));
            let store = StoreOp::new(ctx, value, self.var_slots[param]);
            self.append(ctx, store.get_operation(), None);
        }
        self.initialized_vars
            .extend((0..self.func.n_params).map(|var| var as u32));
        // Every droppable variable gets an initialization flag (parameters
        // arrive bound, everything else starts empty). See `drop_flags`.
        let i1: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        for var in 0..self.func.n_vars {
            let LowerTy::Aggregate { ty, layout } = self.var_lower_ty(var as u32)? else {
                continue;
            };
            if !self.needs_drop(&ty) {
                continue;
            }
            // Local droppable storage zeroes at entry (parameters alias
            // caller storage and arrive initialized).
            if var >= self.func.n_params {
                let slot = self.var_slots[var];
                self.mem_zero(ctx, slot, layout.size);
            }
            let alloca = AllocaOp::new(ctx, i1, one);
            self.append(ctx, alloca.get_operation(), None);
            let init = self.bool_constant(ctx, var < self.func.n_params);
            let store = StoreOp::new(ctx, init, alloca.get_result(ctx));
            self.append(ctx, store.get_operation(), None);
            self.drop_flags.insert(var as u32, alloca.get_result(ctx));
        }
        // Depth-1 projected moves leave a variable partially initialized;
        // give each moved top-level leaf a presence flag so later drops and
        // consumption destroy exactly the surviving leaves.
        let mut move_places = Vec::new();
        collect_projected_move_places(&self.func.blocks, &mut move_places);
        let mut leaf_targets: Vec<(u32, usize)> = move_places
            .iter()
            .filter_map(|place| self.leaf_position(place).map(|pos| (place.root, pos)))
            .collect();
        for (var, ty) in &self.func.var_tys {
            if let Ty::Tuple(elements) | Ty::RuntimePack(elements) = ty {
                leaf_targets.extend(
                    elements
                        .iter()
                        .enumerate()
                        .filter(|(_, element)| self.needs_drop(element))
                        .map(|(position, _)| (*var, position)),
                );
            }
        }
        for (var, position) in leaf_targets {
            let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(var)? else {
                continue;
            };
            if !self.needs_drop(&ty) {
                continue;
            }
            if self
                .leaf_flags
                .get(&var)
                .is_some_and(|leaves| leaves.contains_key(&position))
            {
                continue;
            }
            let alloca = AllocaOp::new(ctx, i1, one);
            self.append(ctx, alloca.get_operation(), None);
            let init = self.bool_constant(ctx, true);
            let store = StoreOp::new(ctx, init, alloca.get_result(ctx));
            self.append(ctx, store.get_operation(), None);
            self.leaf_flags
                .entry(var)
                .or_default()
                .insert(position, alloca.get_result(ctx));
        }
        let jump = BrOp::new(ctx, self.blocks[0], vec![]);
        self.append(ctx, jump.get_operation(), None);

        // Final operand appearances drive the owned-temporary releases. The
        // walk recurses into `try` regions with synthetic block ids assigned
        // in exactly the order region lowering assigns them.
        let function_ids: Vec<usize> = (0..self.func.blocks.len()).collect();
        let mut next_id = self.func.blocks.len();
        record_last_uses(
            &mut self.last_uses,
            self.func.blocks.as_slice(),
            &function_ids,
            &mut next_id,
        );

        for (id, block) in self.func.blocks.iter().enumerate() {
            self.current = Some(self.blocks[id]);
            for (index, instr) in block.instrs.iter().enumerate() {
                self.position = (id, index);
                self.lower_instr(ctx, instr)?;
                self.flush_owned_temps(ctx)?;
            }
            self.position = (id, usize::MAX);
            self.lower_term(ctx, &block.term)?;
        }
        Ok(())
    }

    pub(super) fn lower_instr(
        &mut self,
        ctx: &mut Context,
        instr: &MirInstr,
    ) -> Result<(), PlironError> {
        match instr {
            MirInstr::Const { dest, k } => self.lower_const(ctx, *dest, k),
            MirInstr::ConstructTypeParam { dest, param } => Err(self.unsupported_reg(
                format!("constructing type parameter `{param}` after monomorphization"),
                *dest,
            )),
            MirInstr::SizeOf { dest, ty } => {
                let size = self
                    .layout
                    .layout_of(ty)
                    .map_err(|error| self.unsupported_reg(error.to_string(), *dest))?
                    .size;
                self.lower_const(ctx, *dest, &crate::mir::Const::Int(size as i64))
            }
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => self.lower_materialize(ctx, *dest, *value, target),
            MirInstr::UnOp { op, dest, a } => self.lower_unop(ctx, *op, *dest, *a),
            MirInstr::BinOp {
                op,
                dest,
                a,
                b,
                resolved,
            } => self.lower_binop(ctx, *op, *dest, *a, *b, resolved.as_deref()),
            MirInstr::UseVar { dest, var, mode } => self.lower_use_var(ctx, *dest, *var, *mode),
            MirInstr::DefVar { var, src, .. } => self.lower_def_var(ctx, *var, *src),
            MirInstr::LoadPlace { dest, place } => {
                if self.aliased_receiver_regs.contains(&dest.0) {
                    self.erased.insert(dest.0);
                    return Ok(());
                }
                let (address, ty) = self.place_address(ctx, place, *dest)?;
                self.load_from(ctx, address, &ty, *dest)
            }
            MirInstr::MovePlace { dest, place } => {
                // Moving out of a projection leaves the variable partially
                // initialized. A tracked top-level leaf clears its presence
                // flag (later drops skip it, like the VM's `Value::Moved`
                // tombstone); anything deeper records the blanket marker so
                // a whole-variable drop refuses destructor work instead of
                // double-freeing.
                if !place.proj.is_empty() {
                    let flagged = self.leaf_position(place).and_then(|position| {
                        self.leaf_flags
                            .get(&place.root)
                            .and_then(|leaves| leaves.get(&position))
                            .copied()
                    });
                    match flagged {
                        Some(flag) => {
                            let absent = self.bool_constant(ctx, false);
                            let store = StoreOp::new(ctx, absent, flag);
                            self.append(ctx, store.get_operation(), None);
                        }
                        None => {
                            self.partially_moved.insert(place.root);
                        }
                    }
                }
                let (address, ty) = self.place_address(ctx, place, *dest)?;
                // A move relocates the bytes — ownership transfers to the
                // destination (the VM's `mem::replace`; no clone runs), so
                // the moved value is an owned temporary until consumed.
                match lower_ty(self.name, &ty, &self.layout, self.reg_span(*dest))? {
                    LowerTy::Scalar(scalar) => {
                        let handle = scalar.handle(ctx);
                        let load = LoadOp::new(ctx, address, handle);
                        self.define(ctx, *dest, load.get_operation(), load.get_result(ctx))
                    }
                    LowerTy::Aggregate { ty, layout } => {
                        let storage = self.entry_alloca(ctx, layout.size, layout.align);
                        self.mem_copy(ctx, storage, address, layout.size, *dest);
                        self.reg_values.insert(dest.0, storage);
                        if self.owns_heap(&ty)
                            || self.stdlib_deinit_temp(&ty)
                            || self.needs_drop(&ty)
                        {
                            self.mark_owned_temp(*dest, (*ty).clone())?;
                        }
                        Ok(())
                    }
                    LowerTy::ZeroSized => {
                        self.erased.insert(dest.0);
                        Ok(())
                    }
                }
            }
            MirInstr::Store { place, src } => {
                // The VM overwrites the designated storage without dropping
                // the old value (drop elaboration emits explicit drops), so a
                // plain store/copy is exact.
                let (address, ty) = self.place_address(ctx, place, *src)?;
                self.store_to(ctx, address, &ty, *src)?;
                if matches!(place.proj.last(), Some(Proj::UninitPayload)) {
                    let mut storage_place = place.clone();
                    storage_place.proj.pop();
                    let (storage, _) = self.place_address(ctx, &storage_place, *src)?;
                    let present = self.bool_constant(ctx, true);
                    let flag_store = StoreOp::new(ctx, present, storage);
                    self.append(ctx, flag_store.get_operation(), Some(*src));
                }
                // A whole-variable store (re)initializes the slot; a store
                // into a tracked leaf restores that leaf's presence.
                if place.proj.is_empty() && place.through.is_none() {
                    self.set_drop_flag(ctx, place.root, true);
                } else if let Some(position) = self.leaf_position(place)
                    && let Some(&flag) = self
                        .leaf_flags
                        .get(&place.root)
                        .and_then(|leaves| leaves.get(&position))
                {
                    let present = self.bool_constant(ctx, true);
                    let store = StoreOp::new(ctx, present, flag);
                    self.append(ctx, store.get_operation(), None);
                }
                Ok(())
            }
            MirInstr::GetField { dest, base, field } => {
                self.lower_get_field(ctx, *dest, *base, field)
            }
            MirInstr::MakeTuple {
                dest,
                elems,
                element_types,
            } => self.lower_make_tuple(ctx, *dest, elems, element_types.as_deref()),
            MirInstr::MethodCall {
                dest,
                recv,
                method,
                resolved,
                raises,
                reference_result,
                result_adapter,
                args,
                kwargs,
                recv_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                ..
            } => {
                // The callee's compiled signature is authoritative for the
                // raising and reference-result ABI.
                let _ = (raises, reference_result);
                if result_adapter.is_some() {
                    return Err(
                        self.unsupported_reg("reference-result method adapter".into(), *dest)
                    );
                }
                // Capture accesses are static ownership facts execution
                // erases. Erased type-parameter slots (`value: None`) carry
                // no runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                let _ = capture_accesses;
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) {
                    return Err(self.unsupported_reg(
                        format!("non-positional method contract for `{method}`"),
                        *dest,
                    ));
                }
                self.lower_method_call(
                    ctx,
                    *dest,
                    *recv,
                    method,
                    resolved.as_deref(),
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    recv_place.as_ref(),
                )
            }
            MirInstr::Call {
                dest,
                func,
                raises,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
            } => {
                // The callee's compiled signature is authoritative for the
                // raising ABI.
                let _ = raises;
                // Capture accesses are static ownership facts execution
                // erases. Erased type-parameter slots (`value: None`) carry
                // no runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                let _ = capture_accesses;
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                self.lower_call(ctx, *dest, &func.0, args, kwargs, arg_places, kwarg_places)
            }
            MirInstr::DropVar { var } => self.lower_drop_var(ctx, *var),
            MirInstr::ConsumeVar { var } => self.lower_consume_var(ctx, *var, false),
            MirInstr::ConsumePlace { place, marker } => {
                // Consumption skips the whole-value destructor and destroys
                // only residual fields — a no-op unless fields carry their
                // own destructor work.
                let ty = place
                    .ty
                    .clone()
                    .or_else(|| place.root_ty.clone())
                    .ok_or_else(|| self.unsupported("untyped consumed place".into(), None))?;
                if self.fields_need_drop(&ty) {
                    return Err(
                        self.unsupported("place consumption with droppable fields".into(), None)
                    );
                }
                self.erased.insert(marker.0);
                Ok(())
            }
            MirInstr::InvalidateInteriors { marker, .. } => {
                self.erased.insert(marker.0);
                Ok(())
            }
            MirInstr::EstablishLoans {
                reference,
                loans,
                marker,
                ..
            } => self.lower_establish_loans(ctx, *reference, loans, *marker),
            MirInstr::KeepAlive { .. } => Ok(()),
            MirInstr::CopyValue { dest, value } => self.lower_copy_value(ctx, *dest, *value),
            // Pointer subscripts are runtime intrinsics; every other
            // subscript form routes through nominal `__getitem__` calls the
            // subset does not lower yet.
            MirInstr::Index {
                dest,
                base,
                index,
                intrinsic: Some(crate::mir::MirIntrinsicSubscript::Pointer),
                ..
            } => self.lower_pointer_index(ctx, *dest, *base, *index),
            // References are place addresses: ownership verified the
            // discipline; the backend materializes plain pointers.
            MirInstr::MakeRef { dest, place } => self.lower_make_ref(ctx, *dest, place),
            MirInstr::ReadRef { dest, reference } => self.lower_read_ref(ctx, *dest, *reference),
            MirInstr::WriteRef { reference, value } => {
                self.lower_write_ref(ctx, *reference, *value)
            }
            MirInstr::StoreRef { place, reference } => {
                let place = place.clone();
                let (address, _) = self.place_address(ctx, &place, *reference)?;
                let handle = self.reg_value(ctx, *reference, ScalarTy::Ptr)?;
                let store = StoreOp::new(ctx, handle, address);
                self.append(ctx, store.get_operation(), Some(*reference));
                Ok(())
            }
            // Everything below is outside the supported subset. Every variant
            // is named so that new instructions force a decision here.
            MirInstr::HasNext { dest, iter, method } => {
                self.lower_has_next(ctx, *dest, *iter, method.as_deref())
            }
            MirInstr::Next { dest, iter, call } => {
                self.lower_next(ctx, *dest, *iter, call.as_ref())
            }
            MirInstr::TryNext {
                dest,
                yielded,
                iter,
                call,
                exhaustion: _,
            } => self.lower_try_next(ctx, *dest, *yielded, *iter, call),
            MirInstr::PointerStorageTake {
                dest,
                pointer,
                index,
                element,
            } => self.lower_pointer_storage_take(ctx, *dest, *pointer, *index, element),
            MirInstr::PointerStorageDestroy {
                dest,
                pointer,
                index,
                element,
            } => self.lower_pointer_storage_destroy(ctx, *dest, *pointer, *index, element),
            MirInstr::UninitStorage { dest, init } => self.lower_uninit_storage(ctx, *dest, *init),
            MirInstr::UninitStorageTake {
                dest,
                storage,
                element,
            } => self.lower_uninit_storage_take(ctx, *dest, *storage, element),
            MirInstr::UninitStorageDestroy {
                dest,
                storage,
                element,
            } => self.lower_uninit_storage_destroy(ctx, *dest, *storage, element),
            MirInstr::Index {
                dest,
                base,
                index,
                base_place,
                index_place,
                call: Some(call),
                intrinsic: _,
            } => {
                // A parameterless specialized accessor (a Tuple element
                // getter) takes only `self`; an overloaded `__getitem__`
                // receives the runtime index — the VM's `call.arguments`
                // distinction.
                let positional = if call.arguments.is_empty() {
                    Vec::new()
                } else {
                    vec![SubscriptActual::Reg(*index, index_place.as_ref())]
                };
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *base,
                    base_place.as_ref(),
                    &positional,
                    &[],
                )
            }
            MirInstr::Index {
                dest,
                base,
                index,
                intrinsic: Some(intrinsic),
                ..
            } => self.lower_index_intrinsic(ctx, *dest, *base, *index, intrinsic),
            MirInstr::Slice {
                dest,
                object,
                lower,
                upper,
                step,
                object_place,
                call: Some(call),
                ..
            } => {
                let descriptor = self.build_slice_descriptor(ctx, *dest, *lower, *upper, *step)?;
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *object,
                    object_place.as_ref(),
                    &[SubscriptActual::Descriptor(descriptor)],
                    &[],
                )
            }
            MirInstr::MultiIndex {
                dest,
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call: Some(call),
            } => {
                let positional = self.subscript_actuals(ctx, *dest, args, arg_places)?;
                let mut keywords = Vec::with_capacity(kwargs.len());
                for (i, (name, arg)) in kwargs.iter().enumerate() {
                    let actual = self.subscript_actual(
                        ctx,
                        *dest,
                        arg,
                        kwarg_places.get(i).and_then(Option::as_ref),
                    )?;
                    keywords.push((name.as_str(), actual));
                }
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *object,
                    object_place.as_ref(),
                    &positional,
                    &keywords,
                )
            }
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                value,
                value_place,
                value_keyword,
                call,
            } => {
                // The discarded `__setitem__` result binds to a scratch
                // register outside the function's register space.
                let scratch = Reg(u32::MAX);
                let mut positional = self.subscript_actuals(ctx, scratch, args, arg_places)?;
                let mut keywords = Vec::new();
                if *value_keyword {
                    keywords.push(("value", SubscriptActual::Reg(*value, value_place.as_ref())));
                } else {
                    positional.push(SubscriptActual::Reg(*value, value_place.as_ref()));
                }
                self.lower_subscript_call(
                    ctx,
                    scratch,
                    "__setitem__",
                    call,
                    *receiver,
                    receiver_place.as_ref(),
                    &positional,
                    &keywords,
                )
            }
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
            } => self.lower_make_closure(ctx, *dest, function, captures),
            MirInstr::CallIndirect {
                dest,
                callee,
                resolved,
                raises,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls,
                instantiated_contract,
                instantiated_args,
            } => {
                // The contract is authoritative for the raising ABI; the
                // checker-selected nominal target is consumed by
                // monomorphization's devirtualization; capture accesses are
                // static facts execution erases; the callable value itself
                // needs no stable storage natively — its environment record
                // is the stable storage.
                let _ = (
                    resolved,
                    raises,
                    callee_place,
                    capture_accesses,
                    instantiated_args,
                );
                // A generic-callable contract that still carries value
                // parameter arguments or unresolved declarations at lowering
                // is outside the monomorphized subset.
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) || !param_decls.is_empty() {
                    return Err(
                        self.unsupported_reg("generic callable value invocation".into(), *dest)
                    );
                }
                self.lower_call_indirect(
                    ctx,
                    *dest,
                    *callee,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    instantiated_contract.as_ref(),
                )
            }
            MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value,
            } => self.lower_make_variant(ctx, *dest, alternatives, *index, *value),
            MirInstr::VariantIs {
                dest,
                variant,
                index,
            } => self.lower_variant_is(ctx, *dest, *variant, *index),
            MirInstr::VariantGet {
                dest,
                variant,
                index,
            } => self.lower_variant_get(ctx, *dest, *variant, *index, true),
            MirInstr::VariantTake {
                dest,
                variant,
                index,
                checked,
            } => self.lower_variant_take(ctx, *dest, *variant, *index, *checked),
            MirInstr::VariantReplace {
                dest,
                place,
                input_index,
                output_index,
                value,
                checked,
            } => self.lower_variant_replace(
                ctx,
                *dest,
                place,
                *input_index,
                *output_index,
                *value,
                *checked,
            ),
            MirInstr::Index { dest, .. }
            | MirInstr::Slice { dest, .. }
            | MirInstr::MultiIndex { dest, .. } => {
                Err(self.unsupported_reg(format!("instruction `{}`", instr_name(instr)), *dest))
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => self.lower_make_simd(ctx, *dest, *dtype, *width, elems),
            MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
            } => self.lower_simd_cast(ctx, *dest, *value, *dtype, *width),
            MirInstr::SimdShuffle { dest, value, mask } => {
                self.lower_simd_shuffle(ctx, *dest, *value, mask)
            }
            MirInstr::Raise { src } => self.lower_raise(ctx, *src),
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                cleanup,
            } => self.lower_try(
                ctx,
                body,
                handler.as_ref(),
                orelse.as_deref(),
                finalbody.as_deref(),
                cleanup,
            ),
            MirInstr::GetIter {
                source,
                dest,
                mode: _,
                prepare,
            } => self.lower_get_iter(ctx, *source, *dest, prepare),
            MirInstr::VariantSet {
                dest,
                place,
                index,
                value,
            } => self.lower_variant_set(ctx, *dest, place, *index, *value),
            MirInstr::VariantSetInitWith {
                dest,
                place,
                index,
                factory,
            } => self.lower_variant_set_init_with(ctx, *dest, place, *index, *factory),
            MirInstr::VariantDeinitWith {
                dest,
                variant,
                handler,
                index,
            } => self.lower_variant_deinit_with(ctx, *dest, *variant, *handler, *index),
            MirInstr::Drop { .. } => {
                Err(self.unsupported(format!("instruction `{}`", instr_name(instr)), None))
            }
            MirInstr::Unsupported(message) => {
                Err(self.unsupported(format!("lowering-marked construct: {message}"), None))
            }
        }
    }
}
