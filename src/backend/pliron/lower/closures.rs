//! Closure lowering: `MakeClosure`, capture drop thunks, and indirect
//! calls through contract slots.

use super::*;

impl<'a> FnLowering<'a> {
    /// Build the two-word `{ invoke, env }` value of a retained callable:
    /// intern the target's `invoke` thunk and store its address next to the
    /// environment pointer. Bare function values and empty-capture closures
    /// carry a null environment.
    pub(super) fn lower_make_closure(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        function: &str,
        captures: &[MirClosureCapture],
    ) -> Result<(), PlironError> {
        let Some(contract) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped closure result".into(), dest));
        };
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let signatures = self.signatures;
        let Some(target) = signatures.get(function) else {
            return Err(self.unsupported_reg(format!("closure over uncompiled `{function}`"), dest));
        };
        self.check_contract_target(&abi, target, captures.len(), function, dest)?;
        // The environment record: `{ drop: ptr, slots... }`. A `Reference`
        // slot stores the captured place's address; an owned (`copy`/`move`)
        // slot stores the value inline — the record is the stable storage
        // whose address the invoke thunk passes as the capture's reference
        // parameter (in-place mutation across repeated invocations, the
        // VM's owned-capture re-referencing).
        let pointer_slot = Ty::Pointer {
            element: Box::new(Ty::None),
            origin: crate::origin::PointerOrigin::Untracked { mutable: true },
        };
        let mut modes = String::with_capacity(captures.len());
        let mut slot_tys = vec![pointer_slot.clone()];
        for capture in captures {
            let (mode, slot_ty) = match capture.mode {
                MirCaptureMode::Reference => ('r', pointer_slot.clone()),
                MirCaptureMode::Copy | MirCaptureMode::Move => {
                    let Some(ty) = capture
                        .place
                        .ty
                        .clone()
                        .or_else(|| self.func.var_tys.get(&capture.place.root).cloned())
                    else {
                        return Err(self.unsupported_reg("untyped closure capture".into(), dest));
                    };
                    // The VM's owned capture runs the user's copy/move
                    // constructor; the native record relocates or forks
                    // bytes, so a user-observable constructor rejects. The
                    // nominal String's bridged constructors are exactly the
                    // native fork/relocation semantics.
                    let ctor = if capture.mode == MirCaptureMode::Copy {
                        "__copyinit__"
                    } else {
                        "__moveinit__"
                    };
                    if self.chain_runs_user_lifecycle(&ty, ctor) {
                        return Err(self.unsupported_reg(
                            format!("owned closure capture of `{ty}` with a user `{ctor}`"),
                            dest,
                        ));
                    }
                    if capture.mode == MirCaptureMode::Move && !capture.place.proj.is_empty() {
                        // A projected move capture leaves a residual
                        // aggregate whose partial-drop bookkeeping the
                        // leaf-flag pre-scan does not cover here.
                        return Err(
                            self.unsupported_reg("projected move closure capture".into(), dest)
                        );
                    }
                    (
                        if capture.mode == MirCaptureMode::Copy {
                            'c'
                        } else {
                            'm'
                        },
                        ty,
                    )
                }
            };
            modes.push(mode);
            slot_tys.push(slot_ty);
        }
        let (env, capture_offsets) = if captures.is_empty() {
            let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
            let null = ZeroOp::new(ctx, ptr_ty);
            self.append(ctx, null.get_operation(), Some(dest));
            (null.get_result(ctx), Vec::new())
        } else {
            let composed = self.struct_layout_of(&slot_tys, dest)?;
            let record = self.entry_alloca(ctx, composed.layout.size, composed.layout.align);
            // Header: the per-site drop thunk when some owned slot needs
            // drop work, else null. Re-stored on every execution — a loop
            // re-creating a dropped closure revives the tombstoned header.
            let droppable: Vec<(char, Ty, u64)> = modes
                .chars()
                .zip(&slot_tys[1..])
                .zip(&composed.offsets[1..])
                .map(|((mode, ty), offset)| (mode, ty.clone(), *offset))
                .collect();
            let header = if droppable
                .iter()
                .any(|(mode, ty, _)| *mode != 'r' && self.needs_drop(ty))
            {
                let thunk = self.ensure_capture_drop_thunk(ctx, function, &modes, &droppable)?;
                let address = AddressOfOp::new(ctx, thunk, 0);
                self.append(ctx, address.get_operation(), Some(dest));
                address.get_result(ctx)
            } else {
                let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                let null = ZeroOp::new(ctx, ptr_ty);
                self.append(ctx, null.get_operation(), Some(dest));
                null.get_result(ctx)
            };
            let store_header = StoreOp::new(ctx, header, record);
            self.append(ctx, store_header.get_operation(), Some(dest));
            for (capture, ((mode, ty, _), offset)) in captures
                .iter()
                .zip(droppable.iter().zip(&composed.offsets[1..]))
            {
                let slot = if *offset == 0 {
                    record
                } else {
                    self.gep_byte(ctx, record, *offset, dest)
                };
                let place = capture.place.clone();
                let (source, _) = self.place_address(ctx, &place, dest)?;
                if *mode == 'r' {
                    let store = StoreOp::new(ctx, source, slot);
                    self.append(ctx, store.get_operation(), Some(dest));
                    continue;
                }
                let layout = self.layout.layout_of(ty).map_err(|error| {
                    self.unsupported_reg(format!("closure capture layout ({error})"), dest)
                })?;
                if *mode == 'c' && self.owns_heap(ty) {
                    // A copy capture of a borrowed heap owner forks; a byte
                    // copy would alias buffers both owners release.
                    self.fork_value_into(ctx, slot, ty, layout, source, dest)?;
                } else {
                    self.mem_copy(ctx, slot, source, layout.size, dest);
                }
                if *mode == 'm' {
                    // The VM's move capture runs the compiled stdlib
                    // `__moveinit__` (`move_value`), whose `deinit other`
                    // teardown reports one consume event; the byte
                    // relocation above is that constructor's exact
                    // semantics, so mirror the event.
                    if self.trace_lifecycle
                        && let Ty::Struct(name, _) = ty
                        && self
                            .declarations
                            .contains_key(&format!("{name}.__moveinit__"))
                    {
                        let name = name.clone();
                        self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_CONSUME, &name);
                    }
                    // A whole-root move capture: ownership analysis already
                    // suppressed the source's ordinary drop; clearing the
                    // flag mirrors the VM's tombstoned source.
                    self.set_drop_flag(ctx, place.root, false);
                }
            }
            (record, composed.offsets[1..].to_vec())
        };
        let thunk = self
            .shared
            .ensure_thunk(ctx, target, &modes, &capture_offsets);
        let storage = self.entry_alloca(ctx, 16, 8);
        let invoke = AddressOfOp::new(ctx, thunk, 0);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let store_invoke = StoreOp::new(ctx, invoke.get_result(ctx), storage);
        self.append(ctx, store_invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, storage, 8, dest);
        let store_env = StoreOp::new(ctx, env, env_address);
        self.append(ctx, store_env.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Emit (once per `(target, modes)`) the teardown thunk a capture
    /// record's header names: destroy the owned droppable slots in reverse
    /// capture order (the VM's closure-drop order), then null the header —
    /// drops are idempotent per record, which is what keeps aliasing
    /// two-word copies sound.
    pub(super) fn ensure_capture_drop_thunk(
        &mut self,
        ctx: &mut Context,
        function: &str,
        modes: &str,
        slots: &[(char, Ty, u64)],
    ) -> Result<Identifier, PlironError> {
        let key = (function.to_string(), modes.to_string());
        if let Some(name) = self.shared.drop_thunks.get(&key) {
            return Ok(name.clone());
        }
        let name: Identifier = format!("mjdrop_{}", self.shared.drop_thunks.len())
            .try_into()
            .expect("thunk names are identifier-safe");
        let void = VoidType::get(ctx).to_handle();
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let thunk_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
        let func = FuncOp::new(ctx, name.clone(), thunk_ty);
        self.shared
            .module
            .append_operation(ctx, func.get_operation(), 0);
        let entry = func.get_or_create_entry_block(ctx);
        let region = func
            .get_operation()
            .deref(ctx)
            .regions()
            .next()
            .expect("llvm.func has a body region");
        // Retarget the emission cursor into the thunk body: the drop chain
        // is ordinary `emit_drop_value` output (trace events included, so
        // capture drops report at drop time like the VM's).
        let saved_current = self.current;
        let saved_region = self.region;
        self.current = Some(entry);
        self.region = Some(region);
        let emit = |lowering: &mut Self, ctx: &mut Context| -> Result<(), PlironError> {
            let env = entry.deref(ctx).get_argument(0);
            for (mode, ty, offset) in slots.iter().rev() {
                if *mode == 'r' || !lowering.needs_drop(ty) {
                    continue;
                }
                let address = if *offset == 0 {
                    env
                } else {
                    lowering.gep_byte_unspanned(ctx, env, *offset)
                };
                lowering.emit_drop_value(ctx, address, ty, false)?;
            }
            let null = ZeroOp::new(ctx, ptr_ty);
            lowering.append(ctx, null.get_operation(), None);
            let tombstone = StoreOp::new(ctx, null.get_result(ctx), env);
            lowering.append(ctx, tombstone.get_operation(), None);
            let ret = ReturnOp::new(ctx, None);
            lowering.append(ctx, ret.get_operation(), None);
            Ok(())
        };
        let emitted = emit(self, ctx);
        self.current = saved_current;
        self.region = saved_region;
        emitted?;
        self.shared.drop_thunks.insert(key, name.clone());
        Ok(name)
    }

    /// Call through a retained callable value: bind the arguments against
    /// the callee register's checked contract, load `{ invoke, env }`, and
    /// call `invoke` indirectly with the environment pointer prepended
    /// (after the outcome/sret out-pointer, when one exists).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_call_indirect(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callee: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
        instantiated_contract: Option<&Ty>,
    ) -> Result<(), PlironError> {
        let contract = instantiated_contract
            .cloned()
            .or_else(|| self.func.reg_types.get(&callee.0).cloned());
        let Some(mut contract) = contract else {
            return Err(self.unsupported_reg("untyped indirect callee".into(), dest));
        };
        while let Ty::Ref(reference) = contract {
            contract = *reference.referent;
        }
        if let Ty::Struct(name, _) = &contract {
            // Monomorphization devirtualizes nominal callables into direct
            // `__call__` method calls; one that survives to lowering is a
            // shape it could not rewrite.
            return Err(self.unsupported_reg(
                format!("indirect call through nominal callable `{name}`"),
                dest,
            ));
        }
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let bound =
            self.bind_contract_slots(ctx, dest, &abi, args, kwargs, arg_places, kwarg_places)?;
        let base = self.reg_ptr(ctx, callee)?;
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let invoke = LoadOp::new(ctx, base, ptr_ty);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, base, 8, dest);
        let env = LoadOp::new(ctx, env_address, ptr_ty);
        self.append(ctx, env.get_operation(), Some(dest));
        let mut operands = Vec::with_capacity(bound.len() + 1);
        operands.push(env.get_result(ctx));
        operands.extend(bound);
        self.emit_call_shaped(
            ctx,
            dest,
            CallOpCallable::Indirect(invoke.get_result(ctx)),
            abi.func_ty,
            abi.returns_value,
            abi.sret,
            abi.outcome.clone(),
            operands,
        )
    }

    /// Resolve an indirect call's arguments into the contract's positional
    /// parameter order via `call::match_call_slots` — the same structural
    /// binding as `bind_call_slots`, off the `Ty::Func` contract instead of
    /// a compiled declaration. Defaults reject: the VM binds an omitted
    /// argument from the runtime callee's declaration, which the native
    /// caller cannot see behind the thunk.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_contract_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        abi: &ContractAbi,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<Vec<Value>, PlironError> {
        let kw_names: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let matched = match_call_slots(
            &abi.names,
            &abi.required,
            abi.positional_only,
            abi.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: abi.kw_pack_index.is_some(),
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("indirect-call binding failed: {error:?}"), dest)
        })?;
        if matched.slots.len() + usize::from(abi.kw_pack_index.is_some()) != abi.params.len() {
            return Err(self.unsupported_reg(
                "indirect-call binding disagrees with the contract arity".into(),
                dest,
            ));
        }
        let mut lowered = Vec::with_capacity(abi.params.len());
        let mut slots = matched.slots.iter();
        for (index, expected) in abi.params.iter().enumerate() {
            if abi.kw_pack_index == Some(index) {
                let LowerTy::Aggregate { ty, layout } = expected else {
                    return Err(self.unsupported_reg(
                        "indirect keyword pack lacks aggregate storage".into(),
                        dest,
                    ));
                };
                let Ty::Struct(struct_name, _) = ty.as_ref() else {
                    return Err(self.unsupported_reg(
                        "indirect keyword pack is not a StringDict".into(),
                        dest,
                    ));
                };
                let Some(struct_decl) = self.struct_decls.get(struct_name.as_str()) else {
                    return Err(self.unsupported_reg(
                        "indirect keyword pack lacks a struct declaration".into(),
                        dest,
                    ));
                };
                let field_types = struct_decl
                    .fields
                    .iter()
                    .map(|(_, ty)| ty.clone())
                    .collect::<Vec<_>>();
                let composed = self.struct_layout_of(&field_types, dest)?;
                let count_index = struct_decl
                    .fields
                    .iter()
                    .position(|(field, _)| field == "count")
                    .ok_or_else(|| {
                        self.unsupported_reg("indirect keyword pack lacks `count`".into(), dest)
                    })?;
                let storage = self.entry_alloca(ctx, layout.size.max(1), layout.align.max(1));
                self.mem_zero(ctx, storage, layout.size);
                let count_address =
                    self.gep_byte(ctx, storage, composed.offsets[count_index], dest);
                let count = self.int_constant(ctx, matched.keyword_overflow.len() as i64);
                let store = StoreOp::new(ctx, count, count_address);
                self.append(ctx, store.get_operation(), Some(dest));
                lowered.push(storage);
                continue;
            }
            let slot = slots.next().expect("matched named indirect slot");
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let expected = expected.clone();
            let owned = abi.owned_params.get(index).copied().unwrap_or(false);
            let by_ref = abi.ref_params.get(index).copied().unwrap_or(false);
            let place_address = |lowering: &mut Self,
                                 ctx: &mut Context,
                                 place: Option<&MirPlace>|
             -> Result<Value, PlironError> {
                let Some(place) = place.cloned() else {
                    return Err(lowering.unsupported_reg(
                        "`mut`/`ref` indirect argument without a place".into(),
                        dest,
                    ));
                };
                Ok(lowering.place_address(ctx, &place, dest)?.0)
            };
            let value = match slot {
                ArgSlot::Positional(p) if by_ref => {
                    place_address(self, ctx, arg_places.get(*p).and_then(Option::as_ref))?
                }
                ArgSlot::Keyword(k) if by_ref => {
                    place_address(self, ctx, kwarg_places.get(*k).and_then(Option::as_ref))?
                }
                ArgSlot::Positional(p) => self.arg_value(ctx, args[*p], &expected, owned, dest)?,
                ArgSlot::Keyword(k) => self.arg_value(ctx, kwargs[*k].1, &expected, owned, dest)?,
                ArgSlot::Default => {
                    return Err(self.unsupported_reg(
                        "defaulted argument at an indirect call site".into(),
                        dest,
                    ));
                }
            };
            lowered.push(value);
        }
        Ok(lowered)
    }

    /// Materialize a constant default at the parameter's scalar type, exactly
    /// as the VM's default binding materializes the literal.
    pub(super) fn checked_const_value(
        &mut self,
        ctx: &mut Context,
        value: &CheckedConst,
        expected: ScalarTy,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        match (value, expected) {
            (CheckedConst::Int(literal), _) => {
                let literal = PendingLiteral::Int(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Float(literal), _) => {
                let literal = PendingLiteral::Float(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Bool(value), ScalarTy::Bool) => Ok(self.bool_constant(ctx, *value)),
            (CheckedConst::Bool(_), other) => Err(self.unsupported_reg(
                format!("Bool default argument for a `{}` parameter", other.name()),
                dest,
            )),
            (CheckedConst::String(_) | CheckedConst::None, _) => {
                Err(self.unsupported_reg("non-scalar default argument".into(), dest))
            }
            // A converting-constructor default (e.g. `Optional[T] = None`) runs
            // the constructor to build a heap-backed aggregate — the VM oracle
            // supports it, native default-fill does not yet.
            (CheckedConst::Construct { .. }, _) => Err(self.unsupported_reg(
                "converting-constructor default argument is not yet lowered natively".into(),
                dest,
            )),
        }
    }
}
