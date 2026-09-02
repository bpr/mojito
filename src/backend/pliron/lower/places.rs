//! Place addressing: `place_address`, field offsets/layout, `GetField`,
//! slice descriptors, and tuple construction.

use super::*;

impl<'a> FnLowering<'a> {
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
        // the handle stored in the root's slot: load the pointer, then
        // project relative to the referent type.
        let through_var = place.through.unwrap_or(place.root);
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
        let designated_ty = match &root_ty {
            Ty::Ref(reference) => (*reference.referent).clone(),
            _ => root_ty.clone(),
        };
        let ref_param_root = (through_var as usize) < self.func.n_params
            && self
                .func
                .ref_params
                .get(through_var as usize)
                .copied()
                .unwrap_or(false);
        let (mut ty, mut address) = if place.through.is_some() || matches!(root_ty, Ty::Ref(_)) {
            match &through_ty {
                Ty::Ref(_) | Ty::Pointer { .. } => {
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
            }
        } else {
            (root_ty, root_slot)
        };
        let mut offset: u64 = 0;
        for proj in &place.proj {
            while let Ty::Ref(reference) = ty {
                if offset != 0 {
                    address = self.gep_byte(ctx, address, offset, dest);
                    offset = 0;
                }
                let ptr_ty = ScalarTy::Ptr.handle(ctx);
                let load = LoadOp::new(ctx, address, ptr_ty);
                self.append(ctx, load.get_operation(), Some(dest));
                address = load.get_result(ctx);
                ty = *reference.referent;
            }
            match proj {
                Proj::Field(field) => {
                    let (field_offset, field_ty) = self.field_offset(&ty, field, dest)?;
                    offset += field_offset;
                    ty = field_ty;
                }
                Proj::ConstIndex(index) => {
                    let (Ty::Tuple(elements) | Ty::RuntimePack(elements)) = &ty else {
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
                    // `self.storage[0]` shape).
                    if let Ty::Tuple(elements) | Ty::RuntimePack(elements) = &ty {
                        let elements = elements.clone();
                        let Some(PendingLiteral::Int(literal)) =
                            self.pending_literals.get(&index.0).cloned()
                        else {
                            return Err(self.unsupported_reg(
                                "runtime subscript projection into pack storage".into(),
                                dest,
                            ));
                        };
                        let element = literal
                            .to_i64()
                            .and_then(|value| usize::try_from(value).ok())
                            .filter(|value| *value < elements.len())
                            .ok_or_else(|| {
                                self.unsupported_reg(
                                    "pack subscript projection index out of range".into(),
                                    dest,
                                )
                            })?;
                        let composed = self.struct_layout_of(&elements, dest)?;
                        offset += composed.offsets[element];
                        ty = elements[element].clone();
                        continue;
                    }
                    if let Ty::Simd { dtype, width } = ty {
                        if offset != 0 {
                            address = self.gep_byte(ctx, address, offset, dest);
                            offset = 0;
                        }
                        self.emit_simd_index_guard(ctx, *index, width as usize, dest)?;
                        let element = Ty::Simd { dtype, width: 1 };
                        address =
                            self.pointer_element_address(ctx, address, *index, &element, dest)?;
                        ty = element;
                        continue;
                    }
                    let Ty::Pointer { element, .. } = &ty else {
                        return Err(
                            self.unsupported_reg(format!("subscript projection on `{ty}`"), dest)
                        );
                    };
                    let element = (**element).clone();
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
                    let Ty::Variant(alternatives) = &ty else {
                        return Err(
                            self.unsupported_reg(format!("Variant projection on `{ty}`"), dest)
                        );
                    };
                    let alternatives = alternatives.clone();
                    let Some(selected) = alternatives.get(*index).cloned() else {
                        return Err(
                            self.unsupported_reg("Variant projection checked tag".into(), dest)
                        );
                    };
                    if offset != 0 {
                        address = self.gep_byte(ctx, address, offset, dest);
                        offset = 0;
                    }
                    self.emit_variant_tag_guard(ctx, address, *index, dest)?;
                    let layout = self.layout.variant_layout(&alternatives).map_err(|error| {
                        self.unsupported_reg(format!("Variant layout ({error})"), dest)
                    })?;
                    address = self.offset_address(ctx, address, layout.payload_offset);
                    ty = selected;
                }
                Proj::UninitPayload => {
                    let Some(element) = crate::types::uninit_storage_element(&ty).cloned() else {
                        return Err(self.unsupported_reg(
                            format!("uninit-payload projection on `{ty}`"),
                            dest,
                        ));
                    };
                    let fields = self
                        .layout
                        .struct_layout(&[Ty::Bool, element.clone()])
                        .map_err(|error| {
                            self.unsupported_reg(format!("uninit storage ({error})"), dest)
                        })?;
                    offset += fields.offsets[1];
                    // Stores through the payload overwrite raw — the old
                    // payload leaks by design, exactly the VM's
                    // `unsafe_write`. The enclosing Store marks presence.
                    ty = element;
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

    /// The byte offset and checked type of `field` within struct type `ty`.
    pub(super) fn field_offset(
        &self,
        ty: &Ty,
        field: &str,
        dest: Reg,
    ) -> Result<(u64, Ty), PlironError> {
        let Ty::Struct(name, _) = ty else {
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
    ) -> Result<crate::native::layout::StructLayout, PlironError> {
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
        while let Ty::Ref(reference) = base_ty {
            base_ty = *reference.referent;
        }
        // A slice-descriptor bound access materializes a fresh `Optional`
        // through its compiled constructor — the VM's `slice_bound_optional`.
        if slice_struct_name(&base_ty).is_some() && matches!(field, "start" | "end" | "step") {
            return self.lower_slice_bound_field(ctx, dest, base, field);
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

    /// Move a variable through its compiled `__moveinit__` — the VM's
    /// `move_value` over a `^` transfer: fresh `out self` storage, the source
    /// storage as the consumed `move` argument, and a vacated source slot.
    pub(super) fn move_via_moveinit(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        name: &str,
        layout: Layout,
    ) -> Result<(), PlironError> {
        let moveinit = format!("{name}.__moveinit__");
        let signature = &self.signatures[&moveinit];
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
        // The move vacates the slot (the VM tombstones it); the moved value
        // is an owned temporary until consumed.
        self.set_drop_flag(ctx, var, false);
        if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
            && (self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty) || self.needs_drop(&ty))
        {
            self.mark_owned_temp(dest, ty)?;
        }
        Ok(())
    }

    /// One bound of the raw 32-byte slice descriptor (`{start, end, step,
    /// flags}` i64 fields — the layout `discover_structs` synthesizes),
    /// materialized as a real `Optional` by calling the destination type's
    /// compiled positional constructor: 1-argument when the bound's flag bit
    /// is set, 0-argument otherwise — the VM's `slice_bound_optional`.
    /// One bound of the raw 32-byte slice descriptor (`{start, end, step,
    /// flags}` i64 fields — the layout `discover_structs` synthesizes),
    /// materialized as an `Optional` value over a frame-backed payload slot:
    /// `{data → payload, _size ∈ {0, 1}}` — the observable state the VM's
    /// `slice_bound_optional` constructor calls produce, with no heap
    /// allocation to own.
    pub(super) fn lower_slice_bound_field(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        field: &str,
    ) -> Result<(), PlironError> {
        let Some(optional_ty @ Ty::Struct(..)) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped slice bound access".into(), dest));
        };
        let lowered = lower_ty(self.name, &optional_ty, &self.layout, self.reg_span(dest))?;
        let LowerTy::Aggregate { layout, .. } = lowered else {
            return Err(self.unsupported_reg("slice bound Optional layout".into(), dest));
        };
        let (offset, bit) = match field {
            "start" => (0u64, 1i64),
            "end" => (8, 2),
            _ => (16, 4),
        };
        let descriptor = self.reg_ptr(ctx, base)?;
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let value_address = self.offset_address(ctx, descriptor, offset);
        let value = LoadOp::new(ctx, value_address, i64_handle);
        self.append(ctx, value.get_operation(), Some(dest));
        let flags_address = self.offset_address(ctx, descriptor, 24);
        let flags = LoadOp::new(ctx, flags_address, i64_handle);
        self.append(ctx, flags.get_operation(), Some(dest));
        let mask = self.int_constant(ctx, bit);
        let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
        self.append(ctx, masked.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let is_set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
        self.append(ctx, is_set.get_operation(), Some(dest));
        let payload = self.entry_alloca(ctx, 8, 8);
        let store = StoreOp::new(ctx, value.get_result(ctx), payload);
        self.append(ctx, store.get_operation(), Some(dest));
        let temp = self.entry_alloca(ctx, layout.size, layout.align);
        let store = StoreOp::new(ctx, payload, temp);
        self.append(ctx, store.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let size = SelectOp::new(ctx, is_set.get_result(ctx), one, zero);
        self.append(ctx, size.get_operation(), Some(dest));
        let size_address = self.offset_address(ctx, temp, 8);
        let store = StoreOp::new(ctx, size.get_result(ctx), size_address);
        self.append(ctx, store.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, temp);
        Ok(())
    }

    /// Materialize one slice descriptor in the backend's raw layout: three
    /// i64 bounds at offsets 0/8/16 (absent bounds store 0) and the presence
    /// bitmask at offset 24 (start=1, end=2, step=4) — `Value::Slice`'s
    /// `Option<i64>` fields.
    pub(super) fn build_slice_descriptor(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
    ) -> Result<Value, PlironError> {
        let storage = self.entry_alloca(ctx, 32, 8);
        let mut flags = 0i64;
        for (index, (bound, bit)) in [(lower, 1i64), (upper, 2), (step, 4)].iter().enumerate() {
            let value = match bound {
                Some(reg) => self.reg_value(ctx, *reg, ScalarTy::Int)?,
                None => self.int_constant(ctx, 0),
            };
            let address = self.offset_address(ctx, storage, index as u64 * 8);
            let store = StoreOp::new(ctx, value, address);
            self.append(ctx, store.get_operation(), Some(anchor));
            if bound.is_some() {
                flags |= bit;
            }
        }
        let flags = self.int_constant(ctx, flags);
        let address = self.offset_address(ctx, storage, 24);
        let store = StoreOp::new(ctx, flags, address);
        self.append(ctx, store.get_operation(), Some(anchor));
        Ok(storage)
    }

    /// `MakeTuple`: fresh storage with each element stored at its composed
    /// offset (compiler-private heterogeneous pack storage).
    pub(super) fn lower_make_tuple(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        elems: &[Reg],
        element_types: Option<&[Ty]>,
    ) -> Result<(), PlironError> {
        let elements: Vec<Ty> = match (self.func.reg_types.get(&dest.0), element_types) {
            (Some(Ty::Tuple(es) | Ty::RuntimePack(es)), _) => es.clone(),
            (_, Some(es)) => es.to_vec(),
            _ => {
                return Err(self.unsupported_reg("untyped tuple construction".into(), dest));
            }
        };
        let composed = self.struct_layout_of(&elements, dest)?;
        let storage = self.entry_alloca(ctx, composed.layout.size, composed.layout.align);
        for ((elem, elem_ty), offset) in elems.iter().zip(&elements).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, elem_ty, *elem)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }
}
