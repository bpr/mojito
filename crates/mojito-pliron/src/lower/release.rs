//! Ownership release machinery: owned-temp tracking, release/free
//! emission, and string field/alloc helpers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
use std::collections::BTreeSet;

/// Where an owned temporary is released: in the block that defines it,
/// after the instruction there that reads it last (itself, or the region op
/// whose regions do), or at the first release point when nothing reads it.
#[derive(Clone, Copy)]
pub(super) struct TempRelease {
    block: BlockKey,
    after: Option<usize>,
}

impl<'a> FnLowering<'a> {
    /// Whether the invisible-release rule can free every heap buffer a value
    /// of `ty` owns without running user code: the nominal String (one
    /// buffer), and byte-copied aggregates over such fields.
    pub(super) fn releasable(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::Error => true,
            TyKind::Struct(name, _) if mojito_symbol::symbol::is_stdlib_string_struct(name) => true,
            TyKind::Struct(name, _) => {
                self.lifecycle_symbol(name, "__copyinit__").is_none()
                    && self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                        decl.fields
                            .iter()
                            .all(|(_, field)| !self.owns_heap(field) || self.releasable(field))
                    })
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements
                .iter()
                .all(|element| !self.owns_heap(element) || self.releasable(element)),
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| !self.owns_heap(alternative) || self.releasable(alternative)),
            _ => !self.owns_heap(ty),
        }
    }

    /// Whether a value of `ty` semantically owns heap memory (the nominal
    /// String's buffer; raw pointers are not owned).
    pub(super) fn owns_heap(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::Error => true,
            TyKind::Struct(name, _) if mojito_symbol::symbol::is_stdlib_string_struct(name) => true,
            TyKind::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.owns_heap(field))),
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => {
                elements.iter().any(|element| self.owns_heap(element))
            }
            TyKind::Variant(alternatives) => alternatives
                .iter()
                .any(|alternative| self.owns_heap(alternative)),
            _ => false,
        }
    }

    /// Whether `name`'s declaration takes a variadic pack. Such callees
    /// always bind through the slot matcher — an argument count equal to the
    /// physical parameter count (arity one against the pack slot) must still
    /// build pack storage.
    pub(super) fn variadic_callee(&self, name: &str) -> bool {
        self.declarations
            .get(name)
            .is_some_and(|decl| decl.variadic.is_some())
    }

    /// Record `dest` as an owned heap-carrying temporary, released after its
    /// final read in the block defining it ([`TempRelease`]). A read the
    /// release cannot follow rejects instead of leaking.
    pub(super) fn mark_owned_temp(&mut self, dest: Reg, ty: Ty) -> Result<(), PlironError> {
        self.mark_temp(dest, ty, true)
    }

    /// [`Self::mark_owned_temp`] for a constructed value whose consumer may
    /// share its bytes: a temporary the release cannot follow stays unmarked
    /// instead of rejecting.
    pub(super) fn mark_tracked_owned_temp(&mut self, dest: Reg, ty: Ty) -> Result<(), PlironError> {
        self.mark_temp(dest, ty, false)
    }

    /// Release the owned temporaries defined in the blocks a terminator
    /// leaves, `open_blocks[open..]`, but those it reads itself, which it
    /// carries or returns: on that path nothing reads them again.
    pub(super) fn release_leaving_temps(
        &mut self,
        ctx: &mut Context,
        open: usize,
        term: &MirTerm,
    ) -> Result<(), PlironError> {
        self.release_temps_leaving(ctx, open, &mojito_mir::mir::terminator_regs(term))
    }

    /// Release the owned temporaries defined in `open_blocks[open..]` but
    /// those in `kept`.
    pub(super) fn release_temps_leaving(
        &mut self,
        ctx: &mut Context,
        open: usize,
        kept: &[Reg],
    ) -> Result<(), PlironError> {
        let left: Vec<BlockKey> = self
            .open_blocks
            .get(open..)
            .unwrap_or_default()
            .iter()
            .map(|block| block.key)
            .collect();
        let mut due: Vec<(u32, Ty)> = self
            .owned_temps
            .iter()
            .filter(|(reg, _)| {
                !kept.contains(&Reg(**reg))
                    && self
                        .temp_releases
                        .get(reg)
                        .is_some_and(|release| left.contains(&release.block))
            })
            .map(|(reg, ty)| (*reg, ty.clone()))
            .collect();
        due.sort_by_key(|(reg, _)| *reg);
        for (reg, ty) in due {
            if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
                eprintln!("TEMP-DBG {} release-leaving %r{} {ty}", self.name, reg);
            }
            self.emit_release_reg(ctx, reg, &ty)?;
        }
        Ok(())
    }

    /// Release every owned temporary whose final read was the instruction
    /// just lowered (or that nothing reads).
    pub(super) fn flush_owned_temps(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        for (reg, ty) in self.due_owned_temps() {
            self.owned_temps.remove(&reg);
            if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
                eprintln!("TEMP-DBG {} release %r{} {ty}", self.name, reg);
            }
            self.emit_release_reg(ctx, reg, &ty)?;
        }
        Ok(())
    }

    /// The owned temporaries whose final read is the instruction just
    /// lowered (or that nothing reads), in register order.
    pub(super) fn due_owned_temps(&self) -> Vec<(u32, Ty)> {
        let Some(position) = self.open_blocks.last().copied() else {
            return Vec::new();
        };
        let mut due: Vec<(u32, Ty)> = self
            .owned_temps
            .iter()
            .filter(|(reg, _)| {
                self.temp_releases.get(reg).is_some_and(|release| {
                    release.block == position.key
                        && release.after.is_none_or(|after| after == position.index)
                })
            })
            .map(|(reg, ty)| (*reg, ty.clone()))
            .collect();
        due.sort_by_key(|(reg, _)| *reg);
        due
    }

    /// Free the heap buffers register `reg` (an owned temporary) carries,
    /// without running any user destructor — mirroring the VM, which never
    /// destroys register temporaries.
    pub(super) fn emit_release_reg(
        &mut self,
        ctx: &mut Context,
        reg: u32,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        if matches!(ty.kind(), TyKind::StringLiteral) {
            let Some(descriptor) = self.str_runtime.get(&reg).copied() else {
                return Ok(());
            };
            self.emit_free(ctx, descriptor.data);
            return Ok(());
        }
        let Some(storage) = self.reg_values.get(&reg).copied() else {
            return Ok(());
        };
        let moved = self.moved_out_paths(reg, ty);
        if moved.is_empty() {
            return self.emit_release_temp(ctx, storage, ty);
        }
        self.emit_release_storage_except(ctx, storage, ty, &mut Vec::new(), &moved)
    }

    /// Free the heap buffers the `ty`-typed temporary at `storage` carries.
    pub(super) fn emit_release_temp(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        if self.released_by_destructor(ty) {
            let traced = self.trace_lifecycle;
            self.trace_lifecycle = false;
            let released = self.emit_drop_value(ctx, storage, ty);
            self.trace_lifecycle = traced;
            return released;
        }
        // Register temporaries are not semantic objects in the VM: reclaim
        // their concrete buffers recursively, but never dispatch a language
        // destructor (including a conditional stdlib wrapper destructor that
        // could reach a user payload).
        self.emit_release_storage(ctx, storage, ty)
    }

    /// Whether `ty` is a stdlib-owned aggregate whose compiled destructor may
    /// release a discarded temporary: the destructor chain is
    /// stdlib-authored (pure frees, nothing user-observable).
    pub(super) fn stdlib_deinit_temp(&self, ty: &Ty) -> bool {
        let TyKind::Struct(name, _) = ty.kind() else {
            return false;
        };
        let template = name.split("$mono").next().unwrap_or(name);
        let stdlib = template.starts_with("__module$std$")
            || matches!(
                template,
                "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
            );
        stdlib
            && (self
                .lifecycle_symbol(name, "__deinit__")
                .is_some_and(|deinit| self.signatures.contains_key(deinit))
                || self.needs_drop(ty))
    }

    /// Recursively free the owned heap buffers inside `ty`-typed storage.
    pub(super) fn emit_release_storage(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        match ty.kind() {
            TyKind::Pointer { .. } => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            // A String's buffer and an error's message buffer both sit at
            // offset 0 (MjString/MjError agree on the data-first layout).
            TyKind::Error => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            TyKind::Struct(name, _) if mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            TyKind::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let fields = decl.fields.clone();
                let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
                let composed = self
                    .layout
                    .struct_layout(&field_tys)
                    .map_err(|error| self.unsupported(format!("release layout ({error})"), None))?;
                for (position, field_ty) in field_tys.iter().enumerate() {
                    // Raw Pointer fields are not intrinsically owned, and a
                    // user destructor is not run for a register temporary.
                    // Only recursively release storage whose type carries a
                    // backend-known owned heap value.
                    if !self.owns_heap(field_ty) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_release_storage(ctx, address, field_ty)?;
                }
                Ok(())
            }
            TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self
                    .layout
                    .struct_layout(&elements)
                    .map_err(|error| self.unsupported(format!("release layout ({error})"), None))?;
                for (position, element) in elements.iter().enumerate() {
                    if !self.owns_heap(element) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_release_storage(ctx, address, element)?;
                }
                Ok(())
            }
            TyKind::Variant(alternatives) => {
                let layout = self.layout.variant_layout(alternatives).map_err(|error| {
                    self.unsupported(format!("Variant drop layout ({error})"), None)
                })?;
                self.emit_drop_variant_payload(ctx, ptr, alternatives, layout.payload_offset, &[])
            }
            _ => Ok(()),
        }
    }

    /// `mjrt_free(ptr)`.
    pub(super) fn emit_free(&mut self, ctx: &mut Context, ptr: Value) {
        let free_ty = self.shared.ensure_rt(ctx, "mjrt_free");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_free".try_into().expect("valid identifier")),
            free_ty,
            vec![ptr],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// Load the `(data, size)` fields of nominal-String storage.
    pub(super) fn string_parts(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        dest: Reg,
    ) -> (Value, Value) {
        let ptr_handle = ScalarTy::Ptr.handle(ctx);
        let data = LoadOp::new(ctx, ptr, ptr_handle);
        self.append(ctx, data.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, ptr, 8, dest);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let size = LoadOp::new(ctx, size_address, i64_handle);
        self.append(ctx, size.get_operation(), Some(dest));
        (data.get_result(ctx), size.get_result(ctx))
    }

    /// Load the `cap` field of nominal-String storage.
    pub(super) fn string_cap(&mut self, ctx: &mut Context, ptr: Value, dest: Reg) -> Value {
        let cap_address = self.gep_byte(ctx, ptr, 16, dest);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let cap = LoadOp::new(ctx, cap_address, i64_handle);
        self.append(ctx, cap.get_operation(), Some(dest));
        cap.get_result(ctx)
    }

    /// Store `{data, size, cap}` into nominal-String storage.
    pub(super) fn store_string_fields(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        data: Value,
        size: Value,
        cap: Value,
        dest: Reg,
    ) {
        let data_store = StoreOp::new(ctx, data, storage);
        self.append(ctx, data_store.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, storage, 8, dest);
        let size_store = StoreOp::new(ctx, size, size_address);
        self.append(ctx, size_store.get_operation(), Some(dest));
        let cap_address = self.gep_byte(ctx, storage, 16, dest);
        let cap_store = StoreOp::new(ctx, cap, cap_address);
        self.append(ctx, cap_store.get_operation(), Some(dest));
    }

    /// Store `{_data, _size}` into `StringSpan` view storage (16 bytes: the
    /// pointer at 0, the byte length at 8).
    pub(super) fn store_string_span_fields(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        data: Value,
        size: Value,
        dest: Reg,
    ) {
        let data_store = StoreOp::new(ctx, data, storage);
        self.append(ctx, data_store.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, storage, 8, dest);
        let size_store = StoreOp::new(ctx, size, size_address);
        self.append(ctx, size_store.get_operation(), Some(dest));
    }

    /// `mjrt_alloc(size, align)` with a runtime byte count.
    pub(super) fn emit_alloc(
        &mut self,
        ctx: &mut Context,
        size: Value,
        align: u64,
        dest: Reg,
    ) -> Value {
        let alloc_ty = self.shared.ensure_rt(ctx, "mjrt_alloc");
        let align_value = self.uint_constant(ctx, align);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_alloc".try_into().expect("valid identifier")),
            alloc_ty,
            vec![size, align_value],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// `llvm.memset.p0.i64(dest, 0, len, volatile=false)`: zero storage.
    /// Droppable variable slots zero at entry so a flag-guarded drop or
    /// release path never reads undefined bytes, and the intrinsic use keeps
    /// mem2reg from promoting a slot whose stores sit in since-pruned blocks.
    pub(super) fn mem_zero(&mut self, ctx: &mut Context, dest: Value, len: u64) {
        if len == 0 {
            return;
        }
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, i8_ty, i64_ty, i1_ty], false);
        let i8_int = IntegerType::get(ctx, 8, Signedness::Signless);
        let zero_attr = IntegerAttr::new(i8_int, APInt::from_u64(0, bw(8)));
        let zero = ConstantOp::new(ctx, Box::new(zero_attr));
        self.append(ctx, zero.get_operation(), None);
        let len_value = self.uint_constant(ctx, len);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memset.p0.i64".to_string()),
            fn_ty,
            vec![dest, zero.get_result(ctx), len_value, volatile],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// `llvm.memcpy` with a runtime byte count.
    pub(super) fn mem_copy_dynamic(
        &mut self,
        ctx: &mut Context,
        dest_ptr: Value,
        src: Value,
        len: Value,
        span_reg: Reg,
    ) {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, ptr_ty, i64_ty, i1_ty], false);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memcpy.p0.p0.i64".to_string()),
            fn_ty,
            vec![dest_ptr, src, len, volatile],
        );
        self.append(ctx, call.get_operation(), Some(span_reg));
    }

    fn mark_temp(&mut self, dest: Reg, ty: Ty, reject_untracked: bool) -> Result<(), PlironError> {
        if !self.owns_heap(&ty)
            && !matches!(ty.kind(), TyKind::StringLiteral)
            && !self.stdlib_deinit_temp(&ty)
            && !self.needs_drop(&ty)
        {
            return Ok(());
        }
        let across = self.moved_out.across_regions.get(&dest.0);
        if across.is_some_and(|paths| paths.iter().any(|path| self.moved_out_matters(&ty, path))) {
            return Err(self.unsupported_reg(
                "field moved out of an owned temporary across a region".into(),
                dest,
            ));
        }
        let moved = self.moved_out_paths(dest.0, &ty);
        if !moved.is_empty() {
            if self.released_by_destructor(&ty) {
                return Err(self.unsupported_reg(
                    format!("field moved out of a `{ty}` temporary its destructor releases"),
                    dest,
                ));
            }
            if self.moved_out_covers(&ty, &mut Vec::new(), &moved) {
                return Ok(());
            }
        }
        match self.temp_release(dest) {
            Some(release) => {
                self.temp_releases.insert(dest.0, release);
            }
            None if reject_untracked => {
                return Err(self.unsupported_reg(
                    "owned heap-carrying temporary read outside the block defining it".into(),
                    dest,
                ));
            }
            None => return Ok(()),
        }
        if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
            eprintln!("TEMP-DBG {} mark %r{} {ty}", self.name, dest.0);
        }
        self.owned_temps.insert(dest.0, ty);
        Ok(())
    }

    /// Where `dest`, defined at the position being lowered, is released:
    /// its final read's position in the defining block. `None` when that
    /// read is not in it, or precedes the definition.
    fn temp_release(&self, dest: Reg) -> Option<TempRelease> {
        let position = *self.open_blocks.last()?;
        let Some(chain) = self.last_uses.get(&dest.0) else {
            return Some(TempRelease {
                block: position.key,
                after: None,
            });
        };
        chain
            .iter()
            .find(|(key, _)| *key == position.key)
            .filter(|(_, index)| *index >= position.index)
            .map(|(_, index)| TempRelease {
                block: position.key,
                after: Some(*index),
            })
    }

    /// The paths [`MovedOutFields`] records moved out of `reg`, a `ty`
    /// value, that hold something its release would free or destroy.
    fn moved_out_paths(&self, reg: u32, ty: &Ty) -> BTreeSet<Vec<String>> {
        self.moved_out
            .paths
            .get(&reg)
            .into_iter()
            .flatten()
            .filter(|path| self.moved_out_matters(ty, path))
            .cloned()
            .collect()
    }

    /// Whether the field at `path` in a `ty` value holds something a
    /// release frees or destroys; an unresolved path conservatively does.
    fn moved_out_matters(&self, ty: &Ty, path: &[String]) -> bool {
        self.field_path_ty(ty, path)
            .is_none_or(|field| self.owns_heap(&field) || self.needs_drop(&field))
    }

    /// The type of the field at `path` in a `ty` value.
    fn field_path_ty(&self, ty: &Ty, path: &[String]) -> Option<Ty> {
        path.iter().try_fold(ty.clone(), |ty, field| {
            let TyKind::Struct(name, _) = ty.kind() else {
                return None;
            };
            self.struct_decls
                .get(name.as_str())?
                .fields
                .iter()
                .find(|(declared, _)| declared == field)
                .map(|(_, field_ty)| field_ty.clone())
        })
    }

    /// Whether releasing a `ty` temporary runs its stdlib destructor rather
    /// than freeing its buffers. Collection temporaries own their backing
    /// allocation through their stdlib destructor; their raw pointer field
    /// is not itself a separately owned `Pointer` value. `MaybeUninit` is
    /// deliberately excluded: its trivial wrapper destructor must never
    /// reach a live user payload held in its compiler-private storage.
    fn released_by_destructor(&self, ty: &Ty) -> bool {
        let uninit_wrapper = matches!(ty.kind(), TyKind::Struct(name, _)
            if name.contains("MaybeUninit")
                || mojito_types::types::uninit_storage_element(ty).is_some());
        self.stdlib_deinit_temp(ty) && !self.owns_heap(ty) && !uninit_wrapper
    }

    /// Whether the moved-out paths `moved` leave nothing for the release of
    /// the `ty` value at `path` to free: the path itself moved out, a value
    /// owning no heap buffer, or a declared struct each of whose fields is
    /// covered.
    fn moved_out_covers(
        &self,
        ty: &Ty,
        path: &mut Vec<String>,
        moved: &BTreeSet<Vec<String>>,
    ) -> bool {
        if moved.contains(path) || !self.owns_heap(ty) {
            return true;
        }
        let Some(decl) = self.plain_struct_decl(ty) else {
            return false;
        };
        decl.fields.iter().all(|(field, field_ty)| {
            path.push(field.clone());
            let covered = self.moved_out_covers(field_ty, path, moved);
            path.pop();
            covered
        })
    }

    /// [`Self::emit_release_storage`] of the `ty` value at `path` in `ptr`,
    /// but for the fields `moved` names, which a consumer owns now.
    fn emit_release_storage_except(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        path: &mut Vec<String>,
        moved: &BTreeSet<Vec<String>>,
    ) -> Result<(), PlironError> {
        if moved.contains(path) || !self.owns_heap(ty) {
            return Ok(());
        }
        if !moved.iter().any(|taken| taken.starts_with(path)) {
            return self.emit_release_storage(ctx, ptr, ty);
        }
        let Some(decl) = self.plain_struct_decl(ty) else {
            return Err(self.unsupported(
                format!("a field moved out of part of an owned `{ty}` temporary"),
                None,
            ));
        };
        let fields = decl.fields.clone();
        let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
        let composed = self
            .layout
            .struct_layout(&field_tys)
            .map_err(|error| self.unsupported(format!("release layout ({error})"), None))?;
        for ((field, field_ty), offset) in fields.iter().zip(&composed.offsets) {
            let address = if *offset == 0 {
                ptr
            } else {
                self.gep_byte_unspanned(ctx, ptr, *offset)
            };
            path.push(field.clone());
            let released = self.emit_release_storage_except(ctx, address, field_ty, path, moved);
            path.pop();
            released?;
        }
        Ok(())
    }

    /// The declaration of `ty` when it is a declared struct other than the
    /// nominal String, whose release walks its fields.
    fn plain_struct_decl(&self, ty: &Ty) -> Option<&'a MirStructDeclaration> {
        match ty.kind() {
            TyKind::Struct(name, _) if !mojito_symbol::symbol::is_stdlib_string_struct(name) => {
                self.struct_decls.get(name.as_str()).copied()
            }
            _ => None,
        }
    }
}
