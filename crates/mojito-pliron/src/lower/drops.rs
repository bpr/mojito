//! Drop elaboration lowering: drop/consume vars and recursive value drops.
//! Every drop is static, as drop elaboration decided it: the MIR verifier
//! holds every `drop.var` and `var.consume` to a slot every path reaching
//! it holds, and a part's drop destroys at the address it names.

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

    /// Whether `ty`'s teardown/copy chain can reach a user-authored
    /// lifecycle method (`__deinit__`/`__copyinit__`/`__moveinit__`),
    /// walking struct fields and pointer element types (a container reaches
    /// pointed-to elements through its compiled chain). Stdlib-authored
    /// chains are exempt: pure frees/relocations, nothing user-observable.
    pub(super) fn chain_runs_user_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        match ty.kind() {
            TyKind::Struct(name, _) => {
                let template = name.split("$mono").next().unwrap_or(name);
                let stdlib = template.starts_with("__module$std$")
                    || mojito_symbol::symbol::is_stdlib_string_struct(name)
                    || matches!(
                        template,
                        "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
                    );
                if !stdlib && self.lifecycle_symbol(name, method).is_some() {
                    return true;
                }
                self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                    decl.fields
                        .iter()
                        .any(|(_, field)| self.chain_runs_user_lifecycle(field, method))
                })
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.chain_runs_user_lifecycle(element, method)),
            TyKind::Pointer { element, .. } => self.chain_runs_user_lifecycle(element, method),
            _ => false,
        }
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

    /// `DropVar` — the VM's `drop_named`: nothing for scalars; for aggregates
    /// call `destructor`, the compiled `__deinit__` the drop names (its body
    /// consumes the receiver's fields), else destroy the fields in
    /// declaration order. The slot holds its value on every path reaching
    /// the drop (`mir::verify`'s slot-end rule), so the drop is
    /// unconditional. A slot some part of which an instruction ended
    /// ([`FnLowering::partially_moved`]) rejects instead of guessing.
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
                self.emit_drop(ctx, ptr, &ty, destructor)
            }
        }
    }

    /// Whether the body being lowered is one the compiler synthesized (a
    /// struct's fieldwise `__deinit__`), whose lifecycle events the trace
    /// leaves out, as the VM's does.
    pub(super) fn synthesized_body(&self) -> bool {
        self.declarations
            .get(self.name)
            .is_some_and(mojito_mir::mir::MirFunctionDeclaration::is_synthesized)
    }

    /// `ConsumeVar`: end the slot, upstream's exit `mark_destroyed`, which
    /// destroys nothing, as drop elaboration has destroyed every part the
    /// value still held.
    pub(super) fn lower_consume_var(
        &mut self,
        ctx: &mut Context,
        var: u32,
    ) -> Result<(), PlironError> {
        if let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(var)?
            && self.trace_lifecycle
            && !self.synthesized_body()
            && let TyKind::Struct(name, _) = &ty.kind()
        {
            let name = name.clone();
            self.emit_trace_text(ctx, mojito_native::native::rt_abi::TRACE_CONSUME, &name);
        }
        Ok(())
    }

    /// `DropPlace`: destroy one part at its address, unconditionally, as
    /// drop elaboration emits a part's drop only where the part holds its
    /// value. `destructor` is the place value's own `__deinit__` the drop
    /// names.
    pub(super) fn lower_drop_place(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        destructor: Option<&str>,
    ) -> Result<(), PlironError> {
        let (address, ty) = self.place_address(ctx, place, Reg(0))?;
        if self.needs_named_drop(&ty, destructor) {
            self.emit_drop(ctx, address, &ty, destructor)
        } else {
            Ok(())
        }
    }

    /// Branch on `value != null` into a fresh guarded-work block, returning
    /// the continuation block. The caller emits the guarded work into the
    /// current block, then closes with [`Self::end_guard`].
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

    /// Close a [`Self::begin_nonnull_guard`] region: jump to and continue in
    /// the continuation block.
    pub(super) fn end_guard(&mut self, ctx: &mut Context, cont: Ptr<BasicBlock>) {
        let jump = BrOp::new(ctx, cont, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(cont);
    }

    /// Record that an instruction ended the part `place` names, so a whole
    /// drop of its slot refuses rather than destroy the part twice. A whole
    /// place ends nothing a later drop could repeat.
    pub(super) fn end_part(&mut self, place: &MirPlace) {
        if !place.proj.is_empty() {
            self.partially_moved.insert(place.root);
        }
    }

    /// Emit the VM's `drop_value` over storage no instruction names a
    /// destructor for (a field of a struct without one, a frame-exit
    /// release): [`Self::emit_drop`] with the `__deinit__` the member table
    /// of `ty`'s struct records.
    pub(super) fn emit_drop_value(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        self.emit_drop(ctx, ptr, ty, self.type_destructor(ty))
    }

    /// Emit the drop of the value of type `ty` at `ptr`: a call of
    /// `destructor`, a struct's compiled `__deinit__` (its own body destroys
    /// the receiver's fields), else recurse into fields in
    /// declaration order.
    pub(super) fn emit_drop(
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
                self.end_guard(ctx, cont_thunk);
                self.end_guard(ctx, cont_env);
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
                    if self.trace_lifecycle
                        && !self
                            .declarations
                            .get(deinit)
                            .is_some_and(mojito_mir::mir::MirFunctionDeclaration::is_synthesized)
                    {
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
    /// teardown no drop instruction names a destructor for: a frame exit's
    /// backstop drop of a still-live slot.
    pub(super) fn type_destructor(&self, ty: &Ty) -> Option<&'a str> {
        match ty.kind() {
            TyKind::Struct(name, _) => self.lifecycle_symbol(name, "__deinit__"),
            _ => None,
        }
    }

    /// [`Self::needs_drop`] for a drop naming `destructor` as `ty`'s own.
    pub(super) fn needs_named_drop(&self, ty: &Ty, destructor: Option<&str>) -> bool {
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
