//! Release machinery: heap-buffer free emission and string field/alloc
//! helpers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

/// Emit, through `append`, the `(data, size)` of the nominal String (or the
/// message of the `MjError`) at `ptr`, as the library's `unsafe_ptr` and
/// `byte_length` read them: an inline String
/// (`MJ_STRING_FLAG_IS_INLINE` in `cap`) holds its bytes at `ptr` itself and
/// its length in `cap`'s top byte; a static or owned one points at them.
/// Branch-free, so the executable wrapper's error report shares it.
pub(super) fn emit_nominal_string_parts(
    ctx: &mut Context,
    ptr: Value,
    mut append: impl FnMut(&Context, Ptr<Operation>),
) -> (Value, Value) {
    use mojito_native::native::rt_abi::{MJ_STRING_FLAG_IS_INLINE, MJ_STRING_INLINE_LENGTH_START};
    let ptr_handle = ScalarTy::PTR.handle(ctx);
    let i8_handle: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
    let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
    let i64_handle: TypeHandle = i64_ty.into();
    let mut constant = |ctx: &mut Context, value: i64| {
        let attr = IntegerAttr::new(i64_ty, APInt::from_u64(value as u64, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        append(ctx, op.get_operation());
        op.get_result(ctx)
    };
    let flag = constant(ctx, MJ_STRING_FLAG_IS_INLINE);
    let zero = constant(ctx, 0);
    let shift = constant(ctx, MJ_STRING_INLINE_LENGTH_START);
    let mask = constant(ctx, 0b1_1111);
    let mut word = |ctx: &mut Context, offset: u32, handle: TypeHandle| {
        let address = if offset == 0 {
            ptr
        } else {
            let gep = GetElementPtrOp::new(ctx, ptr, vec![GepIndex::Constant(offset)], i8_handle);
            append(ctx, gep.get_operation());
            gep.get_result(ctx)
        };
        let load = LoadOp::new(ctx, address, handle);
        append(ctx, load.get_operation());
        load.get_result(ctx)
    };
    let data = word(ctx, 0, ptr_handle);
    let size = word(ctx, 8, i64_handle);
    let cap = word(ctx, 16, i64_handle);
    let flagged = AndOp::new(ctx, cap, flag);
    append(ctx, flagged.get_operation());
    let inline = ICmpOp::new(ctx, ICmpPredicateAttr::NE, flagged.get_result(ctx), zero);
    append(ctx, inline.get_operation());
    let shifted = LShrOp::new(ctx, cap, shift);
    append(ctx, shifted.get_operation());
    let length = AndOp::new(ctx, shifted.get_result(ctx), mask);
    append(ctx, length.get_operation());
    let data = SelectOp::new(ctx, inline.get_result(ctx), ptr, data);
    append(ctx, data.get_operation());
    let size = SelectOp::new(ctx, inline.get_result(ctx), length.get_result(ctx), size);
    append(ctx, size.get_operation());
    (data.get_result(ctx), size.get_result(ctx))
}

impl FnLowering<'_> {
    /// Whether a value of `ty` semantically owns heap memory (the nominal
    /// String's buffer; raw pointers are not owned).
    pub(super) fn owns_heap(&self, ty: &Ty) -> bool {
        match ty.kind() {
            kind if kind.is_error() => true,
            struct_ty
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                true
            }
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => self
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

    /// Recursively free the owned heap buffers inside `ty`-typed storage.
    pub(super) fn emit_release_storage(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        match ty.kind() {
            pointer if pointer.is_pointer_layout() => {
                let handle = ScalarTy::PTR.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            // An error is its message String (MjError is one MjString).
            kind if kind.is_error() => {
                self.emit_string_buffer_release(ctx, ptr);
                Ok(())
            }
            struct_ty
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                self.emit_string_buffer_release(ctx, ptr);
                Ok(())
            }
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
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

    /// The `(data, size)` of the nominal String (or `MjError`) at `ptr`, in
    /// any of its forms ([`emit_nominal_string_parts`]).
    pub(super) fn nominal_string_parts(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        dest: Reg,
    ) -> (Value, Value) {
        emit_nominal_string_parts(ctx, ptr, |ctx, op| self.append(ctx, op, Some(dest)))
    }

    /// Load the `{data, size}` leading words of a `StringSpan` or an
    /// `MjStrDesc` at `ptr`. A nominal String reads through
    /// [`Self::nominal_string_parts`], since an inline one's first word is
    /// text.
    pub(super) fn string_parts(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        dest: Reg,
    ) -> (Value, Value) {
        let ptr_handle = ScalarTy::PTR.handle(ctx);
        let data = LoadOp::new(ctx, ptr, ptr_handle);
        self.append(ctx, data.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, ptr, 8, dest);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let size = LoadOp::new(ctx, size_address, i64_handle);
        self.append(ctx, size.get_operation(), Some(dest));
        (data.get_result(ctx), size.get_result(ctx))
    }

    /// Free the buffer of the nominal String (or the message of the
    /// `MjError`) at `ptr` when the String owns it, as the library's
    /// `_drop_ref`: a static or inline String's capacity field lacks
    /// `FLAG_IS_REF_COUNTED` (an inline String's first word is text, never
    /// freed), and `mjrt_free` of null is a no-op.
    pub(super) fn emit_string_buffer_release(&mut self, ctx: &mut Context, ptr: Value) {
        let ptr_handle = ScalarTy::PTR.handle(ctx);
        let data = LoadOp::new(ctx, ptr, ptr_handle);
        self.append(ctx, data.get_operation(), None);
        let field_address = self.gep_byte_unspanned(ctx, ptr, 16);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let field = LoadOp::new(ctx, field_address, i64_handle);
        self.append(ctx, field.get_operation(), None);
        let owned = self.string_owns_buffer(ctx, field.get_result(ctx), None);
        let null = ZeroOp::new(ctx, ptr_handle);
        self.append(ctx, null.get_operation(), None);
        let target = SelectOp::new(ctx, owned, data.get_result(ctx), null.get_result(ctx));
        self.append(ctx, target.get_operation(), None);
        self.emit_free(ctx, target.get_result(ctx));
    }

    /// Store the static form of a nominal String over the `len` bytes at
    /// `data`, which no String frees or writes: a capacity field of 0.
    pub(super) fn store_static_string(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        data: Value,
        len: Value,
        dest: Reg,
    ) {
        let field = self.int_constant(ctx, 0);
        self.store_string_fields(ctx, storage, data, len, field, dest);
    }

    /// Store a nominal String holding a copy of the `len` bytes at `src`, as
    /// the library builds a String of that length: up to `INLINE_CAPACITY`
    /// bytes inline, in the storage's own 24 bytes with `FLAG_IS_INLINE` and
    /// the length in the capacity field's top byte; a longer text owned, as
    /// the library's `_alloc` builds one, a fresh buffer of `len` rounded up
    /// to 8 bytes whose 8-byte units, flagged `FLAG_IS_REF_COUNTED`, are the
    /// capacity field.
    pub(super) fn store_owned_string_copy(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        src: Value,
        len: Value,
        dest: Reg,
    ) {
        use mojito_native::native::rt_abi::{
            MJ_STRING_FLAG_IS_INLINE, MJ_STRING_FLAG_IS_REF_COUNTED, MJ_STRING_INLINE_CAPACITY,
            MJ_STRING_INLINE_LENGTH_START,
        };
        let region = self.region.expect("lowering is inside a function");
        let inline_block = BasicBlock::new(ctx, None, vec![]);
        inline_block.insert_at_back(region, ctx);
        let owned_block = BasicBlock::new(ctx, None, vec![]);
        owned_block.insert_at_back(region, ctx);
        let join = BasicBlock::new(ctx, None, vec![]);
        join.insert_at_back(region, ctx);
        let capacity = self.int_constant(ctx, MJ_STRING_INLINE_CAPACITY);
        let fits = ICmpOp::new(ctx, ICmpPredicateAttr::ULE, len, capacity);
        self.append(ctx, fits.get_operation(), Some(dest));
        let branch = CondBrOp::new(
            ctx,
            fits.get_result(ctx),
            inline_block,
            vec![],
            owned_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));

        self.current = Some(inline_block);
        let zero = self.int_constant(ctx, 0);
        let null = ZeroOp::new(ctx, ScalarTy::PTR.handle(ctx));
        self.append(ctx, null.get_operation(), Some(dest));
        self.store_string_fields(ctx, storage, null.get_result(ctx), zero, zero, dest);
        self.mem_copy_dynamic(ctx, storage, src, len, dest);
        let shift = self.int_constant(ctx, MJ_STRING_INLINE_LENGTH_START);
        let length = ShlOp::new_with_overflow_flag(ctx, len, shift, no_overflow_flags());
        self.append(ctx, length.get_operation(), Some(dest));
        let flag = self.int_constant(ctx, MJ_STRING_FLAG_IS_INLINE);
        let field = OrOp::new(ctx, length.get_result(ctx), flag);
        self.append(ctx, field.get_operation(), Some(dest));
        let cap_address = self.gep_byte(ctx, storage, 16, dest);
        let cap_store = StoreOp::new(ctx, field.get_result(ctx), cap_address);
        self.append(ctx, cap_store.get_operation(), Some(dest));
        let done = BrOp::new(ctx, join, vec![]);
        self.append(ctx, done.get_operation(), Some(dest));

        self.current = Some(owned_block);
        let seven = self.int_constant(ctx, 7);
        let rounded = AddOp::new_with_overflow_flag(ctx, len, seven, no_overflow_flags());
        self.append(ctx, rounded.get_operation(), Some(dest));
        let three = self.int_constant(ctx, 3);
        let units = LShrOp::new(ctx, rounded.get_result(ctx), three);
        self.append(ctx, units.get_operation(), Some(dest));
        let bytes =
            ShlOp::new_with_overflow_flag(ctx, units.get_result(ctx), three, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let data = self.emit_alloc(ctx, bytes.get_result(ctx), 1, dest);
        self.mem_copy_dynamic(ctx, data, src, len, dest);
        let flag = self.int_constant(ctx, MJ_STRING_FLAG_IS_REF_COUNTED);
        let field = OrOp::new(ctx, units.get_result(ctx), flag);
        self.append(ctx, field.get_operation(), Some(dest));
        self.store_string_fields(ctx, storage, data, len, field.get_result(ctx), dest);
        let done = BrOp::new(ctx, join, vec![]);
        self.append(ctx, done.get_operation(), Some(dest));

        self.current = Some(join);
    }

    /// Whether the nominal-String capacity `field` marks an owned buffer
    /// (`FLAG_IS_REF_COUNTED` set), as an `i1`.
    pub(super) fn string_owns_buffer(
        &mut self,
        ctx: &mut Context,
        field: Value,
        dest: Option<Reg>,
    ) -> Value {
        let flag = self.int_constant(
            ctx,
            mojito_native::native::rt_abi::MJ_STRING_FLAG_IS_REF_COUNTED,
        );
        let masked = AndOp::new(ctx, field, flag);
        self.append(ctx, masked.get_operation(), dest);
        let zero = self.int_constant(ctx, 0);
        let owned = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
        self.append(ctx, owned.get_operation(), dest);
        owned.get_result(ctx)
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

    /// `llvm.memset.p{d}.i64(dest, 0, len, volatile=false)`: zero storage.
    /// Droppable variable slots zero at entry so a flag-guarded drop or
    /// release path never reads undefined bytes, and the intrinsic use keeps
    /// mem2reg from promoting a slot whose stores sit in since-pruned blocks.
    pub(super) fn mem_zero(&mut self, ctx: &mut Context, dest: Value, len: u64) {
        if len == 0 {
            return;
        }
        let ptr_ty = dest.get_type(ctx);
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
            StringAttr::new(format!(
                "llvm.memset.p{}.i64",
                value_address_space(ctx, dest)
            )),
            fn_ty,
            vec![dest, zero.get_result(ctx), len_value, volatile],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// `llvm.memcpy` with a runtime byte count, its overload named by each
    /// operand's address space.
    pub(super) fn mem_copy_dynamic(
        &mut self,
        ctx: &mut Context,
        dest_ptr: Value,
        src: Value,
        len: Value,
        span_reg: Reg,
    ) {
        let (dest_ty, src_ty) = (dest_ptr.get_type(ctx), src.get_type(ctx));
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![dest_ty, src_ty, i64_ty, i1_ty], false);
        let volatile = self.bool_constant(ctx, false);
        let name = format!(
            "llvm.memcpy.p{}.p{}.i64",
            value_address_space(ctx, dest_ptr),
            value_address_space(ctx, src)
        );
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new(name),
            fn_ty,
            vec![dest_ptr, src, len, volatile],
        );
        self.append(ctx, call.get_operation(), Some(span_reg));
    }
}
