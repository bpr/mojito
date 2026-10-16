//! Drop elaboration lowering: drop/consume vars, drop flags, pack leaf
//! flags, and recursive value drops.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl<'a> FnLowering<'a> {
    /// The symbol of the lifecycle member `member` (`__deinit__`,
    /// `__copyinit__`, `__moveinit__`) the struct `name` owns, from its
    /// declaration's member table, when the program declares it.
    pub(super) fn lifecycle_symbol(&self, name: &str, member: &str) -> Option<&'a str> {
        self.struct_member_symbols(name, member)
            .next()
            .map(|(_, symbol)| symbol)
    }

    /// The entries of the member `method` (`write_to`, `__eq__`) in the
    /// struct `name`'s member table, each overload and instance with its
    /// key, that the program declares.
    pub(super) fn struct_member_symbols(
        &self,
        name: &str,
        method: &str,
    ) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.struct_decls
            .get(name)
            .copied()
            .into_iter()
            .flat_map(move |declaration| declaration.members_of(method))
            .map(|entry| (entry.member.as_str(), entry.symbol.as_str()))
            .filter(|(_, symbol)| self.declarations.contains_key(*symbol))
    }

    /// The symbol of the member `method` the struct `name` runs for a
    /// dispatch with `argc` arguments besides the receiver, from its member
    /// table: the one compiled instance under the key callable resolution
    /// selects among the member's overloads.
    pub(super) fn struct_member(&self, name: &str, method: &str, argc: usize) -> Option<&'a str> {
        let candidates = mojito_symbol::symbol::MemberCandidates::new(
            name,
            self.struct_member_symbols(name, method)
                .filter_map(|(key, symbol)| {
                    Some((key, symbol, self.signatures.get(symbol)?.params.len()))
                }),
        );
        candidates.resolve(method, argc)
    }

    /// The destructor a cleanup list's or a frame exit's drop of `var` runs,
    /// which no instruction names: [`Self::type_destructor`] of its slot.
    pub(super) fn cleanup_destructor(&self, var: u32) -> Option<&'a str> {
        self.type_destructor(self.func.var_tys.get(&var)?)
    }

    /// `DropVar` — the VM's `drop_named`: nothing for scalars; for aggregates
    /// call `destructor`, the compiled `__deinit__` the drop names (its body
    /// consumes the receiver's fields), else destroy the fields in
    /// declaration order. Combinations whose residual state is only
    /// dynamically knowable (a destructor plus independently droppable
    /// fields, or a partially-moved variable) reject instead of guessing.
    pub(super) fn lower_drop_var(
        &mut self,
        ctx: &mut Context,
        var: u32,
        destructor: Option<&str>,
    ) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                if !self.needs_named_drop(&ty, destructor) {
                    return Ok(());
                }
                if self.partially_moved.contains(&var) {
                    return Err(self.unsupported(
                        format!(
                            "drop of partially-moved variable `{}`",
                            self.func
                                .var_names
                                .get(var as usize)
                                .map_or("?", String::as_str)
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
                    if destructor.is_some() {
                        // `__deinit__` runs only when every tracked leaf
                        // survives. Its compiled `deinit self` epilogue
                        // consumes the receiver and destroys its residual
                        // fields, so the caller must not repeat that work.
                        let guards: Vec<Ptr<BasicBlock>> = leaves
                            .values()
                            .map(|&leaf| self.begin_flag_guard(ctx, leaf))
                            .collect();
                        self.emit_drop(ctx, ptr, &ty, destructor)?;
                        for guard in guards.into_iter().rev() {
                            self.end_flag_guard(ctx, guard);
                        }
                    } else {
                        let owned_pack =
                            self.declarations.get(self.name).is_some_and(|declaration| {
                                mojito_mir::mir::owned_pack_slot(declaration) == Some(var)
                            });
                        self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves, owned_pack)?;
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
                        self.emit_drop(ctx, ptr, &ty, destructor)?;
                        self.set_drop_flag(ctx, var, false);
                        self.end_flag_guard(ctx, cont);
                        Ok(())
                    }
                    None => self.emit_drop(ctx, ptr, &ty, destructor),
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
                    && let TyKind::Struct(name, _) = &ty.kind()
                {
                    let name = name.clone();
                    self.emit_trace_text(ctx, mojito_native::native::rt_abi::TRACE_CONSUME, &name);
                }
                if self.fields_need_drop(&ty) {
                    // The named explicit destructor owns the aggregate; its
                    // surviving fields receive their ordinary declaration-
                    // order destruction (the VM's `ConsumeVar`).
                    // Struct and tuple-like consumption destroy their
                    // surviving fields/elements; deeper untracked moves
                    // reject rather than guess.
                    if !matches!(
                        &ty.kind(),
                        TyKind::Struct(..) | TyKind::Tuple(..) | TyKind::RuntimePack(..)
                    ) || (matches!(&ty.kind(), TyKind::Struct(..))
                        && self.partially_moved.contains(&var))
                    {
                        return Err(self.unsupported(
                            "variable consumption with droppable fields".into(),
                            None,
                        ));
                    }
                    let leaves = self.leaf_flags.get(&var).cloned().unwrap_or_default();
                    let ptr = self.var_slots[var as usize];
                    self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves, false)?;
                }
                self.set_drop_flag(ctx, var, false);
                if let Some(continuation) = continuation {
                    self.end_flag_guard(ctx, continuation);
                }
                Ok(())
            }
        }
    }

    /// `DropPlace`: destroy one depth-1 field of an aggregate variable under
    /// its leaf presence flag (the VM's tombstone), so the receiver's later
    /// `ConsumeVar`/exit backstop skips it. The entry-block flag allocation
    /// tracks every depth-1 `DropPlace` place. A nested or reference-rooted
    /// place has no flag: drop elaboration emits one only where the place is
    /// statically initialized (a store about to overwrite it), so it drops
    /// unguarded at its address. `destructor` is the place value's own
    /// `__deinit__` the drop names.
    pub(super) fn lower_drop_place(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        destructor: Option<&str>,
    ) -> Result<(), PlironError> {
        let flagged = self.leaf_position(place).and_then(|position| {
            self.leaf_flags
                .get(&place.root)
                .and_then(|leaves| leaves.get(&position))
                .map(|flag| (position, *flag))
        });
        let Some((position, flag)) = flagged else {
            let (address, ty) = self.place_address(ctx, place, Reg(0))?;
            return if self.needs_named_drop(&ty, destructor) {
                self.emit_drop(ctx, address, &ty, destructor)
            } else {
                Ok(())
            };
        };
        let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(place.root)? else {
            return Ok(());
        };
        let element_tys: Vec<Ty> = match &ty.kind() {
            TyKind::Struct(name, _) => match self.struct_decls.get(name.as_str()).copied() {
                Some(decl) => decl.fields.iter().map(|(_, field)| field.clone()).collect(),
                None => return Ok(()),
            },
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements.clone(),
            _ => return Ok(()),
        };
        let element = element_tys[position].clone();
        if !self.needs_named_drop(&element, destructor) {
            return Ok(());
        }
        let composed = self
            .layout
            .struct_layout(&element_tys)
            .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
        let ptr = self.var_slots[place.root as usize];
        let offset = composed.offsets[position];
        let address = if offset == 0 {
            ptr
        } else {
            self.gep_byte_unspanned(ctx, ptr, offset)
        };
        let cont = self.begin_flag_guard(ctx, flag);
        self.emit_drop(ctx, address, &element, destructor)?;
        let absent = self.bool_constant(ctx, false);
        let store = StoreOp::new(ctx, absent, flag);
        self.append(ctx, store.get_operation(), None);
        self.end_flag_guard(ctx, cont);
        Ok(())
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
    /// rest unconditionally. Struct fields and tuple storage destroy in
    /// declaration order (left-to-right) — the VM's `drop_value` order; an
    /// owning positional collector (`last_first`) destroys last to first.
    pub(super) fn emit_surviving_leaf_drops(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        leaves: &std::collections::BTreeMap<usize, Value>,
        last_first: bool,
    ) -> Result<(), PlironError> {
        let element_tys = match ty.kind() {
            TyKind::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                decl.fields.iter().map(|(_, field)| field.clone()).collect()
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements.clone(),
            _ => return Ok(()),
        };
        let composed = self
            .layout
            .struct_layout(&element_tys)
            .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
        let mut positions: Vec<usize> = (0..element_tys.len()).collect();
        if last_first {
            positions.reverse();
        }
        for position in positions {
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
                    self.emit_drop_value(ctx, address, &element)?;
                    self.end_flag_guard(ctx, cont);
                }
                None => self.emit_drop_value(ctx, address, &element)?,
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
        match (ty.kind(), &place.proj[0]) {
            (TyKind::Struct(name, _), Proj::Field(field)) => self
                .struct_decls
                .get(name.as_str())
                .and_then(|decl| decl.fields.iter().position(|(name, _)| name == field)),
            (TyKind::Tuple(elements) | TyKind::RuntimePack(elements), Proj::ConstIndex(index)) => {
                (*index < elements.len()).then_some(*index)
            }
            _ => None,
        }
    }

    /// Record that the projection `place` no longer holds a value: a tracked
    /// top-level leaf clears its presence flag (later drops skip it, like the
    /// VM's `Value::Moved` tombstone); anything deeper records the blanket
    /// marker, so a whole-variable drop refuses destructor work instead of
    /// double-freeing. A whole variable has no flag to clear.
    pub(super) fn clear_presence(&mut self, ctx: &mut Context, place: &MirPlace) {
        if place.proj.is_empty() {
            return;
        }
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

    /// Emit the VM's `drop_value` over storage no drop instruction names a
    /// destructor for (a field, a storage destroy, a variant payload, a
    /// capture, a frame-exit release): [`Self::emit_drop`] with the
    /// `__deinit__` the member table of `ty`'s struct records.
    pub(super) fn emit_drop_value(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        self.emit_drop(ctx, ptr, ty, self.type_destructor(ty))
    }

    /// Emit the drop of the value of type `ty` at `ptr`: a call of
    /// `destructor`, a struct's compiled `__deinit__` (its own body consumes
    /// the receiver's residual fields), else recurse into fields in
    /// declaration order.
    fn emit_drop(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        destructor: Option<&str>,
    ) -> Result<(), PlironError> {
        match ty.kind() {
            // The built-in error has no user destructor; dropping it frees
            // the message buffer invisibly (the VM's error drop is a no-op —
            // its message is arena-owned).
            TyKind::Error => {
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
            TyKind::Func(_) => {
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
            TyKind::Struct(name, _) => {
                if let Some(deinit) = destructor {
                    let Some(signature) = self.signatures.get(deinit) else {
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
                        self.emit_trace_text(ctx, mojito_native::native::rt_abi::TRACE_DROP, &name);
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
                for (position, (_, field_ty)) in fields.iter().enumerate() {
                    if !self.needs_drop(field_ty) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, field_ty)?;
                }
                Ok(())
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self
                    .layout
                    .struct_layout(&elements)
                    .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
                for (position, element) in elements.iter().enumerate() {
                    if !self.needs_drop(element) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, element)?;
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
        self.needs_named_drop(ty, self.type_destructor(ty))
    }

    /// The `__deinit__` the member table of `ty`'s struct records, for a
    /// teardown no drop instruction names a destructor for.
    fn type_destructor(&self, ty: &Ty) -> Option<&'a str> {
        match ty.kind() {
            TyKind::Struct(name, _) => self.lifecycle_symbol(name, "__deinit__"),
            _ => None,
        }
    }

    /// [`Self::needs_drop`] for a drop naming `destructor` as `ty`'s own.
    fn needs_named_drop(&self, ty: &Ty, destructor: Option<&str>) -> bool {
        destructor.is_some()
            || matches!(ty.kind(), TyKind::Error | TyKind::Func(_))
            || self.fields_need_drop(ty)
    }

    /// Whether any transitive field/element of `ty` performs drop work
    /// (excluding `ty`'s own whole-value destructor).
    pub(super) fn fields_need_drop(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.needs_drop(field))),
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => {
                elements.iter().any(|element| self.needs_drop(element))
            }
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .any(|alternative| self.needs_drop(alternative)),
            _ => false,
        }
    }

    /// Whether `ty` is a struct that owns the lifecycle member `method`.
    pub(super) fn has_lifecycle_method(&self, ty: &Ty, method: &str) -> bool {
        matches!(ty.kind(), TyKind::Struct(name, _) if self.lifecycle_symbol(name, method).is_some())
    }

    /// Whether `ty` or any transitive field declares `method`.
    pub(super) fn has_nested_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        if self.has_lifecycle_method(ty, method) {
            return true;
        }
        match ty.kind() {
            TyKind::Struct(name, _) => self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                decl.fields
                    .iter()
                    .any(|(_, field)| self.has_nested_lifecycle(field, method))
            }),
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.has_nested_lifecycle(element, method)),
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .any(|alternative| self.has_nested_lifecycle(alternative, method)),
            _ => false,
        }
    }
}
