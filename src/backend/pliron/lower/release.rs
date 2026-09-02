//! Ownership release machinery: owned-temp tracking, release/free
//! emission, and string field/alloc helpers.

use super::*;

impl<'a> FnLowering<'a> {
    /// Whether the invisible-release rule can free every heap buffer a value
    /// of `ty` owns without running user code: the nominal String (one
    /// buffer), and byte-copied aggregates over such fields.
    pub(super) fn releasable(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Error => true,
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => true,
            Ty::Struct(name, _) => {
                !self
                    .declarations
                    .contains_key(&format!("{name}.__copyinit__"))
                    && self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                        decl.fields
                            .iter()
                            .all(|(_, field)| !self.owns_heap(field) || self.releasable(field))
                    })
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| !self.owns_heap(element) || self.releasable(element)),
            Ty::Variant(alternatives) => alternatives
                .iter()
                .all(|alternative| !self.owns_heap(alternative) || self.releasable(alternative)),
            _ => !self.owns_heap(ty),
        }
    }

    /// Whether a value of `ty` semantically owns heap memory (the nominal
    /// String's buffer; raw pointers are not owned).
    pub(super) fn owns_heap(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Error => true,
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => true,
            Ty::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.owns_heap(field))),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().any(|element| self.owns_heap(element))
            }
            Ty::Variant(alternatives) => alternatives
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
    /// final use in this block. A temporary whose final use sits in another
    /// block would need liveness analysis — reject instead of leaking.
    pub(super) fn mark_owned_temp(&mut self, dest: Reg, ty: Ty) -> Result<(), PlironError> {
        if !self.owns_heap(&ty)
            && !matches!(ty, Ty::StringLiteral)
            && !self.stdlib_deinit_temp(&ty)
            && !self.needs_drop(&ty)
        {
            return Ok(());
        }
        if let Some((block, _)) = self.last_uses.get(&dest.0)
            && *block != self.position.0
        {
            return Err(self.unsupported_reg(
                "owned heap-carrying temporary used across blocks".into(),
                dest,
            ));
        }
        if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
            eprintln!("TEMP-DBG {} mark %r{} {ty}", self.name, dest.0);
        }
        self.owned_temps.insert(dest.0, ty);
        Ok(())
    }

    /// Release every owned temporary whose final use was the instruction just
    /// lowered (or that is never used at all).
    pub(super) fn flush_owned_temps(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        let due: Vec<(u32, Ty)> = self
            .owned_temps
            .iter()
            .filter(|(reg, _)| match self.last_uses.get(reg) {
                None => true,
                Some(last) => *last == self.position,
            })
            .map(|(reg, ty)| (*reg, ty.clone()))
            .collect();
        for (reg, ty) in due {
            self.owned_temps.remove(&reg);
            if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
                eprintln!("TEMP-DBG {} release %r{} {ty}", self.name, reg);
            }
            self.emit_release_reg(ctx, reg, &ty)?;
        }
        Ok(())
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
        if matches!(ty, Ty::StringLiteral) {
            let Some(descriptor) = self.str_runtime.get(&reg).copied() else {
                return Ok(());
            };
            self.emit_free(ctx, descriptor.data);
            return Ok(());
        }
        let Some(storage) = self.reg_values.get(&reg).copied() else {
            return Ok(());
        };
        // Collection temporaries own their backing allocation through their
        // stdlib destructor; their raw pointer field is not itself a
        // separately owned Pointer value. MaybeUninit is deliberately
        // excluded: its trivial wrapper destructor must never reach a live
        // user payload held in its compiler-private storage.
        let uninit_wrapper = matches!(ty, Ty::Struct(name, _)
            if name.contains("MaybeUninit")
                || crate::types::uninit_storage_element(ty).is_some());
        if self.stdlib_deinit_temp(ty) && !self.owns_heap(ty) && !uninit_wrapper {
            let traced = self.trace_lifecycle;
            self.trace_lifecycle = false;
            let released = self.emit_drop_value(ctx, storage, ty, false);
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
        let Ty::Struct(name, _) = ty else {
            return false;
        };
        let template = name.split("$mono").next().unwrap_or(name);
        let stdlib = template.starts_with("__module$std$")
            || matches!(
                template,
                "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
            );
        stdlib
            && (self.signatures.contains_key(&format!("{name}.__deinit__")) || self.needs_drop(ty))
    }

    /// Recursively free the owned heap buffers inside `ty`-typed storage.
    pub(super) fn emit_release_storage(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        match ty {
            Ty::Pointer { .. } => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            // A String's buffer and an error's message buffer both sit at
            // offset 0 (MjString/MjError agree on the data-first layout).
            Ty::Error => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            Ty::Struct(name, _) => {
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
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
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
            Ty::Variant(alternatives) => {
                let layout = self.layout.variant_layout(alternatives).map_err(|error| {
                    self.unsupported(format!("Variant drop layout ({error})"), None)
                })?;
                self.emit_drop_variant_payload(ctx, ptr, alternatives, layout.payload_offset)
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
}
