//! Constructor and `String` constructor/builtin lowering, plus
//! `MakeRef`/`ReadRef`/`WriteRef`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{TyKind, erase_origin_arguments};

impl FnLowering<'_> {
    /// A call naming the declared struct `name`, which names no constructor:
    /// fieldwise per-field stores, as the VM builds it.
    pub(super) fn lower_constructor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        let struct_ty = match self.func.reg_types.get(&dest.0) {
            Some(ty) if matches!(ty.kind(), TyKind::Struct(..)) => ty.clone(),
            _ => TyKind::Struct(name.to_string(), Vec::new()).into(),
        };
        let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
        let LowerTy::Aggregate { layout, .. } = lowered else {
            return Err(self.unsupported_reg(format!("constructor for `{name}`"), dest));
        };
        let decl = self.struct_decls[name];
        if !decl.fieldwise_init {
            return Err(self.unsupported_reg(
                format!("a call naming `{name}`, which names no constructor and is not fieldwise"),
                dest,
            ));
        }
        if !kwargs.is_empty() {
            return Err(self.unsupported_reg(
                format!("keyword arguments in the fieldwise constructor of `{name}`"),
                dest,
            ));
        }
        if args.len() != decl.fields.len() {
            return Err(self.unsupported_reg(
                format!(
                    "fieldwise constructor of `{name}` expects {} arguments, got {}",
                    decl.fields.len(),
                    args.len()
                ),
                dest,
            ));
        }
        let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, t)| t.clone()).collect();
        self.store_fields(ctx, dest, layout, &field_tys, args)
    }

    /// A `struct.create`: fresh storage of `dest`'s struct type with each
    /// field operand stored at its offset, as a fieldwise constructor
    /// builds it.
    pub(super) fn lower_struct_create(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        fields: &[Reg],
    ) -> Result<(), PlironError> {
        let Some(struct_ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped struct creation".into(), dest));
        };
        let TyKind::Struct(name, _) = struct_ty.kind() else {
            return Err(self.unsupported_reg(format!("struct creation of `{struct_ty}`"), dest));
        };
        let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
        let (LowerTy::Aggregate { layout, .. }, Some(decl)) =
            (lowered, self.struct_decls.get(name.as_str()))
        else {
            return Err(self.unsupported_reg(format!("struct creation of `{name}`"), dest));
        };
        let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, t)| t.clone()).collect();
        self.store_fields(ctx, dest, layout, &field_tys, fields)
    }

    /// Fresh `layout` storage for `dest`, each of `values` stored at the
    /// offset of its field of type `field_tys`.
    fn store_fields(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        layout: Layout,
        field_tys: &[Ty],
        values: &[Reg],
    ) -> Result<(), PlironError> {
        let composed = self.struct_layout_of(field_tys, dest)?;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for ((value, field_ty), offset) in values.iter().zip(field_tys).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, field_ty, *value)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// The `String(x)` builtin — the VM's `format_value` over one argument:
    /// a literal stays compile-time; scalars format through `mjrt_fmt_*`
    /// into a dedicated allocation (an owned runtime string); a nominal
    /// String reads back as a borrowed runtime string.
    pub(super) fn lower_string_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("String conversion contract".into(), dest));
        }
        let arg = args[0];
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.str_consts.insert(dest.0, bytes);
            return Ok(());
        }
        if let Some(ty) = self.func.reg_types.get(&arg.0)
            && let TyKind::Struct(name, _) = ty.kind()
            && mojito_symbol::symbol::is_stdlib_string_struct(name)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            return self.borrow_or_adopt_string(ctx, arg, dest, data, len);
        }
        // An error value converts to its bare message (the VM's
        // `format_value` over `Value::Error`), borrowed from the error.
        if matches!(
            self.func.reg_types.get(&arg.0).map(Ty::kind),
            Some(TyKind::Error)
        ) {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            return self.borrow_or_adopt_string(ctx, arg, dest, data, len);
        }
        // A runtime StringLiteral value reads back as a borrowed string.
        if matches!(
            self.func.reg_types.get(&arg.0).map(Ty::kind),
            Some(TyKind::StringLiteral)
        ) && !self.pending_literals.contains_key(&arg.0)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            return self.borrow_or_adopt_string(ctx, arg, dest, data, len);
        }
        // A nominal struct converts through its `write_to` conformance over
        // a fresh accumulator — the VM's `format_value` struct arm. The
        // accumulated buffer transfers into the resulting owned string.
        if let Some(TyKind::Struct(name, _)) = (self.func.reg_types.get(&arg.0).cloned())
            .as_ref()
            .map(Ty::kind)
            && !mojito_symbol::symbol::is_stdlib_string_struct(name)
        {
            let writer = self.entry_alloca(ctx, 16, 8);
            self.mem_zero(ctx, writer, 16);
            self.append_struct_via_write_to(ctx, arg, name, writer, dest)?;
            let (data, len) = self.string_parts(ctx, writer, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            // The accumulator alloca is exactly the 16-byte `MjStrDesc`
            // StringLiteral storage; aggregate consumers read it directly.
            self.reg_values.insert(dest.0, writer);
            self.mark_owned_temp(dest, Ty::string_literal())?;
            return Ok(());
        }
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A runtime FloatLiteral value rejects — the VM formats its
            // exact rational, which f64 storage cannot reproduce.
            None => match self.func.reg_types.get(&arg.0).map(Ty::kind) {
                Some(TyKind::FloatLiteral) => {
                    if !self.pending_literals.contains_key(&arg.0) {
                        return Err(self.unsupported_reg(
                            "String conversion of a runtime FloatLiteral value".into(),
                            dest,
                        ));
                    }
                    ScalarTy::Float64
                }
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, arg, ty)?;
        let (text, len) = self.format_scalar(ctx, ty, value, dest)?;
        let data = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, data, text, len, dest);
        self.str_runtime.insert(
            dest.0,
            RuntimeStr {
                data,
                len,
                owned: true,
            },
        );
        self.mark_owned_temp(dest, Ty::string_literal())?;
        Ok(())
    }

    /// Record `dest` as the `(data, len)` string the `String(arg)` builtin
    /// reads out of `arg`. A borrowed descriptor aliases `arg`'s buffer, so
    /// when `arg` is an owned register temporary — released right after this
    /// instruction, its final use — the buffer transfers to `dest` instead
    /// (owned, released after `dest`'s own final use, or stolen by a String
    /// constructor).
    ///
    /// A register that aliases a *place*, or snapshots one
    /// ([`FnLowering::load_from`]), copies out instead. MIR types this
    /// builtin's result as a fresh owned string carrying no loans, so the
    /// ownership analysis reads the call as the place's last use and drops it
    /// between this instruction and the constructor that reads the
    /// descriptor. `String` is the one borrow-classified builtin
    /// (`support::borrowed_operands`) whose result outlives its call, so it is
    /// the one that cannot leave the descriptor pointing into that place.
    fn borrow_or_adopt_string(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
        data: Value,
        len: Value,
    ) -> Result<(), PlironError> {
        let mut data = data;
        let mut owned = self.owned_temps.remove(&arg.0).is_some();
        if !owned
            && (self.aliased_load_regs.contains(&arg.0) || self.storage_reads.contains(&arg.0))
        {
            let copy = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, copy, data, len, dest);
            data = copy;
            owned = true;
        }
        self.str_runtime
            .insert(dest.0, RuntimeStr { data, len, owned });
        if owned {
            self.mark_owned_temp(dest, Ty::string_literal())?;
        }
        Ok(())
    }

    /// The `Error(x)` builtin. Before Stage 4's tagged outcomes the only
    /// consumer is a `raise`, which reads the message bytes and exits — so an
    /// error value lowers as its message string pair.
    /// The lowered function-return value kind: the reference pointer for a
    /// reference-returning function, else the checked return type's lowering.
    pub(super) fn return_value_lower(&self) -> Result<Option<LowerTy>, PlironError> {
        if self.func.returns_reference {
            return Ok(Some(LowerTy::Scalar(ScalarTy::Ptr)));
        }
        match self.func.ret_ty.as_ref() {
            Some(ty) if *ty != TyKind::None => {
                Ok(Some(lower_ty(self.name, ty, &self.layout, None)?))
            }
            _ => Ok(None),
        }
    }

    /// `MakeRef`: materialize a reference to a verified place — its address.
    /// A place through a local reference forwards (and extends) the stored
    /// handle.
    pub(super) fn lower_make_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
    ) -> Result<(), PlironError> {
        let place = place.clone();
        if matches!(place.proj.last(), Some(Proj::UninitPayload)) {
            let mut storage_place = place.clone();
            storage_place.proj.pop();
            let (storage, _) = self.place_address(ctx, &storage_place, dest)?;
            self.guard_uninit_present(ctx, storage, TrapCategory::UninitRead, dest)?;
        }
        let (address, ty) = self.place_address(ctx, &place, dest)?;
        // A bare reference-typed variable re-borrows: its slot stores a
        // handle (reference slots always hold real referent addresses), and
        // the made reference is that stored handle, collapsing the chain
        // like the VM's recursive `Value::Ref` reads. A projected place
        // whose designated element is itself a reference (a `List[ref T]`
        // element) instead addresses the slot — its consumers dereference
        // explicitly.
        // A projected place ENDING at a stored reference also forwards when
        // the destination is typed as the stored handle itself (`ref s =
        // self.src` reborrows): the ref-field slot holds a real referent
        // address, so the handle is the loaded slot value, not the slot
        // address. A storage-borrow destination (`ref (ref T)`) keeps the
        // slot address; its consumers dereference explicitly.
        let forwards_stored_handle = !place.proj.is_empty()
            && match (ty.kind(), self.func.reg_types.get(&dest.0).map(Ty::kind)) {
                (TyKind::Ref(stored), Some(TyKind::Ref(dest_ref))) => {
                    dest_ref.referent == stored.referent
                }
                _ => false,
            };
        if (place.proj.is_empty() || forwards_stored_handle)
            && let TyKind::Ref(reference) = ty.kind()
        {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            if matches!(reference.referent.kind(), TyKind::Pointer { .. }) {
                self.pointer_slot_refs.insert(dest.0);
            }
            self.reg_values.insert(dest.0, load.get_result(ctx));
            return Ok(());
        }
        // A reference typed as one to the pointer itself, whatever origin
        // either spells, borrows its storage: a read of it is the pointer,
        // not its pointee.
        let borrows_pointer = match (self.func.reg_types.get(&dest.0).map(Ty::kind), ty.kind()) {
            (Some(TyKind::Ref(reference)), TyKind::Pointer { element, .. }) => matches!(
                reference.referent.kind(),
                TyKind::Pointer { element: referent, .. }
                    if erase_origin_arguments(referent) == erase_origin_arguments(element)
            ),
            _ => false,
        };
        if matches!(ty.kind(), TyKind::Pointer { .. }) && !borrows_pointer {
            self.pointer_slot_refs.insert(dest.0);
        }
        self.reg_values.insert(dest.0, address);
        Ok(())
    }

    /// `ReadRef`: read the referent behind a handle. The read itself is a
    /// plain structural snapshot; an explicit following `CopyValue` owns
    /// user-visible copy construction. Keeping those operations distinct
    /// prevents reference adapters from invoking `__copyinit__` twice.
    pub(super) fn lower_read_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        reference: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // A handle addressing pointer-typed storage dereferences through the
        // stored pointer (the VM's reference-pointer boundary).
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference read".into(), dest));
        };
        self.load_moved_from(ctx, pointer, &ty, dest)
    }

    /// `WriteRef`: write a value through a handle into the referent storage.
    pub(super) fn lower_write_ref(
        &mut self,
        ctx: &mut Context,
        reference: Reg,
        value: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // See `lower_read_ref`: a handle addressing pointer-typed storage
        // dereferences through the stored pointer.
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(value));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference write".into(), value));
        };
        self.store_to(ctx, pointer, &ty, value)
    }
}
