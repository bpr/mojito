//! Drop elaboration lowering: drop/consume vars, drop flags, pack leaf
//! flags, and recursive value drops.

use super::*;

impl<'a> FnLowering<'a> {
    /// `DropVar` — the VM's `drop_value`: nothing for scalars; for aggregates
    /// run the compiled `__deinit__` when defined, then destroy fields in
    /// reverse declaration order. Combinations whose residual state is only
    /// dynamically knowable (a destructor plus independently droppable
    /// fields, or a partially-moved variable) reject instead of guessing.
    pub(super) fn lower_drop_var(
        &mut self,
        ctx: &mut Context,
        var: u32,
    ) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                if !self.needs_drop(&ty) {
                    return Ok(());
                }
                if self.partially_moved.contains(&var) {
                    return Err(self.unsupported(
                        format!(
                            "drop of partially-moved variable `{}`",
                            self.func
                                .var_names
                                .get(var as usize)
                                .map(String::as_str)
                                .unwrap_or("?")
                        ),
                        None,
                    ));
                }
                let ptr = self.var_slots[var as usize];
                let flag = self.drop_flags.get(&var).copied();
                if let Some(leaves) = self.leaf_flags.get(&var).cloned() {
                    // Tracked depth-1 moves: destroy the surviving leaves;
                    // any absent leaf suppresses the whole-value destructor
                    // (the VM's partial-aggregate rule).
                    let cont = flag.map(|flag| self.begin_flag_guard(ctx, flag));
                    if self.has_lifecycle_method(&ty, "__deinit__") {
                        // `__deinit__` runs only when every tracked leaf
                        // survives. Its compiled `deinit self` epilogue
                        // consumes the receiver and destroys its residual
                        // fields, so the caller must not repeat that work.
                        let guards: Vec<Ptr<BasicBlock>> = leaves
                            .values()
                            .map(|&leaf| self.begin_flag_guard(ctx, leaf))
                            .collect();
                        self.emit_drop_value(ctx, ptr, &ty, false)?;
                        for guard in guards.into_iter().rev() {
                            self.end_flag_guard(ctx, guard);
                        }
                    } else {
                        self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves)?;
                    }
                    self.set_drop_flag(ctx, var, false);
                    if let Some(cont) = cont {
                        self.end_flag_guard(ctx, cont);
                    }
                    return Ok(());
                }
                match flag {
                    Some(flag) => {
                        let cont = self.begin_flag_guard(ctx, flag);
                        self.emit_drop_value(ctx, ptr, &ty, false)?;
                        self.set_drop_flag(ctx, var, false);
                        self.end_flag_guard(ctx, cont);
                        Ok(())
                    }
                    None => self.emit_drop_value(ctx, ptr, &ty, false),
                }
            }
        }
    }

    /// `ConsumeVar`: skip the whole-value destructor but destroy residual
    /// fields — a no-op unless fields carry their own destructor work. The
    /// consumed slot is empty afterwards either way. `synthesized` marks the
    /// frame-exit residual release of a `deinit` parameter, which has no MIR
    /// instruction and therefore no VM lifecycle event — only a real
    /// `MirInstr::ConsumeVar` reports a consume trace.
    pub(super) fn lower_consume_var(
        &mut self,
        ctx: &mut Context,
        var: u32,
        synthesized: bool,
    ) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                let continuation = self
                    .drop_flags
                    .get(&var)
                    .copied()
                    .map(|flag| self.begin_flag_guard(ctx, flag));
                if !synthesized
                    && self.trace_lifecycle
                    && let Ty::Struct(name, _) = ty.as_ref()
                {
                    let name = name.clone();
                    self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_CONSUME, &name);
                }
                let fully_drained_tuple = matches!(ty.as_ref(), Ty::Struct(name, _)
                    if name.starts_with("Tuple$t") && self.name.contains(".deinit_with"));
                if self.fields_need_drop(&ty) && !fully_drained_tuple {
                    // The named explicit destructor consumed the aggregate;
                    // its surviving fields still receive their ordinary
                    // reverse-order destruction (the VM's `ConsumeVar`).
                    // Struct and tuple-like consumption destroy their
                    // surviving fields/elements; deeper untracked moves
                    // reject rather than guess.
                    if !matches!(
                        ty.as_ref(),
                        Ty::Struct(..) | Ty::Tuple(..) | Ty::RuntimePack(..)
                    ) || (matches!(ty.as_ref(), Ty::Struct(name, _) if !name.starts_with("Tuple$t"))
                        && self.partially_moved.contains(&var))
                    {
                        return Err(self.unsupported(
                            "variable consumption with droppable fields".into(),
                            None,
                        ));
                    }
                    let leaves = self.leaf_flags.get(&var).cloned().unwrap_or_default();
                    let ptr = self.var_slots[var as usize];
                    self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves)?;
                }
                self.set_drop_flag(ctx, var, false);
                if let Some(continuation) = continuation {
                    self.end_flag_guard(ctx, continuation);
                }
                Ok(())
            }
        }
    }

    /// Branch on `flag` into a fresh guarded-work block, returning the
    /// continuation block. The caller emits the guarded work into the current
    /// block, then closes with [`Self::end_flag_guard`].
    pub(super) fn begin_flag_guard(&mut self, ctx: &mut Context, flag: Value) -> Ptr<BasicBlock> {
        let i1: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let load = LoadOp::new(ctx, flag, i1);
        self.append(ctx, load.get_operation(), None);
        let region = self.region.expect("lowering is inside a function");
        let work = BasicBlock::new(ctx, None, vec![]);
        work.insert_at_back(region, ctx);
        let cont = BasicBlock::new(ctx, None, vec![]);
        cont.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, load.get_result(ctx), work, vec![], cont, vec![]);
        self.append(ctx, branch.get_operation(), None);
        self.current = Some(work);
        cont
    }

    /// Branch on `value != null` into a fresh guarded-work block, returning
    /// the continuation block — the pointer analogue of
    /// [`Self::begin_flag_guard`], closed by the same
    /// [`Self::end_flag_guard`].
    pub(super) fn begin_nonnull_guard(
        &mut self,
        ctx: &mut Context,
        value: Value,
    ) -> Ptr<BasicBlock> {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let null = ZeroOp::new(ctx, ptr_ty);
        self.append(ctx, null.get_operation(), None);
        let compare = ICmpOp::new(ctx, ICmpPredicateAttr::NE, value, null.get_result(ctx));
        self.append(ctx, compare.get_operation(), None);
        let region = self.region.expect("lowering is inside a function");
        let work = BasicBlock::new(ctx, None, vec![]);
        work.insert_at_back(region, ctx);
        let cont = BasicBlock::new(ctx, None, vec![]);
        cont.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, compare.get_result(ctx), work, vec![], cont, vec![]);
        self.append(ctx, branch.get_operation(), None);
        self.current = Some(work);
        cont
    }

    /// Close a [`Self::begin_flag_guard`] region: jump to and continue in the
    /// continuation block.
    pub(super) fn end_flag_guard(&mut self, ctx: &mut Context, cont: Ptr<BasicBlock>) {
        let jump = BrOp::new(ctx, cont, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(cont);
    }

    /// Store `value` into `var`'s initialization flag; a no-op for variables
    /// without one (nothing droppable to guard).
    pub(super) fn set_drop_flag(&mut self, ctx: &mut Context, var: u32, value: bool) {
        // A whole-value (re)initialization makes every tracked leaf present
        // again.
        if value && let Some(leaves) = self.leaf_flags.get(&var) {
            let leaves: Vec<Value> = leaves.values().copied().collect();
            let present = self.bool_constant(ctx, true);
            for flag in leaves {
                let store = StoreOp::new(ctx, present, flag);
                self.append(ctx, store.get_operation(), None);
            }
        }
        let Some(&flag) = self.drop_flags.get(&var) else {
            return;
        };
        let constant = self.bool_constant(ctx, value);
        let store = StoreOp::new(ctx, constant, flag);
        self.append(ctx, store.get_operation(), None);
    }

    /// Store an SSA `i1` into a variable's initialization flag.
    pub(super) fn set_drop_flag_value(&mut self, ctx: &mut Context, var: u32, value: Value) {
        let Some(&flag) = self.drop_flags.get(&var) else {
            return;
        };
        let store = StoreOp::new(ctx, value, flag);
        self.append(ctx, store.get_operation(), None);
    }

    /// Clear the presence flag selected by a runtime pack cursor. Pack
    /// iteration moves that element out; later cleanup must destroy only the
    /// leaves which were not visited (including an early-break suffix).
    pub(super) fn clear_pack_leaf_flag(&mut self, ctx: &mut Context, var: u32, position: Value) {
        let Some(leaves) = self.leaf_flags.get(&var).cloned() else {
            return;
        };
        let region = self.region.expect("pack leaf update is inside a function");
        let continuation = BasicBlock::new(ctx, None, vec![]);
        continuation.insert_at_back(region, ctx);
        let mut next = self.current.expect("pack leaf update has a current block");
        for (index, flag) in leaves {
            self.current = Some(next);
            let clear = BasicBlock::new(ctx, None, vec![]);
            clear.insert_at_back(region, ctx);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let expected = self.int_constant(ctx, index as i64);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, position, expected);
            self.append(ctx, matches.get_operation(), None);
            let branch = CondBrOp::new(ctx, matches.get_result(ctx), clear, vec![], rest, vec![]);
            self.append(ctx, branch.get_operation(), None);
            self.current = Some(clear);
            let absent = self.bool_constant(ctx, false);
            let store = StoreOp::new(ctx, absent, flag);
            self.append(ctx, store.get_operation(), None);
            let jump = BrOp::new(ctx, continuation, vec![]);
            self.append(ctx, jump.get_operation(), None);
            next = rest;
        }
        self.current = Some(next);
        let jump = BrOp::new(ctx, continuation, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(continuation);
    }

    /// Destroy the surviving droppable top-level leaves of partially-tracked
    /// storage: leaves with a presence flag drop under that flag's guard, the
    /// rest unconditionally. Struct fields destroy in reverse declaration
    /// order and pack elements left-to-right — the VM's `drop_value` orders.
    pub(super) fn emit_surviving_leaf_drops(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        leaves: &std::collections::BTreeMap<usize, Value>,
    ) -> Result<(), PlironError> {
        let (element_tys, forward) = match ty {
            Ty::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let tys: Vec<Ty> = decl.fields.iter().map(|(_, field)| field.clone()).collect();
                (tys, false)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => (elements.clone(), true),
            _ => return Ok(()),
        };
        let composed = self
            .layout
            .struct_layout(&element_tys)
            .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
        let order: Vec<usize> = if forward {
            (0..element_tys.len()).collect()
        } else {
            (0..element_tys.len()).rev().collect()
        };
        for position in order {
            let element = element_tys[position].clone();
            if !self.needs_drop(&element) {
                continue;
            }
            let offset = composed.offsets[position];
            let address = if offset == 0 {
                ptr
            } else {
                self.gep_byte_unspanned(ctx, ptr, offset)
            };
            match leaves.get(&position).copied() {
                Some(flag) => {
                    let cont = self.begin_flag_guard(ctx, flag);
                    self.emit_drop_value(ctx, address, &element, false)?;
                    self.end_flag_guard(ctx, cont);
                }
                None => self.emit_drop_value(ctx, address, &element, false)?,
            }
        }
        Ok(())
    }

    /// The top-level leaf position a depth-1 projection addresses — a
    /// declared field of a struct-typed variable or a constant element of
    /// tuple/pack storage. These are the only shapes the per-leaf presence
    /// flags track; anything else keeps the blanket partially-moved marker.
    pub(super) fn leaf_position(&self, place: &MirPlace) -> Option<usize> {
        if place.proj.len() != 1 || place.through.is_some() {
            return None;
        }
        let ty = self.func.var_tys.get(&place.root)?;
        match (ty, &place.proj[0]) {
            (Ty::Struct(name, _), Proj::Field(field)) => self
                .struct_decls
                .get(name.as_str())
                .and_then(|decl| decl.fields.iter().position(|(name, _)| name == field)),
            (Ty::Struct(name, _), Proj::ConstIndex(index)) if name.starts_with("Tuple$t") => self
                .struct_decls
                .get(name.as_str())
                .and_then(|decl| (*index < decl.fields.len()).then_some(*index)),
            (Ty::Tuple(elements) | Ty::RuntimePack(elements), Proj::ConstIndex(index)) => {
                (*index < elements.len()).then_some(*index)
            }
            _ => None,
        }
    }

    /// Emit the VM's `drop_value` over storage: the struct's compiled
    /// `__deinit__` (its own body consumes the receiver's residual fields),
    /// else recurse into fields in reverse declaration order.
    pub(super) fn emit_drop_value(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        skip_whole_deinit: bool,
    ) -> Result<(), PlironError> {
        match ty {
            // The built-in error has no user destructor; dropping it frees
            // the message buffer invisibly (the VM's error drop is a no-op —
            // its message is arena-owned).
            Ty::Error => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            // A retained callable's teardown lives behind its record header:
            // env null (thin/bare value) and header null (no owned droppable
            // captures) are no-ops, and the drop thunk nulls the header
            // after running, so drops of aliasing two-word copies are
            // idempotent per record — the VM's deep-copying closure clones
            // are a recorded divergence.
            Ty::Func { .. } => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let env_address = self.gep_byte_unspanned(ctx, ptr, 8);
                let env = LoadOp::new(ctx, env_address, handle);
                self.append(ctx, env.get_operation(), None);
                let env = env.get_result(ctx);
                let cont_env = self.begin_nonnull_guard(ctx, env);
                let drop_thunk = LoadOp::new(ctx, env, handle);
                self.append(ctx, drop_thunk.get_operation(), None);
                let drop_thunk = drop_thunk.get_result(ctx);
                let cont_thunk = self.begin_nonnull_guard(ctx, drop_thunk);
                let void = VoidType::get(ctx).to_handle();
                let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                let thunk_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
                let call = CallOp::new(
                    ctx,
                    CallOpCallable::Indirect(drop_thunk),
                    thunk_ty,
                    vec![env],
                );
                self.append(ctx, call.get_operation(), None);
                self.end_flag_guard(ctx, cont_thunk);
                self.end_flag_guard(ctx, cont_env);
                Ok(())
            }
            Ty::Struct(name, _) => {
                let deinit = format!("{name}.__deinit__");
                if !skip_whole_deinit && self.declarations.contains_key(&deinit) {
                    let Some(signature) = self.signatures.get(&deinit) else {
                        return Err(
                            self.unsupported(format!("drop via uncompiled `{deinit}`"), None)
                        );
                    };
                    if signature.outcome.is_some() {
                        return Err(
                            self.unsupported(format!("raising destructor `{deinit}`"), None)
                        );
                    }
                    if self.trace_lifecycle {
                        let name = name.clone();
                        self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_DROP, &name);
                    }
                    let callee: Identifier = signature
                        .mangled
                        .as_str()
                        .try_into()
                        .expect("mangled names are identifier-safe");
                    let func_ty = signature.func_ty;
                    let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![ptr]);
                    self.append(ctx, call.get_operation(), None);
                    return Ok(());
                }
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let fields = decl.fields.clone();
                let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
                let composed = self
                    .layout
                    .struct_layout(&field_tys)
                    .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
                for (position, (_, field_ty)) in fields.iter().enumerate().rev() {
                    if !self.needs_drop(field_ty) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, field_ty, false)?;
                }
                Ok(())
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self
                    .layout
                    .struct_layout(&elements)
                    .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
                for (position, element) in elements.iter().enumerate().rev() {
                    if !self.needs_drop(element) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, element, false)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Whether dropping a value of `ty` performs any work: a struct with a
    /// `__deinit__`, any transitive field/element that does, the built-in
    /// error (its message buffer frees on drop), or a retained callable
    /// (its environment record may carry owned droppable captures — a
    /// null-guarded header dispatch, free when there are none).
    pub(super) fn needs_drop(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Error | Ty::Func { .. })
            || self.has_lifecycle_method(ty, "__deinit__")
            || self.fields_need_drop(ty)
    }

    /// Whether any transitive field/element of `ty` performs drop work
    /// (excluding `ty`'s own whole-value destructor).
    pub(super) fn fields_need_drop(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.needs_drop(field))),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().any(|element| self.needs_drop(element))
            }
            Ty::Variant(alternatives) => alternatives
                .iter()
                .any(|alternative| self.needs_drop(alternative)),
            _ => false,
        }
    }

    /// Whether `ty` is a struct whose program declares `method` for it.
    pub(super) fn has_lifecycle_method(&self, ty: &Ty, method: &str) -> bool {
        matches!(ty, Ty::Struct(name, _) if self
            .declarations
            .contains_key(&format!("{name}.{method}")))
    }

    /// Whether `ty` or any transitive field declares `method`.
    pub(super) fn has_nested_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        if self.has_lifecycle_method(ty, method) {
            return true;
        }
        match ty {
            Ty::Struct(name, _) => self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                decl.fields
                    .iter()
                    .any(|(_, field)| self.has_nested_lifecycle(field, method))
            }),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.has_nested_lifecycle(element, method)),
            _ => false,
        }
    }
}
