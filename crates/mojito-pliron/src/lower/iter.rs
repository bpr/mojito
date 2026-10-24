//! Iterator lowering: `GetIter`, pack iteration, `HasNext`/`Next` and
//! their raising/reference variants.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// `GetIter`: normalize the iterable variable through its checker-selected
    /// (and mono-retargeted) `__iter__` chain into the iterator variable.
    /// Receiver conventions mirror the VM: a borrowed (`ref`/`mut`) step
    /// aliases the current storage — for step 0 that is the source slot, the
    /// VM's reference-handle seam, so a borrowing iterator roots at the loop
    /// frame — a `read` step passes a plain byte copy (the VM's
    /// `current.clone()`, no lifecycle copy), and an owned (`var`) step
    /// consumes the current storage in place.
    pub(super) fn lower_get_iter(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        prepare: &[String],
    ) -> Result<(), PlironError> {
        // An owned `VariadicList` iterates itself, narrowing its span past
        // each element it hands on. Handled before the identity check.
        if prepare.is_empty()
            && self.owned_list_element(dest).is_some()
            && (source == dest || self.owned_list_element(source).is_some())
        {
            return self.lower_variadic_list_iter_init(ctx, source, dest);
        }
        if prepare.is_empty()
            && let Some(elements) = self.pack_iter_elements(dest)
            && (source == dest
                || matches!(
                    self.func.var_tys.get(&source).map(Ty::kind),
                    Some(TyKind::RuntimePack(_) | TyKind::Tuple(_))
                ))
        {
            return self.lower_pack_iter_init(ctx, source, dest, &elements);
        }
        if prepare.is_empty() && source == dest {
            // Identity normalization: the slot already holds the iterator.
            return Ok(());
        }
        let LowerTy::Aggregate {
            layout: dest_layout,
            ..
        } = self.var_lower_ty(dest)?
        else {
            return Err(self.unsupported("non-aggregate iterator variable".into(), None));
        };
        // A borrowed named source binds its slot to a reference handle; load
        // it to reach the iterable's storage, as the VM dereferences for
        // method resolution.
        let source_ty = self.func.var_tys.get(&source).cloned().ok_or_else(|| {
            self.unsupported(
                format!(
                    "untyped variable `{}`",
                    self.func
                        .var_names
                        .get(source as usize)
                        .map_or("?", String::as_str)
                ),
                None,
            )
        })?;
        let (mut current, mut current_ty) = if let TyKind::Ref(reference) = source_ty.kind() {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, self.var_slots[source as usize], handle);
            self.append(ctx, load.get_operation(), None);
            (load.get_result(ctx), reference.referent.clone())
        } else {
            (self.var_slots[source as usize], source_ty)
        };
        // Whether `current` is a chain temporary this instruction owns (the
        // source variable owns its own storage).
        let mut owns_current = false;
        for selected in prepare {
            let Some(signature) = self.signatures.get(selected) else {
                return Err(self.unsupported(
                    format!("iterator preparation via uncompiled `{selected}`"),
                    None,
                ));
            };
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported(format!("raising iterator preparation `{selected}`"), None)
                );
            }
            let Some(receiver_param) = signature.params.first().cloned() else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without a receiver"),
                    None,
                ));
            };
            let Some(result_layout) = signature.sret else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without an aggregate result"),
                    None,
                ));
            };
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            let borrowed = signature.ref_params.first().copied().unwrap_or(false)
                || matches!(
                    self.declarations
                        .get(selected)
                        .and_then(|decl| decl.receiver_convention.as_ref()),
                    Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
                );
            let owned = signature.owned_params.first().copied().unwrap_or(false);
            let (receiver, release_current) = if borrowed || owned {
                // Aliased or consumed in place; a consumed chain temporary
                // needs no release (the callee destroyed it).
                (current, false)
            } else {
                let LowerTy::Aggregate { layout, .. } = receiver_param else {
                    return Err(self.unsupported(
                        format!("iterator preparation `{selected}` on a scalar receiver"),
                        None,
                    ));
                };
                let copy = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, copy, current, layout.size, Reg(u32::MAX));
                (copy, owns_current)
            };
            let result = self.entry_alloca(ctx, result_layout.size, result_layout.align);
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                func_ty,
                vec![result, receiver],
            );
            self.append(ctx, call.get_operation(), None);
            if release_current {
                // The VM's superseded intermediate drops silently (no user
                // destructor); free its heap invisibly or reject.
                if self.owns_heap(&current_ty) {
                    if self.releasable(&current_ty) {
                        self.emit_release_storage(ctx, current, &current_ty)?;
                    } else {
                        return Err(self.unsupported(
                            format!(
                                "iterator preparation abandoning `{current_ty}` with destructor work"
                            ),
                            None,
                        ));
                    }
                }
            }
            current = result;
            current_ty = self
                .declarations
                .get(selected)
                .map(|decl| decl.ret_ty.clone())
                .ok_or_else(|| {
                    self.unsupported(
                        format!("iterator preparation `{selected}` without declaration facts"),
                        None,
                    )
                })?;
            owns_current = true;
        }
        if prepare.is_empty() && self.owns_heap(&current_ty) {
            // A stepless split binds a plain clone of the source; a byte copy
            // of heap-owning storage would double-release at the two drops.
            return Err(self.unsupported(
                format!("borrowed iteration of `{current_ty}` without a preparation step"),
                None,
            ));
        }
        self.mem_copy(
            ctx,
            self.var_slots[dest as usize],
            current,
            dest_layout.size,
            Reg(u32::MAX),
        );
        Ok(())
    }

    /// The pack element list of `iter`'s split slot, when the monomorphizer
    /// typed it for the compiler-private pack fallback.
    pub(super) fn pack_iter_elements(&self, iter: u32) -> Option<Vec<Ty>> {
        match self.func.var_tys.get(&iter).map(Ty::kind) {
            Some(TyKind::RuntimePack(elements) | TyKind::Tuple(elements)) => Some(elements.clone()),
            _ => None,
        }
    }

    /// The element type of `var`, when it holds the owned `VariadicList`
    /// `for var v in xs^` consumes.
    pub(super) fn owned_list_element(&self, var: u32) -> Option<Ty> {
        self.func
            .var_tys
            .get(&var)
            .and_then(|ty| mojito_types::types::variadic_list_element(ty.kind()))
            .cloned()
    }

    /// The byte stride between the elements of homogeneous pack storage
    /// `elements` (non-empty); `what` names the rejected heterogeneous use.
    pub(super) fn homogeneous_pack_stride(
        &self,
        elements: &[Ty],
        what: &str,
        dest: Reg,
    ) -> Result<u64, PlironError> {
        if elements.iter().any(|element| element != &elements[0]) {
            return Err(self.unsupported_reg(format!("heterogeneous {what}"), dest));
        }
        let composed = self.struct_layout_of(elements, dest)?;
        Ok(if elements.len() > 1 {
            composed.offsets[1] - composed.offsets[0]
        } else {
            0
        })
    }

    /// The offsets, within the `VariadicList` `var` holds, of its span's
    /// pointer to the first element pointer and of its length.
    fn variadic_list_span(&self, var: u32, dest: Reg) -> Result<(u64, u64), PlironError> {
        let list = self.func.var_tys.get(&var).cloned().ok_or_else(|| {
            self.unsupported_reg("an untyped `VariadicList` slot".to_string(), dest)
        })?;
        let (span_offset, span) = self.field_offset(&list, "_value", dest)?;
        let (data, _) = self.field_offset(&span, "_data", dest)?;
        let (size, _) = self.field_offset(&span, "_size", dest)?;
        Ok((span_offset + data, span_offset + size))
    }

    /// `base + position * stride` bytes: the address of element `position`
    /// of homogeneous pack storage.
    pub(super) fn strided_element_address(
        &mut self,
        ctx: &mut Context,
        base: Value,
        position: Value,
        stride: u64,
        dest: Reg,
    ) -> Value {
        let stride_value = self.int_constant(ctx, stride as i64);
        let scaled =
            MulOp::new_with_overflow_flag(ctx, position, stride_value, no_overflow_flags());
        self.append(ctx, scaled.get_operation(), Some(dest));
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let address = GetElementPtrOp::new(
            ctx,
            base,
            vec![GepIndex::Value(scaled.get_result(ctx))],
            i8_ty,
        );
        self.append(ctx, address.get_operation(), Some(dest));
        address.get_result(ctx)
    }

    /// Define `dest` as a zeroed value of its register type, for a read on
    /// a path that never completes (an empty pack's element).
    pub(super) fn define_zeroed(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let int = Ty::int();
        match lower_ty(
            self.name,
            self.func.reg_types.get(&dest.0).unwrap_or(&int),
            &self.layout,
            self.reg_span(dest),
        )? {
            LowerTy::Scalar(_) => {
                let zero = self.int_constant(ctx, 0);
                self.reg_values.insert(dest.0, zero);
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.reg_values.insert(dest.0, storage);
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
            }
        }
        Ok(())
    }

    /// The backend-side advance position of a pack-fallback iterator slot
    /// (the slot itself keeps the pack layout), created on first use.
    pub(super) fn pack_position_slot(&mut self, ctx: &mut Context, iter: u32) -> Value {
        if let Some(slot) = self.pack_positions.get(&iter) {
            return *slot;
        }
        // A typed slot: mem2reg promotes an alloca at its element type, and
        // a byte-array slot would promote as `i8` under the i64 loads.
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let slot = self.entry_typed_alloca(ctx, i64_handle);
        self.pack_positions.insert(iter, slot);
        slot
    }

    /// Initialize a pack-fallback iterator: position zero, and — for a
    /// distinct destination slot — the pack bytes relocated (a raw move,
    /// the VM's iterator-slot pack copy).
    pub(super) fn lower_pack_iter_init(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        elements: &[Ty],
    ) -> Result<(), PlironError> {
        let position = self.pack_position_slot(ctx, dest);
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, position);
        self.append(ctx, store.get_operation(), None);
        if source != dest {
            let composed = self.struct_layout_of(elements, Reg(u32::MAX))?;
            let from = self.var_slots[source as usize];
            let to = self.var_slots[dest as usize];
            self.mem_copy(ctx, to, from, composed.layout.size, Reg(u32::MAX));
        }
        Ok(())
    }

    /// `HasNext` over the compiler-private pack fallback: the shadow position
    /// against the static element count.
    pub(super) fn lower_has_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
    ) -> Result<(), PlironError> {
        if self.owned_list_element(iter).is_some() {
            let (_, size) = self.variadic_list_span(iter, dest)?;
            let length_slot = self.offset_address(ctx, self.var_slots[iter as usize], size);
            let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
            let length = LoadOp::new(ctx, length_slot, i64_handle);
            self.append(ctx, length.get_operation(), Some(dest));
            let length = length.get_result(ctx);
            let zero = self.int_constant(ctx, 0);
            let more = ICmpOp::new(ctx, ICmpPredicateAttr::SGT, length, zero);
            return self.define(ctx, dest, more.get_operation(), more.get_result(ctx));
        }
        let Some(elements) = self.pack_iter_elements(iter) else {
            return Err(self.unsupported_reg("method-free iterator length read".into(), dest));
        };
        let position_slot = self.pack_position_slot(ctx, iter);
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let position = LoadOp::new(ctx, position_slot, i64_handle);
        self.append(ctx, position.get_operation(), Some(dest));
        let count = self.int_constant(ctx, elements.len() as i64);
        let more = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, position.get_result(ctx), count);
        self.define(ctx, dest, more.get_operation(), more.get_result(ctx))
    }

    /// `Next` over the compiler-private pack fallback: read the element at the
    /// cursor position and advance (the VM's `remove(0)` pop, with the
    /// position standing in for the shift).
    pub(super) fn lower_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
    ) -> Result<(), PlironError> {
        if let Some(element) = self.owned_list_element(iter) {
            let source = self.advance_variadic_list(ctx, iter, dest)?;
            return self.move_pack_element_out(ctx, dest, source, &element);
        }
        {
            if let Some(elements) = self.pack_iter_elements(iter) {
                let Some(first) = elements.first() else {
                    // An empty pack's advance is dead code (`HasNext` is
                    // statically false).
                    return self.define_zeroed(ctx, dest);
                };
                // A run-time cursor would leave the visited elements moved
                // out under a teardown that drop elaboration decided for the
                // whole pack, so droppable elements refuse.
                if elements.iter().any(|element| self.needs_drop(element)) {
                    return Err(self.unsupported_reg(
                        "pack-cursor advance over droppable elements".into(),
                        dest,
                    ));
                }
                let stride = self.homogeneous_pack_stride(&elements, "pack advance", dest)?;
                let slot = self.var_slots[iter as usize];
                let position_slot = self.pack_position_slot(ctx, iter);
                let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let position = LoadOp::new(ctx, position_slot, i64_handle);
                self.append(ctx, position.get_operation(), Some(dest));
                let source =
                    self.strided_element_address(ctx, slot, position.get_result(ctx), stride, dest);
                let one = self.int_constant(ctx, 1);
                let next = AddOp::new_with_overflow_flag(
                    ctx,
                    position.get_result(ctx),
                    one,
                    no_overflow_flags(),
                );
                self.append(ctx, next.get_operation(), Some(dest));
                let store = StoreOp::new(ctx, next.get_result(ctx), position_slot);
                self.append(ctx, store.get_operation(), Some(dest));
                let first = first.clone();
                return self.move_pack_element_out(ctx, dest, source, &first);
            }
        }
        Err(self.unsupported_reg("method-free iterator advance".into(), dest))
    }

    /// Initialize an owned `VariadicList`'s loop: for a distinct
    /// destination slot, relocate the list (its elements stay in the
    /// caller's storage, now owned through the iterator).
    fn lower_variadic_list_iter_init(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
    ) -> Result<(), PlironError> {
        if source != dest {
            let LowerTy::Aggregate { layout, .. } = self.var_lower_ty(dest)? else {
                return Err(self.unsupported("a non-aggregate `VariadicList`".into(), None));
            };
            let from = self.var_slots[source as usize];
            let to = self.var_slots[dest as usize];
            self.mem_copy(ctx, to, from, layout.size, Reg(u32::MAX));
        }
        Ok(())
    }

    /// Pop the first element of the owned `VariadicList` `iter`: return its
    /// address and narrow the list's span past it, so the span always
    /// covers exactly the elements not yet consumed, which its destructor
    /// destroys.
    fn advance_variadic_list(
        &mut self,
        ctx: &mut Context,
        iter: u32,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let (data, size) = self.variadic_list_span(iter, dest)?;
        let list = self.var_slots[iter as usize];
        let data_slot = self.offset_address(ctx, list, data);
        let ptr_handle = ScalarTy::Ptr.handle(ctx);
        let elements = LoadOp::new(ctx, data_slot, ptr_handle);
        self.append(ctx, elements.get_operation(), Some(dest));
        let elements = elements.get_result(ctx);
        let source = LoadOp::new(ctx, elements, ptr_handle);
        self.append(ctx, source.get_operation(), Some(dest));
        let pointer_size = self.layout.target.triple.pointer_size();
        let rest = self.gep_byte(ctx, elements, pointer_size, dest);
        let store = StoreOp::new(ctx, rest, data_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let length_slot = self.offset_address(ctx, list, size);
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let length = LoadOp::new(ctx, length_slot, i64_handle);
        self.append(ctx, length.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let remaining =
            SubOp::new_with_overflow_flag(ctx, length.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, remaining.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, remaining.get_result(ctx), length_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        Ok(source.get_result(ctx))
    }

    /// Move the pack element at `source` out into `dest`, leaving the
    /// source bytes zeroed (the VM's popped element).
    fn move_pack_element_out(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        source: Value,
        element: &Ty,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, element, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, source, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, storage, source, layout.size, dest);
                self.mem_zero(ctx, source, layout.size);
                self.reg_values.insert(dest.0, storage);
                if self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty) || self.needs_drop(&ty) {
                    self.mark_owned_temp(dest, ty)?;
                }
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }
}
