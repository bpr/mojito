//! Place addressing: `place_address`, field offsets/layout, `GetField`,
//! tuple construction, and the `!pop.array` ops.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Const;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// Resolve a place to the address and checked type of its designated
    /// storage: the root variable slot plus statically composed field and
    /// tuple-element offsets from the shared layout engine. A pointer
    /// subscript projection loads the pointer value and continues at
    /// `pointer + index * sizeof(element)` — the VM's unchecked heap
    /// addressing.
    pub(super) fn place_address(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        dest: Reg,
    ) -> Result<(Value, Ty), PlironError> {
        let root_ty = self
            .func
            .var_tys
            .get(&place.root)
            .cloned()
            .or_else(|| place.root_ty.clone())
            .ok_or_else(|| {
                self.unsupported_reg(format!("untyped place root ${}", place.root), dest)
            })?;
        let root_slot = self
            .var_slots
            .get(place.root as usize)
            .copied()
            .ok_or_else(|| {
                self.unsupported_reg(format!("place root ${} out of range", place.root), dest)
            })?;
        // A place through a local reference designates the referent behind
        // the handle stored in the reference's slot. A static `ref` binding's
        // handle already addresses its loan place, so only the projections
        // beyond that place apply on top of it; a reference-result handle
        // (no recorded place) designates the root's referent and takes the
        // whole projection.
        let through_var = place.through.unwrap_or(place.root);
        let recorded_prefix = place
            .through
            .filter(|through| *through != place.root)
            .and_then(|through| self.reference_places.get(&through))
            .filter(|recorded| {
                recorded.root == place.root
                    && recorded.ty.is_some()
                    && recorded.proj.len() <= place.proj.len()
                    && recorded
                        .proj
                        .iter()
                        .zip(&place.proj)
                        .all(|(a, b)| projections_agree(a, b))
            })
            .map(|recorded| {
                (
                    recorded.ty.clone().expect("recorded place is typed"),
                    recorded.proj.len(),
                )
            });
        let through_slot = self
            .var_slots
            .get(through_var as usize)
            .copied()
            .ok_or_else(|| {
                self.unsupported_reg(format!("place handle ${through_var} out of range"), dest)
            })?;
        let through_ty = self
            .func
            .var_tys
            .get(&through_var)
            .cloned()
            .unwrap_or_else(|| root_ty.clone());
        let designated_ty = match root_ty.kind() {
            TyKind::Ref(reference) => reference.referent.clone(),
            _ => root_ty.clone(),
        };
        let ref_param_root = (through_var as usize) < self.func.n_params
            && self
                .func
                .ref_params
                .get(through_var as usize)
                .copied()
                .unwrap_or(false);
        let (mut ty, mut address, projected) =
            if let Some((recorded_ty, projected)) = recorded_prefix {
                let handle = ScalarTy::Ptr.handle(ctx);
                let load = LoadOp::new(ctx, through_slot, handle);
                self.append(ctx, load.get_operation(), Some(dest));
                (recorded_ty, load.get_result(ctx), projected)
            } else if place.through.is_some() || matches!(root_ty.kind(), TyKind::Ref(_)) {
                let (ty, address) = match through_ty.kind() {
                    TyKind::Ref(_) | TyKind::Pointer { .. } => {
                        let handle = ScalarTy::Ptr.handle(ctx);
                        let load = LoadOp::new(ctx, through_slot, handle);
                        self.append(ctx, load.get_operation(), Some(dest));
                        (designated_ty, load.get_result(ctx))
                    }
                    // A `mut`/`ref` parameter is typed as its referent and its
                    // aliased slot already IS the referent address.
                    _ if ref_param_root => (designated_ty, through_slot),
                    _ => {
                        return Err(self.unsupported_reg(
                            format!("place through non-reference handle `{through_ty}`"),
                            dest,
                        ));
                    }
                };
                (ty, address, 0)
            } else {
                (root_ty, root_slot, 0)
            };
        let mut offset: u64 = 0;
        for (step, proj) in place.proj.iter().enumerate().skip(projected) {
            while let TyKind::Ref(reference) = ty.kind() {
                if offset != 0 {
                    address = self.gep_byte(ctx, address, offset, dest);
                    offset = 0;
                }
                let ptr_ty = ScalarTy::Ptr.handle(ctx);
                let load = LoadOp::new(ctx, address, ptr_ty);
                self.append(ctx, load.get_operation(), Some(dest));
                address = load.get_result(ctx);
                ty = reference.referent.clone();
            }
            match proj {
                Proj::Field(field) => {
                    let (field_offset, field_ty) = self.field_offset(&ty, field, dest)?;
                    offset += field_offset;
                    // An instance erasing an argument declares a pointer
                    // field's pointee as `None`; the place records the
                    // pointee a subscript through the field strides by.
                    ty = match (field_ty.kind(), place.projection_tys.get(step)) {
                        (TyKind::Pointer { .. }, Some(recorded))
                            if matches!(recorded.kind(), TyKind::Pointer { .. }) =>
                        {
                            recorded.clone()
                        }
                        _ => field_ty,
                    };
                }
                Proj::ConstIndex(index) => {
                    if let TyKind::PopArray { size, element } = ty.kind() {
                        let element = element.clone();
                        if size.known().is_none_or(|size| {
                            u64::try_from(*index).map_or(true, |index| index >= size)
                        }) {
                            return Err(self.unsupported_reg(
                                format!("array-element projection index {index} out of range"),
                                dest,
                            ));
                        }
                        let stride = self
                            .layout
                            .layout_of(&element)
                            .map_err(|error| {
                                self.unsupported_reg(
                                    format!("array element layout ({error})"),
                                    dest,
                                )
                            })?
                            .size;
                        offset += stride * *index as u64;
                        ty = element;
                        continue;
                    }
                    let (TyKind::Tuple(elements) | TyKind::RuntimePack(elements)) = ty.kind()
                    else {
                        return Err(self
                            .unsupported_reg(format!("tuple-element projection on `{ty}`"), dest));
                    };
                    let elements = elements.clone();
                    let composed = self.struct_layout_of(&elements, dest)?;
                    let Some(element_offset) = composed.offsets.get(*index).copied() else {
                        return Err(self.unsupported_reg(
                            format!("tuple-element projection index {index} out of range"),
                            dest,
                        ));
                    };
                    offset += element_offset;
                    ty = elements[*index].clone();
                }
                Proj::Index(index) => {
                    // A literal index into pack storage projects statically,
                    // like `Proj::ConstIndex` (the Tuple accessor bodies'
                    // `self.storage[0]` shape); a runtime one traps outside
                    // `0..len` and addresses homogeneous storage by stride.
                    if let TyKind::Tuple(elements) | TyKind::RuntimePack(elements) = ty.kind() {
                        let elements = elements.clone();
                        if let Some(element) = self.constant_pack_index(*index, elements.len()) {
                            let composed = self.struct_layout_of(&elements, dest)?;
                            offset += composed.offsets[element];
                            ty = elements[element].clone();
                            continue;
                        }
                        if offset != 0 {
                            address = self.gep_byte(ctx, address, offset, dest);
                            offset = 0;
                        }
                        self.emit_index_guard(ctx, *index, elements.len(), dest)?;
                        // An empty pack's projection is never reached; it
                        // keeps the checked element type the place records.
                        let Some(first) = elements.first() else {
                            ty = place.projection_tys.get(step).cloned().ok_or_else(|| {
                                self.unsupported_reg(
                                    "untyped projection into an empty pack".into(),
                                    dest,
                                )
                            })?;
                            continue;
                        };
                        let stride = self.homogeneous_pack_stride(
                            &elements,
                            "runtime pack projection",
                            dest,
                        )?;
                        let position = self.reg_value(ctx, *index, ScalarTy::Int)?;
                        address =
                            self.strided_element_address(ctx, address, position, stride, dest);
                        ty = first.clone();
                        continue;
                    }
                    let TyKind::Pointer { element, .. } = ty.kind() else {
                        return Err(
                            self.unsupported_reg(format!("subscript projection on `{ty}`"), dest)
                        );
                    };
                    let element = element.clone();
                    // The address so far designates pointer storage; load the
                    // pointer value and address its element.
                    if offset != 0 {
                        address = self.gep_byte(ctx, address, offset, dest);
                        offset = 0;
                    }
                    let ptr_handle = ScalarTy::Ptr.handle(ctx);
                    let load = LoadOp::new(ctx, address, ptr_handle);
                    self.append(ctx, load.get_operation(), Some(dest));
                    address = self.pointer_element_address(
                        ctx,
                        load.get_result(ctx),
                        *index,
                        &element,
                        dest,
                    )?;
                    ty = element;
                }
                Proj::Variant(index) => {
                    let TyKind::Variant(alternatives) = ty.kind() else {
                        return Err(
                            self.unsupported_reg(format!("Variant projection on `{ty}`"), dest)
                        );
                    };
                    let alternatives = alternatives.clone();
                    let index = self.known_variant_index(index)?;
                    let Some(selected) = alternatives.get(index).cloned() else {
                        return Err(
                            self.unsupported_reg("Variant projection checked tag".into(), dest)
                        );
                    };
                    if offset != 0 {
                        address = self.gep_byte(ctx, address, offset, dest);
                        offset = 0;
                    }
                    self.emit_variant_tag_guard(ctx, address, index, dest)?;
                    let layout = self.layout.variant_layout(&alternatives).map_err(|error| {
                        self.unsupported_reg(format!("Variant layout ({error})"), dest)
                    })?;
                    address = self.offset_address(ctx, address, layout.payload_offset);
                    ty = selected;
                }
                Proj::UninitPayload => {
                    let Some(element) = mojito_types::types::uninit_storage_element(&ty).cloned()
                    else {
                        return Err(self.unsupported_reg(
                            format!("uninit-payload projection on `{ty}`"),
                            dest,
                        ));
                    };
                    let fields = self
                        .layout
                        .struct_layout(&[Ty::bool(), element.clone()])
                        .map_err(|error| {
                            self.unsupported_reg(format!("uninit storage ({error})"), dest)
                        })?;
                    offset += fields.offsets[1];
                    // Stores through the payload overwrite raw — the old
                    // payload leaks by design, exactly the VM's
                    // `unsafe_write`. The enclosing Store marks presence.
                    ty = element;
                }
                // The elaborator erases every rebound place's step.
                Proj::Rebind => {
                    return Err(
                        self.unsupported_reg("rebound place in elaborated MIR".into(), dest)
                    );
                }
            }
        }
        let address = if offset == 0 {
            address
        } else {
            self.gep_byte(ctx, address, offset, dest)
        };
        Ok((address, ty))
    }

    /// The address a `mut`/`ref` parameter binds for its argument's
    /// `place`: the storage the place designates, or, for a `ref` field,
    /// the referent the handle stored there addresses, as a read of the
    /// field reads through it.
    pub(super) fn reference_argument_address(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let (mut address, mut ty) = self.place_address(ctx, place, dest)?;
        while let TyKind::Ref(reference) = ty.kind() {
            let load = LoadOp::new(ctx, address, ScalarTy::Ptr.handle(ctx));
            self.append(ctx, load.get_operation(), Some(dest));
            address = load.get_result(ctx);
            ty = reference.referent.clone();
        }
        Ok(address)
    }

    /// The byte offset and checked type of `field` within struct type `ty`.
    pub(super) fn field_offset(
        &self,
        ty: &Ty,
        field: &str,
        dest: Reg,
    ) -> Result<(u64, Ty), PlironError> {
        let TyKind::Struct(name, _) = ty.kind() else {
            return Err(self.unsupported_reg(format!("field access on `{ty}`"), dest));
        };
        let Some(decl) = self.struct_decls.get(name.as_str()) else {
            return Err(
                self.unsupported_reg(format!("struct `{name}` without a declaration"), dest)
            );
        };
        let Some(position) = decl.fields.iter().position(|(n, _)| n == field) else {
            return Err(
                self.unsupported_reg(format!("struct `{name}` has no field `{field}`"), dest)
            );
        };
        let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, t)| t.clone()).collect();
        let composed = self.struct_layout_of(&field_tys, dest)?;
        Ok((composed.offsets[position], field_tys[position].clone()))
    }

    /// The composed layout of `fields`, or a contextual rejection.
    pub(super) fn struct_layout_of(
        &self,
        fields: &[Ty],
        dest: Reg,
    ) -> Result<mojito_native::native::layout::StructLayout, PlironError> {
        self.layout
            .struct_layout(fields)
            .map_err(|error| self.unsupported_reg(format!("aggregate layout ({error})"), dest))
    }

    /// `GetField` on an aggregate-valued register (field reads through places
    /// use `LoadPlace`; this covers direct register bases).
    pub(super) fn lower_get_field(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        field: &str,
    ) -> Result<(), PlironError> {
        let Some(mut base_ty) = self.func.reg_types.get(&base.0).cloned() else {
            return Err(self.unsupported_reg(format!("untyped field base %r{}", base.0), dest));
        };
        while let TyKind::Ref(reference) = base_ty.kind() {
            base_ty = reference.referent.clone();
        }
        let (offset, field_ty) = self.field_offset(&base_ty, field, dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let address = if offset == 0 {
            base_ptr
        } else {
            self.gep_byte(ctx, base_ptr, offset, dest)
        };
        self.load_from(ctx, address, &field_ty, dest)
    }

    /// Move a variable through the compiled `__moveinit__` it names — the VM's
    /// `move_value` over a `^` transfer: fresh `out self` storage, the source
    /// storage as the consumed `move` argument, and a vacated source slot.
    pub(super) fn move_via_moveinit(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        moveinit: &str,
        layout: Layout,
    ) -> Result<(), PlironError> {
        let Some(signature) = self.signatures.get(moveinit) else {
            return Err(self.unsupported_reg(format!("move via the uncompiled `{moveinit}`"), dest));
        };
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{moveinit}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        let src = self.var_slots[var as usize];
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![storage, src],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `MakeTuple`: fresh storage with each element stored at its composed
    /// offset (compiler-private pack storage, a `VariadicList`'s array of
    /// element pointers among it).
    pub(super) fn lower_make_tuple(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        elems: &[Reg],
        element_types: Option<&[Ty]>,
    ) -> Result<(), PlironError> {
        let elements: Vec<Ty> = match (
            self.func.reg_types.get(&dest.0).map(Ty::kind),
            element_types,
        ) {
            (Some(TyKind::Tuple(es) | TyKind::RuntimePack(es)), _) => es.clone(),
            (_, Some(es)) => es.to_vec(),
            _ => {
                return Err(self.unsupported_reg("untyped tuple construction".into(), dest));
            }
        };
        let storage = self.collect_pack_elements(ctx, &elements, elems, dest)?;
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `array.create`: fresh `!pop.array` storage holding `elems` in order,
    /// laid out as that many fields of the element type are.
    pub(super) fn lower_array_create(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        elems: &[Reg],
    ) -> Result<(), PlironError> {
        let Some(TyKind::PopArray { element, .. }) = self.func.reg_types.get(&dest.0).map(Ty::kind)
        else {
            return Err(self.unsupported_reg("untyped array construction".into(), dest));
        };
        let elements = vec![element.clone(); elems.len()];
        let storage = self.collect_pack_elements(ctx, &elements, elems, dest)?;
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `array.repeat`: fresh `!pop.array` storage holding `value` at every
    /// element's stride.
    pub(super) fn lower_array_repeat(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
    ) -> Result<(), PlironError> {
        let (element, size, stride) = self.array_shape(dest, dest)?;
        let storage = self.array_storage(ctx, dest)?;
        for at in 0..size {
            let address = self.offset_address(ctx, storage, stride * at);
            self.store_to(ctx, address, &element, value)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `array.get`: element `index` of `array`'s storage, read at its
    /// stride, as a constant element projection reads it.
    pub(super) fn lower_array_get(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        array: Reg,
        index: &Const,
    ) -> Result<(), PlironError> {
        let (element, _, stride) = self.array_shape(array, dest)?;
        let index = self.array_index(index, dest)?;
        let storage = self.reg_ptr(ctx, array)?;
        let address = self.offset_address(ctx, storage, stride * index);
        self.load_from(ctx, address, &element, dest)
    }

    /// `array.replace`: a fresh copy of `array`'s storage with `value`
    /// stored at element `index`.
    pub(super) fn lower_array_replace(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        array: Reg,
        value: Reg,
        index: &Const,
    ) -> Result<(), PlironError> {
        let (element, _, stride) = self.array_shape(array, dest)?;
        let index = self.array_index(index, dest)?;
        let storage = self.array_storage(ctx, dest)?;
        let ty = self.func.reg_types[&dest.0].clone();
        let layout = self
            .layout
            .layout_of(&ty)
            .map_err(|error| self.unsupported_reg(format!("array layout ({error})"), dest))?;
        let source = self.reg_ptr(ctx, array)?;
        self.copy_value(ctx, storage, source, &ty, layout, dest);
        let address = self.offset_address(ctx, storage, stride * index);
        self.store_to(ctx, address, &element, value)?;
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `array.gep`: the address of element `index` of the `!pop.array`
    /// `array` addresses, at the element's stride, as the pin's
    /// `pop.array.gep` neither reads nor bound-checks it.
    pub(super) fn lower_array_gep(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        array: Reg,
        index: Reg,
    ) -> Result<(), PlironError> {
        let Some(TyKind::KgenPointer(pointee)) = self.func.reg_types.get(&array.0).map(Ty::kind)
        else {
            return Err(self.unsupported_reg("untyped array.gep operand".into(), dest));
        };
        let TyKind::PopArray { element, .. } = pointee.kind() else {
            return Err(self.unsupported_reg("array.gep over a non-array pointee".into(), dest));
        };
        let stride = self
            .layout
            .layout_of(element)
            .map_err(|error| self.unsupported_reg(format!("array element layout ({error})"), dest))?
            .size;
        let base = self.reg_value(ctx, array, ScalarTy::Ptr)?;
        let index = self.reg_value(ctx, index, ScalarTy::Int)?;
        let size = self.uint_constant(ctx, stride);
        let bytes = MulOp::new_with_overflow_flag(ctx, index, size, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let gep = GetElementPtrOp::new(
            ctx,
            base,
            vec![GepIndex::Value(bytes.get_result(ctx))],
            i8_ty,
        );
        self.append(ctx, gep.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, gep.get_result(ctx));
        Ok(())
    }

    /// The element type, element count, and element stride of the
    /// `!pop.array` register `array`.
    fn array_shape(&self, array: Reg, dest: Reg) -> Result<(Ty, u64, u64), PlironError> {
        let Some(TyKind::PopArray { size, element }) =
            self.func.reg_types.get(&array.0).map(Ty::kind)
        else {
            return Err(self.unsupported_reg("untyped array operand".into(), dest));
        };
        let size = size
            .known()
            .ok_or_else(|| self.unsupported_reg(format!("the array size `{size}`"), dest))?;
        let stride = self
            .layout
            .layout_of(element)
            .map_err(|error| self.unsupported_reg(format!("array element layout ({error})"), dest))?
            .size;
        Ok((element.clone(), size, stride))
    }

    /// Fresh storage for the `!pop.array` register `dest`, laid out as
    /// [`Self::lower_array_create`]'s.
    fn array_storage(&mut self, ctx: &mut Context, dest: Reg) -> Result<Value, PlironError> {
        let ty = self.func.reg_types[&dest.0].clone();
        let layout = self
            .layout
            .layout_of(&ty)
            .map_err(|error| self.unsupported_reg(format!("array layout ({error})"), dest))?;
        Ok(self.entry_alloca(ctx, layout.size.max(1), layout.align.max(1)))
    }

    /// The element a closed `array.get` or `array.replace` index names.
    fn array_index(&self, index: &Const, dest: Reg) -> Result<u64, PlironError> {
        index
            .index_attribute()
            .and_then(|index| u64::try_from(index).ok())
            .ok_or_else(|| self.unsupported_reg(format!("the array index `{index:?}`"), dest))
    }

    /// Relocate the registers `elems` into fresh pack storage laid out as
    /// `elements` (the VM's `Tuple(*args)`): `store_to` transfers owned
    /// temporaries and stores the rest as the MIR left them.
    fn collect_pack_elements(
        &mut self,
        ctx: &mut Context,
        elements: &[Ty],
        elems: &[Reg],
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let composed = self.struct_layout_of(elements, dest)?;
        let storage = self.entry_alloca(
            ctx,
            composed.layout.size.max(1),
            composed.layout.align.max(1),
        );
        for ((elem, element), offset) in elems.iter().zip(elements).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, element, *elem)?;
        }
        Ok(storage)
    }
}

/// Whether a recorded loan-place projection and a later use's projection
/// select the same storage step: dynamic indices agree regardless of the
/// register that carries them (a rebound `ref` binding's index register is
/// not the use's).
fn projections_agree(recorded: &Proj, used: &Proj) -> bool {
    match (recorded, used) {
        (Proj::Field(a), Proj::Field(b)) => a == b,
        (Proj::Index(_), Proj::Index(_)) => true,
        (Proj::ConstIndex(a), Proj::ConstIndex(b)) => a == b,
        (Proj::Variant(a), Proj::Variant(b)) => a.may_equal(b),
        (Proj::UninitPayload, Proj::UninitPayload) => true,
        _ => false,
    }
}
