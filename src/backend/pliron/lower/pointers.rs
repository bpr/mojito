//! Pointer indexing/dereference guards, pointer and uninit storage
//! take/destroy, and aggregate copy/fork.

use super::*;

impl<'a> FnLowering<'a> {
    /// `p[i]` over the pointer subscript intrinsic: load the element at
    /// `p + i * sizeof(element)` — the VM's unchecked heap read.
    pub(super) fn lower_pointer_index(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
    ) -> Result<(), PlironError> {
        let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&base.0).cloned() else {
            return Err(
                self.unsupported_reg("pointer subscript on a non-pointer base".into(), dest)
            );
        };
        let ptr = self.reg_value(ctx, base, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, &element, dest)?;
        self.load_from(ctx, address, &element, dest)
    }

    /// `pointer + index * sizeof(element)` as an opaque address.
    pub(super) fn pointer_element_address(
        &mut self,
        ctx: &mut Context,
        pointer: Value,
        index: Reg,
        element: &Ty,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        self.guard_pointer_dereference(ctx, pointer, dest)?;
        let element_layout = self.layout.layout_of(element).map_err(|error| {
            self.unsupported_reg(format!("pointer element layout ({error})"), dest)
        })?;
        let index_value = self.reg_value(ctx, index, ScalarTy::Int)?;
        let size = self.uint_constant(ctx, element_layout.size);
        let bytes = MulOp::new_with_overflow_flag(ctx, index_value, size, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let gep = GetElementPtrOp::new(
            ctx,
            pointer,
            vec![GepIndex::Value(bytes.get_result(ctx))],
            i8_ty,
        );
        self.append(ctx, gep.get_operation(), Some(dest));
        Ok(gep.get_result(ctx))
    }

    /// Classify a raw-pointer dereference through the runtime allocation
    /// registry before LLVM touches the address.
    pub(super) fn guard_pointer_dereference(
        &mut self,
        ctx: &mut Context,
        pointer: Value,
        span: Reg,
    ) -> Result<(), PlironError> {
        let status_ty = self.shared.ensure_rt(ctx, "mjrt_pointer_status");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_pointer_status".try_into().expect("valid identifier")),
            status_ty,
            vec![pointer],
        );
        self.append(ctx, call.get_operation(), Some(span));
        let status = call.get_result(ctx);
        let dangling = self.tag_constant(ctx, 1);
        let is_dangling = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, status, dangling);
        self.append(ctx, is_dangling.get_operation(), Some(span));
        self.emit_trap_guard(
            ctx,
            is_dangling.get_result(ctx),
            TrapCategory::PointerDangling,
            span,
        )?;
        let freed = self.tag_constant(ctx, 2);
        let is_freed = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, status, freed);
        self.append(ctx, is_freed.get_operation(), Some(span));
        self.emit_trap_guard(
            ctx,
            is_freed.get_result(ctx),
            TrapCategory::PointerUseAfterFree,
            span,
        )
    }

    /// `PointerStorageTake`: move an initialized element out of
    /// `UnsafePointer` collection storage — the VM's `heap_take`
    /// (`mem::replace`): a raw byte move with no `__copyinit__` and no
    /// tombstone. Ownership verification guarantees single-take on the
    /// runnable subset; the uninitialized-misuse traps live in off-gate
    /// runtime_error fixtures.
    pub(super) fn lower_pointer_storage_take(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_value(ctx, pointer, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, element, dest)?;
        self.load_moved_from(ctx, address, element, dest)?;
        // The destination owns the moved value now: free its heap buffers if
        // it dies as a discarded temporary (the VM's Rust runtime frees
        // register temporaries invisibly).
        self.mark_owned_temp(dest, element.clone())
    }

    /// `PointerStorageDestroy`: run the element destructor in place at the
    /// element address — the VM's `heap_destroy` (`heap_take` +
    /// `drop_value`). `emit_drop_value` supplies the compiled-`__deinit__`
    /// dispatch, rejection of raising/droppable-field destructors, and the
    /// lifecycle-trace event.
    pub(super) fn lower_pointer_storage_destroy(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_value(ctx, pointer, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, element, dest)?;
        self.emit_drop_value(ctx, address, element, false)?;
        self.erased.insert(dest.0);
        Ok(())
    }

    /// `UninitStorage`: native presence bit plus inline payload. An `init`
    /// payload moves in raw (the VM's `mem::replace`, no `__moveinit__`).
    pub(super) fn lower_uninit_storage(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        init: Option<Reg>,
    ) -> Result<(), PlironError> {
        let Some(dest_ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped uninit storage result".into(), dest));
        };
        let Some(element) = crate::types::uninit_storage_element(&dest_ty).cloned() else {
            return Err(self.unsupported_reg(
                format!("uninit storage of non-storage type `{dest_ty}`"),
                dest,
            ));
        };
        let layout = self
            .layout
            .layout_of(&dest_ty)
            .map_err(|error| self.unsupported_reg(format!("uninit storage ({error})"), dest))?;
        if layout.size == 0 {
            self.erased.insert(dest.0);
            return Ok(());
        }
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        let fields = self
            .layout
            .struct_layout(&[Ty::Bool, element.clone()])
            .map_err(|error| self.unsupported_reg(format!("uninit storage ({error})"), dest))?;
        let initialized = self.bool_constant(ctx, init.is_some());
        let flag_store = StoreOp::new(ctx, initialized, storage);
        self.append(ctx, flag_store.get_operation(), Some(dest));
        if let Some(src) = init {
            let payload = self.offset_address(ctx, storage, fields.offsets[1]);
            self.store_to(ctx, payload, &element, src)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `UninitStorageTake`: move the payload out of inline uninit storage —
    /// a raw byte move (the VM's `mem::replace` of the payload box).
    pub(super) fn lower_uninit_storage_take(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        storage: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_ptr(ctx, storage)?;
        self.guard_uninit_present(ctx, ptr, TrapCategory::UninitTake, dest)?;
        let fields = self
            .layout
            .struct_layout(&[Ty::Bool, element.clone()])
            .map_err(|error| self.unsupported_reg(format!("uninit storage ({error})"), dest))?;
        let payload = self.offset_address(ctx, ptr, fields.offsets[1]);
        self.load_moved_from(ctx, payload, element, dest)?;
        let absent = self.bool_constant(ctx, false);
        let clear = StoreOp::new(ctx, absent, ptr);
        self.append(ctx, clear.get_operation(), Some(dest));
        self.mark_owned_temp(dest, element.clone())
    }

    /// `UninitStorageDestroy`: run the payload destructor in place — the
    /// VM's take-then-`drop_value`.
    pub(super) fn lower_uninit_storage_destroy(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        storage: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_ptr(ctx, storage)?;
        self.guard_uninit_present(ctx, ptr, TrapCategory::UninitDestroy, dest)?;
        let fields = self
            .layout
            .struct_layout(&[Ty::Bool, element.clone()])
            .map_err(|error| self.unsupported_reg(format!("uninit storage ({error})"), dest))?;
        let payload = self.offset_address(ctx, ptr, fields.offsets[1]);
        self.emit_drop_value(ctx, payload, element, false)?;
        self.erased.insert(dest.0);
        Ok(())
    }

    /// An owned copy of an aggregate — the VM's `clone_value`: the nominal
    /// String copies through the native bridge (the stdlib byte loop needs
    /// machinery outside this stage), a struct's compiled `__copyinit__` runs
    /// when it defines one, and otherwise a byte copy applies (exact for
    /// every type whose transitive fields carry no user copy constructor; a
    /// nested-only `__copyinit__` rejects rather than diverge from the VM's
    /// recursive clone).
    pub(super) fn copy_aggregate(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        ty: &Ty,
        layout: Layout,
        src_ptr: Value,
    ) -> Result<(), PlironError> {
        if let Ty::Struct(name, _) = ty
            && crate::symbol::is_stdlib_string_struct(name)
        {
            // The stdlib copy constructor: a fresh `cap`-byte allocation with
            // `size` bytes copied and `size`/`cap` preserved.
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, dest);
            let src_cap = self.string_cap(ctx, src_ptr, dest);
            let new_data = self.emit_alloc(ctx, src_cap, 1, dest);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, dest);
            self.store_string_fields(ctx, storage, new_data, src_size, src_cap, dest);
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        }
        if matches!(ty, Ty::Error) {
            // The VM's clone of an error duplicates its message, so the copy
            // outlives the original's drop.
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, dest);
            let new_data = self.emit_alloc(ctx, src_size, 1, dest);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, dest);
            self.store_string_fields(ctx, storage, new_data, src_size, src_size, dest);
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        }
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        if let Ty::Struct(name, _) = ty
            && self
                .declarations
                .contains_key(&format!("{name}.__copyinit__"))
        {
            let copyinit = format!("{name}.__copyinit__");
            let Some(signature) = self.signatures.get(&copyinit) else {
                return Err(self.unsupported_reg(format!("copy via uncompiled `{copyinit}`"), dest));
            };
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported_reg(format!("raising copy constructor `{copyinit}`"), dest)
                );
            }
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            // `__copyinit__(out self, copy: Self)`: dest storage, then source.
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                func_ty,
                vec![storage, src_ptr],
            );
            self.append(ctx, call.get_operation(), Some(dest));
            // A copy constructor may have allocated; release what the
            // invisible rule understands (String buffers) or, for a stdlib
            // collection copy, its own compiled destructor chain.
            if self.releasable(ty) || self.stdlib_deinit_temp(ty) || self.needs_drop(ty) {
                self.mark_owned_temp(dest, ty.clone())?;
            }
        } else if self.has_nested_lifecycle(ty, "__copyinit__") {
            self.fork_value_into(ctx, storage, ty, layout, src_ptr, dest)?;
            self.reg_values.insert(dest.0, storage);
            if self.releasable(ty) || self.stdlib_deinit_temp(ty) || self.needs_drop(ty) {
                self.mark_owned_temp(dest, ty.clone())?;
            }
            return Ok(());
        } else if self.owns_heap(ty) {
            // Drop elaboration may destroy the owning variable immediately
            // after its last use — before this temporary is read — so
            // aliasing its buffers is not an option under real frees: fork
            // the copy and release it after its own last use (the VM's
            // arena-shared plain clone, made explicit).
            self.fork_value_into(ctx, storage, ty, layout, src_ptr, dest)?;
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        } else {
            // A byte copy of a heap-less value carries everything it needs.
            self.mem_copy(ctx, storage, src_ptr, layout.size, dest);
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Fork the value at `src_ptr` into `dst`: a byte copy whose
    /// String/Error components are re-duplicated so each copy owns its own
    /// buffers — the native analog of the VM's arena-shared plain clone
    /// (whose aliasing is invisible because the arena never reclaims). User
    /// copy constructors never run here; the VM's plain clone does not run
    /// them either. Values owning raw pointer storage cannot fork bufferwise
    /// and reject contextually.
    pub(super) fn fork_value_into(
        &mut self,
        ctx: &mut Context,
        dst: Value,
        ty: &Ty,
        layout: Layout,
        src_ptr: Value,
        span: Reg,
    ) -> Result<(), PlironError> {
        if let Ty::Struct(name, _) = ty
            && crate::symbol::is_stdlib_string_struct(name)
        {
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, span);
            let src_cap = self.string_cap(ctx, src_ptr, span);
            let new_data = self.emit_alloc(ctx, src_cap, 1, span);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, span);
            self.store_string_fields(ctx, dst, new_data, src_size, src_cap, span);
            return Ok(());
        }
        if matches!(ty, Ty::Error) {
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, span);
            let new_data = self.emit_alloc(ctx, src_size, 1, span);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, span);
            self.store_string_fields(ctx, dst, new_data, src_size, src_size, span);
            return Ok(());
        }
        // A structural clone of a stdlib owning collection must use its
        // compiled copy constructor so element ownership is duplicated. Do
        // not generalize this to user structs: their observable
        // `__copyinit__` belongs only to an explicit CopyValue boundary.
        if self.stdlib_deinit_temp(ty)
            && let Ty::Struct(name, _) = ty
            && self
                .declarations
                .contains_key(&format!("{name}.__copyinit__"))
        {
            let copyinit = format!("{name}.__copyinit__");
            let signature = self.signatures.get(&copyinit).ok_or_else(|| {
                self.unsupported_reg(format!("copy via uncompiled `{copyinit}`"), span)
            })?;
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported_reg(format!("raising copy constructor `{copyinit}`"), span)
                );
            }
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                signature.func_ty,
                vec![dst, src_ptr],
            );
            self.append(ctx, call.get_operation(), Some(span));
            return Ok(());
        }
        if let Ty::Variant(alternatives) = ty {
            self.mem_copy(ctx, dst, src_ptr, layout.size, span);
            let variant = self.layout.variant_layout(alternatives).map_err(|error| {
                self.unsupported_reg(format!("Variant fork layout ({error})"), span)
            })?;
            let handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
            let tag = LoadOp::new(ctx, src_ptr, handle);
            self.append(ctx, tag.get_operation(), Some(span));
            let src_payload = self.offset_address(ctx, src_ptr, variant.payload_offset);
            let dst_payload = self.offset_address(ctx, dst, variant.payload_offset);
            let region = self.region.expect("Variant fork is inside a function");
            let continuation = BasicBlock::new(ctx, None, vec![]);
            continuation.insert_at_back(region, ctx);
            let mut next = self.current.expect("Variant fork has a current block");
            for (index, alternative) in alternatives.iter().enumerate() {
                if !self.owns_heap(alternative) {
                    continue;
                }
                self.current = Some(next);
                let fork_block = BasicBlock::new(ctx, None, vec![]);
                fork_block.insert_at_back(region, ctx);
                let rest = BasicBlock::new(ctx, None, vec![]);
                rest.insert_at_back(region, ctx);
                let expected = self.tag_constant(ctx, index as u32);
                let matches =
                    ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), expected);
                self.append(ctx, matches.get_operation(), Some(span));
                let branch = CondBrOp::new(
                    ctx,
                    matches.get_result(ctx),
                    fork_block,
                    vec![],
                    rest,
                    vec![],
                );
                self.append(ctx, branch.get_operation(), Some(span));
                self.current = Some(fork_block);
                let alternative_layout = self.layout.layout_of(alternative).map_err(|error| {
                    self.unsupported_reg(format!("Variant fork payload layout ({error})"), span)
                })?;
                self.fork_value_into(
                    ctx,
                    dst_payload,
                    alternative,
                    alternative_layout,
                    src_payload,
                    span,
                )?;
                let jump = BrOp::new(ctx, continuation, vec![]);
                self.append(ctx, jump.get_operation(), Some(span));
                next = rest;
            }
            self.current = Some(next);
            let jump = BrOp::new(ctx, continuation, vec![]);
            self.append(ctx, jump.get_operation(), Some(span));
            self.current = Some(continuation);
            return Ok(());
        }
        let elements: Vec<(Ty, u64)> = match ty {
            Ty::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()) else {
                    return Err(self.unsupported_reg(format!("fork of undeclared `{ty}`"), span));
                };
                let fields: Vec<Ty> = decl.fields.iter().map(|(_, ty)| ty.clone()).collect();
                let composed = self.struct_layout_of(&fields, span)?;
                fields.into_iter().zip(composed.offsets).collect()
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self.struct_layout_of(&elements, span)?;
                elements.into_iter().zip(composed.offsets).collect()
            }
            other => {
                return Err(self.unsupported_reg(format!("fork of `{other}`"), span));
            }
        };
        self.mem_copy(ctx, dst, src_ptr, layout.size, span);
        for (element, offset) in elements {
            if !self.owns_heap(&element) {
                continue;
            }
            let element_layout = self.layout.layout_of(&element).map_err(|error| {
                self.unsupported_reg(format!("fork element layout ({error})"), span)
            })?;
            let src_field = self.gep_byte(ctx, src_ptr, offset, span);
            let dst_field = self.gep_byte(ctx, dst, offset, span);
            self.fork_value_into(ctx, dst_field, &element, element_layout, src_field, span)?;
        }
        Ok(())
    }
}
