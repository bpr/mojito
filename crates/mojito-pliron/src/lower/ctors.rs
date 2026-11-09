//! Constructor and `String` constructor/builtin lowering, plus
//! `MakeRef`/`ReadRef`/`WriteRef`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{TyKind, erase_origin_arguments};

impl FnLowering<'_> {
    /// A `struct.create`: fresh storage of `dest`'s struct type with each
    /// field operand stored at its offset.
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
        let Some((decl, _)) = self.mir_declarations.struct_instance(&struct_ty) else {
            return Err(self.unsupported_reg(format!("struct creation of `{name}`"), dest));
        };
        let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, t)| t.clone()).collect();
        let held = self.mir_declarations.register_field(&decl.name).is_some();
        match lower_ty(
            self.name,
            &struct_ty,
            &self.layout,
            self.mir_declarations,
            self.reg_span(dest),
        )? {
            // A held struct is its one field, so creating it is the identity,
            // as upstream's `LowerKGENToLLVM` lowers `kgen.struct.create`
            // of a one-element register-passable struct.
            LowerTy::Scalar(scalar) if held => {
                let value = self.reg_value(ctx, fields[0], scalar)?;
                self.reg_values.insert(dest.0, value);
                Ok(())
            }
            // A struct of no storage (a held one's erased field, or a
            // numeric literal's no fields) is the erased register, as
            // upstream's `kgen.struct.create` of no fields is no storage.
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                self.store_fields(ctx, dest, layout, &field_tys, fields)
            }
            LowerTy::Scalar(_) => {
                Err(self.unsupported_reg(format!("struct creation of `{name}`"), dest))
            }
        }
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
            Some(ty) if *ty != TyKind::None && *ty != TyKind::Never => Ok(Some(lower_ty(
                self.name,
                ty,
                &self.layout,
                self.mir_declarations,
                None,
            )?)),
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
            self.made_ref_places.insert(
                dest.0,
                MirPlace {
                    ty: Some(reference.referent.clone()),
                    ..place.clone()
                },
            );
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
        self.made_ref_places.insert(
            dest.0,
            MirPlace {
                ty: Some(ty),
                ..place
            },
        );
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
