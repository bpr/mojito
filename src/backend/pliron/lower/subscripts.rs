//! Subscript-call, index-intrinsic, and slice-index lowering.

use super::*;

impl<'a> FnLowering<'a> {
    /// One checker-selected subscript invocation (`Index`/`Slice`/
    /// `MultiIndex`/`MultiSet` nominal dispatch): bind the receiver by its
    /// compiled convention, match the actuals (index registers and
    /// inline-built slice descriptors) against the callee's slots, call, and
    /// write a `mut self` receiver back — the VM's `method_call` over the
    /// subscript contract. `anchor` is the result register for the get forms
    /// and a scratch register for `MultiSet` (whose result is discarded).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_subscript_call(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        method: &str,
        call: &crate::mir::MirSubscriptCall,
        recv: Reg,
        recv_place: Option<&MirPlace>,
        positional: &[SubscriptActual],
        keywords: &[(&str, SubscriptActual)],
    ) -> Result<(), PlironError> {
        let resolved = call.target.clone();
        let Some(signature) = self.signatures.get(&resolved) else {
            return Err(
                self.unsupported_reg(format!("subscript call to uncompiled `{resolved}`"), anchor)
            );
        };
        let params = signature.params.clone();
        let owned = signature.owned_params.clone();
        let by_reference = signature.ref_params.clone();
        if params.is_empty() {
            return Err(self.unsupported_reg(
                format!("subscript target `{resolved}` without a receiver"),
                anchor,
            ));
        }
        let Some(decl) = self.declarations.get(&resolved) else {
            return Err(self.unsupported_reg(
                format!("subscript call to `{resolved}` without a recorded declaration"),
                anchor,
            ));
        };
        if decl.variadic.is_some() || decl.kw_variadic.is_some() {
            return Err(
                self.unsupported_reg(format!("variadic subscript call to `{resolved}`"), anchor)
            );
        }
        let kw_names: Vec<&str> = keywords.iter().map(|(name, _)| *name).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            positional.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .map_err(|error| {
            self.unsupported_reg(
                format!("subscript binding for `{resolved}` failed: {error:?}"),
                anchor,
            )
        })?;
        let defaults = decl.defaults.clone();
        let receiver_convention = decl.receiver_convention;
        let receiver_alias = recv_place.is_some()
            && matches!(
                receiver_convention,
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
        let recv_owned = owned.first().copied().unwrap_or(false);
        let recv_value = if receiver_alias {
            let place = recv_place.expect("aliased receivers have a place").clone();
            self.aliased_receiver_address(ctx, &place, anchor)?
        } else {
            self.arg_value(ctx, recv, &params[0], recv_owned, anchor)?
        };
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let rest_by_reference = if by_reference.len() > 1 {
            &by_reference[1..]
        } else {
            &[]
        };
        if matched.slots.len() != rest.len() {
            return Err(self.unsupported_reg(
                format!("subscript binding for `{resolved}` disagrees with its compiled arity"),
                anchor,
            ));
        }
        let mut operands = vec![recv_value];
        for (i, slot) in matched.slots.iter().enumerate() {
            let expected = rest[i].clone();
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let owned = rest_owned.get(i).copied().unwrap_or(false);
            let by_ref = rest_by_reference.get(i).copied().unwrap_or(false);
            let actual = match slot {
                ArgSlot::Positional(p) => Some(&positional[*p]),
                ArgSlot::Keyword(k) => Some(&keywords[*k].1),
                ArgSlot::Default => None,
            };
            let value = match actual {
                Some(SubscriptActual::Reg(reg, place)) => {
                    if by_ref {
                        let Some(place) = place else {
                            return Err(self.unsupported_reg(
                                format!(
                                    "`mut`/`ref` subscript argument of `{resolved}` without a place"
                                ),
                                anchor,
                            ));
                        };
                        let place = (*place).clone();
                        self.place_address(ctx, &place, anchor)?.0
                    } else {
                        self.arg_value(ctx, *reg, &expected, owned, anchor)?
                    }
                }
                Some(SubscriptActual::Descriptor(value)) => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("`mut`/`ref` slice-descriptor argument of `{resolved}`"),
                            anchor,
                        ));
                    }
                    *value
                }
                None => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("defaulted `mut`/`ref` parameter of `{resolved}`"),
                            anchor,
                        ));
                    }
                    let Some(default) = defaults.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("non-constant default argument in call to `{resolved}`"),
                            anchor,
                        ));
                    };
                    let LowerTy::Scalar(scalar) = expected else {
                        return Err(self.unsupported_reg(
                            format!("non-scalar default argument in call to `{resolved}`"),
                            anchor,
                        ));
                    };
                    let default = default.clone();
                    self.checked_const_value(ctx, &default, scalar, anchor)?
                }
            };
            operands.push(value);
        }
        self.emit_bound_call(ctx, anchor, &resolved, operands)?;
        // A reference result is the callee's returned place pointer — the
        // caller-side handle convention; a handle to pointer-typed storage
        // joins `pointer_slot_refs` like `MakeRef`.
        if let Some(reference) = &call.reference_result
            && matches!(*reference.referent, Ty::Pointer { .. })
        {
            self.pointer_slot_refs.insert(anchor.0);
        }
        // `mut self` receivers without an aliased place write the modified
        // receiver back — the `lower_method_call` contract.
        let write_back = !receiver_alias
            && match self.func.reg_types.get(&recv.0) {
                Some(Ty::Struct(struct_name, _)) => self
                    .struct_decls
                    .get(struct_name.as_str())
                    .is_some_and(|d| {
                        d.mut_self_methods.contains(resolved.as_str())
                            || d.mut_self_methods.contains(method)
                    }),
                _ => false,
            };
        if write_back && let Some(place) = recv_place {
            let LowerTy::Aggregate { layout, .. } = &params[0] else {
                return Err(
                    self.unsupported_reg("mutating subscript on a scalar receiver".into(), anchor)
                );
            };
            let size = layout.size;
            let recv_ptr = self.reg_ptr(ctx, recv)?;
            let place = place.clone();
            let (address, _) = self.place_address(ctx, &place, anchor)?;
            self.mem_copy(ctx, address, recv_ptr, size, anchor);
        }
        Ok(())
    }

    /// One `MirSubscriptArg` as a lowered actual: an index register or an
    /// inline-built slice descriptor.
    pub(super) fn subscript_actual<'p>(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        arg: &crate::mir::MirSubscriptArg,
        place: Option<&'p MirPlace>,
    ) -> Result<SubscriptActual<'p>, PlironError> {
        Ok(match arg {
            crate::mir::MirSubscriptArg::Index(reg) => SubscriptActual::Reg(*reg, place),
            crate::mir::MirSubscriptArg::Slice {
                lower, upper, step, ..
            } => SubscriptActual::Descriptor(
                self.build_slice_descriptor(ctx, anchor, *lower, *upper, *step)?,
            ),
        })
    }

    pub(super) fn subscript_actuals<'p>(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        args: &[crate::mir::MirSubscriptArg],
        places: &'p [Option<MirPlace>],
    ) -> Result<Vec<SubscriptActual<'p>>, PlironError> {
        args.iter()
            .enumerate()
            .map(|(i, arg)| {
                self.subscript_actual(ctx, anchor, arg, places.get(i).and_then(Option::as_ref))
            })
            .collect()
    }

    /// An intrinsic storage subscript: a constant index into heterogeneous
    /// (`TupleStorage`) or homogeneous (`VariadicStorage`) pack storage — the
    /// VM's `index_value` clone at a statically composed offset. Runtime
    /// indexes stay rejected until the packs slice.
    pub(super) fn lower_index_intrinsic(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
        intrinsic: &crate::mir::MirIntrinsicSubscript,
    ) -> Result<(), PlironError> {
        use crate::mir::MirIntrinsicSubscript as Sub;
        if matches!(intrinsic, Sub::Simd) {
            let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&base.0).cloned() else {
                return Err(self.unsupported_reg("SIMD subscript base type".into(), dest));
            };
            let base_ptr = self.reg_ptr(ctx, base)?;
            let element = Ty::Simd { dtype, width: 1 };
            self.emit_simd_index_guard(ctx, index, width as usize, dest)?;
            let address = self.pointer_element_address(ctx, base_ptr, index, &element, dest)?;
            return self.load_from(ctx, address, &element, dest);
        }
        if !matches!(intrinsic, Sub::TupleStorage | Sub::VariadicStorage) {
            return Err(self.unsupported_reg("intrinsic subscript".into(), dest));
        }
        let elements = match self.func.reg_types.get(&base.0) {
            Some(Ty::Tuple(elements) | Ty::RuntimePack(elements)) => elements.clone(),
            other => {
                return Err(self.unsupported_reg(
                    format!(
                        "intrinsic subscript on `{}`",
                        other.map(|ty| ty.to_string()).unwrap_or_default()
                    ),
                    dest,
                ));
            }
        };
        let Some(PendingLiteral::Int(literal)) = self.pending_literals.get(&index.0).cloned()
        else {
            return Err(self.unsupported_reg("runtime index into pack storage".into(), dest));
        };
        let element = literal
            .to_i64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value < elements.len())
            .ok_or_else(|| {
                self.unsupported_reg("pack subscript index out of range".into(), dest)
            })?;
        let composed = self.struct_layout_of(&elements, dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let offset = composed.offsets[element];
        let address = if offset == 0 {
            base_ptr
        } else {
            self.gep_byte(ctx, base_ptr, offset, dest)
        };
        self.load_from(ctx, address, &elements[element], dest)
    }

    pub(super) fn emit_simd_index_guard(
        &mut self,
        ctx: &mut Context,
        index: Reg,
        width: usize,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let value = self.reg_value(ctx, index, ScalarTy::Int)?;
        let limit = self.int_constant(ctx, width as i64);
        let invalid = ICmpOp::new(ctx, ICmpPredicateAttr::UGE, value, limit);
        self.append(ctx, invalid.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            invalid.get_result(ctx),
            TrapCategory::UnhandledError,
            dest,
        )
    }

    /// The slice-descriptor `indices(length)` normalization — the VM's
    /// `normalize_slice_bounds` — as branch-free selects over the raw
    /// descriptor, producing the three-element bounds tuple. A zero step
    /// traps (the VM's runtime error; the negative-length check is
    /// unreachable over container sizes and is not replicated).
    pub(super) fn lower_slice_indices(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        if args.len() != 1 {
            return Err(self.unsupported_reg("slice `indices` call contract".into(), dest));
        }
        let length = self.reg_value(ctx, args[0], ScalarTy::Int)?;
        let descriptor = self.reg_ptr(ctx, recv)?;
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let bound = |lowering: &mut Self, ctx: &mut Context, offset: u64, bit: i64| {
            let address = lowering.offset_address(ctx, descriptor, offset);
            let value = LoadOp::new(ctx, address, i64_handle);
            lowering.append(ctx, value.get_operation(), Some(dest));
            let flags_address = lowering.offset_address(ctx, descriptor, 24);
            let flags = LoadOp::new(ctx, flags_address, i64_handle);
            lowering.append(ctx, flags.get_operation(), Some(dest));
            let mask = lowering.int_constant(ctx, bit);
            let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
            lowering.append(ctx, masked.get_operation(), Some(dest));
            let zero = lowering.int_constant(ctx, 0);
            let is_set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
            lowering.append(ctx, is_set.get_operation(), Some(dest));
            (value.get_result(ctx), is_set.get_result(ctx))
        };
        let (raw_lower, has_lower) = bound(self, ctx, 0, 1);
        let (raw_upper, has_upper) = bound(self, ctx, 8, 2);
        let (raw_step, has_step) = bound(self, ctx, 16, 4);
        let one = self.int_constant(ctx, 1);
        let step = SelectOp::new(ctx, has_step, raw_step, one);
        self.append(ctx, step.get_operation(), Some(dest));
        let step = step.get_result(ctx);
        let zero = self.int_constant(ctx, 0);
        let step_is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, step, zero);
        self.append(ctx, step_is_zero.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            step_is_zero.get_result(ctx),
            TrapCategory::UnhandledError,
            dest,
        )?;
        let step_positive = ICmpOp::new(ctx, ICmpPredicateAttr::SGT, step, zero);
        self.append(ctx, step_positive.get_operation(), Some(dest));
        let step_positive = step_positive.get_result(ctx);
        let minus_one = self.int_constant(ctx, -1);
        let len_minus_one = SubOp::new_with_overflow_flag(ctx, length, one, no_overflow_flags());
        self.append(ctx, len_minus_one.get_operation(), Some(dest));
        let len_minus_one = len_minus_one.get_result(ctx);
        // Clamp an explicit bound to a valid range, wrapping a negative
        // index once (`runtime::normalize_slice_bounds`'s `adjust`).
        let adjust = |lowering: &mut Self, ctx: &mut Context, raw: Value| {
            let wrapped = AddOp::new_with_overflow_flag(ctx, raw, length, no_overflow_flags());
            lowering.append(ctx, wrapped.get_operation(), Some(dest));
            let negative = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, raw, zero);
            lowering.append(ctx, negative.get_operation(), Some(dest));
            let adjusted =
                SelectOp::new(ctx, negative.get_result(ctx), wrapped.get_result(ctx), raw);
            lowering.append(ctx, adjusted.get_operation(), Some(dest));
            let adjusted = adjusted.get_result(ctx);
            let clamp =
                |lowering: &mut Self, ctx: &mut Context, value: Value, low: Value, high: Value| {
                    let below = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, value, low);
                    lowering.append(ctx, below.get_operation(), Some(dest));
                    let floored = SelectOp::new(ctx, below.get_result(ctx), low, value);
                    lowering.append(ctx, floored.get_operation(), Some(dest));
                    let above =
                        ICmpOp::new(ctx, ICmpPredicateAttr::SGT, floored.get_result(ctx), high);
                    lowering.append(ctx, above.get_operation(), Some(dest));
                    let clamped =
                        SelectOp::new(ctx, above.get_result(ctx), high, floored.get_result(ctx));
                    lowering.append(ctx, clamped.get_operation(), Some(dest));
                    clamped.get_result(ctx)
                };
            let positive = clamp(lowering, ctx, adjusted, zero, length);
            let negative = clamp(lowering, ctx, adjusted, minus_one, len_minus_one);
            let result = SelectOp::new(ctx, step_positive, positive, negative);
            lowering.append(ctx, result.get_operation(), Some(dest));
            result.get_result(ctx)
        };
        let adjusted_lower = adjust(self, ctx, raw_lower);
        let adjusted_upper = adjust(self, ctx, raw_upper);
        let default_start = SelectOp::new(ctx, step_positive, zero, len_minus_one);
        self.append(ctx, default_start.get_operation(), Some(dest));
        let start = SelectOp::new(
            ctx,
            has_lower,
            adjusted_lower,
            default_start.get_result(ctx),
        );
        self.append(ctx, start.get_operation(), Some(dest));
        let default_stop = SelectOp::new(ctx, step_positive, length, minus_one);
        self.append(ctx, default_stop.get_operation(), Some(dest));
        let stop = SelectOp::new(ctx, has_upper, adjusted_upper, default_stop.get_result(ctx));
        self.append(ctx, stop.get_operation(), Some(dest));
        let storage = self.entry_alloca(ctx, 24, 8);
        for (index, value) in [start.get_result(ctx), stop.get_result(ctx), step]
            .into_iter()
            .enumerate()
        {
            let address = self.offset_address(ctx, storage, index as u64 * 8);
            let store = StoreOp::new(ctx, value, address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }
}
