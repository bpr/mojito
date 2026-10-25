//! Subscript-call, index-intrinsic, and slice-index lowering.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;

impl FnLowering<'_> {
    /// One checker-selected subscript invocation (`Index`/`MultiIndex`/
    /// `MultiSet` nominal dispatch): bind the receiver by its compiled
    /// convention, match the actual index registers against the callee's
    /// slots, call, and
    /// write a `mut self` receiver back — the VM's `method_call` over the
    /// subscript contract. `anchor` is the result register for the get forms
    /// and a scratch register for `MultiSet` (whose result is discarded).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_subscript_call(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        method: &str,
        call: &mojito_mir::mir::MirSubscriptCall,
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
        if decl.kw_variadic.is_some() {
            return Err(self.unsupported_reg(
                format!("keyword-variadic subscript call to `{resolved}`"),
                anchor,
            ));
        }
        // A positional collector's slot takes the pack the subscript builds
        // (`tuple.make`), its one overflow operand.
        let collector = decl.variadic.as_ref().and(decl.variadic_index);
        let kw_names: Vec<&str> = keywords.iter().map(|(name, _)| *name).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            positional.len(),
            &kw_names,
            CallVariadics {
                positional: collector.is_some(),
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
                Some(mojito_ast::ast::ArgConvention::Mut | mojito_ast::ast::ArgConvention::Ref)
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
        if matched.slots.len() + usize::from(collector.is_some()) != rest.len() {
            return Err(self.unsupported_reg(
                format!("subscript binding for `{resolved}` disagrees with its compiled arity"),
                anchor,
            ));
        }
        let collector_value = match (collector, matched.positional_overflow.as_slice()) {
            (None, _) => None,
            (Some(_), [built]) => match &positional[*built] {
                SubscriptActual::Reg(reg, _) => Some(self.reg_value(ctx, *reg, ScalarTy::Ptr)?),
            },
            (Some(_), _) => None,
        };
        if collector.is_some() && collector_value.is_none() {
            return Err(self.unsupported_reg(
                format!("a subscript call of `{resolved}` passes no positional collector"),
                anchor,
            ));
        }
        let mut operands = vec![recv_value];
        let mut slots = matched.slots.iter().enumerate();
        for (param, expected) in rest.iter().enumerate() {
            if Some(param) == collector
                && let Some(value) = collector_value
            {
                operands.push(value);
                continue;
            }
            let Some((i, slot)) = slots.next() else {
                return Err(self.unsupported_reg(
                    format!("subscript binding for `{resolved}` disagrees with its compiled arity"),
                    anchor,
                ));
            };
            let expected = expected.clone();
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let owned = rest_owned.get(param).copied().unwrap_or(false);
            let by_ref = rest_by_reference.get(param).copied().unwrap_or(false);
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
                    let default = default.clone();
                    self.default_argument_value(ctx, &default, &expected, owned, &resolved, anchor)?
                }
            };
            operands.push(value);
        }
        self.emit_bound_call(ctx, anchor, &resolved, operands)?;
        // `mut self` receivers without an aliased place write the modified
        // receiver back — the `lower_method_call` contract.
        let write_back = !receiver_alias
            && !self.aliased_load_regs.contains(&recv.0)
            && match self.func.reg_types.get(&recv.0).map(Ty::kind) {
                Some(TyKind::Struct(struct_name, _)) => self
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

    /// One `MirSubscriptArg` as a lowered actual: its index register.
    pub(super) const fn subscript_actual<'p>(
        arg: &mojito_mir::mir::MirSubscriptArg,
        place: Option<&'p MirPlace>,
    ) -> SubscriptActual<'p> {
        match arg {
            mojito_mir::mir::MirSubscriptArg::Index(reg) => SubscriptActual::Reg(*reg, place),
        }
    }

    pub(super) fn subscript_actuals<'p>(
        args: &[mojito_mir::mir::MirSubscriptArg],
        places: &'p [Option<MirPlace>],
    ) -> Vec<SubscriptActual<'p>> {
        args.iter()
            .enumerate()
            .map(|(i, arg)| Self::subscript_actual(arg, places.get(i).and_then(Option::as_ref)))
            .collect()
    }

    /// An intrinsic storage subscript: a constant index into heterogeneous
    /// (`TupleStorage`) or homogeneous (`VariadicStorage`) pack storage — the
    /// VM's `index_value` clone at a statically composed offset; a runtime
    /// index reads homogeneous storage at a strided offset.
    pub(super) fn lower_index_intrinsic(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
        intrinsic: mojito_mir::mir::MirIntrinsicSubscript,
    ) -> Result<(), PlironError> {
        use mojito_mir::mir::MirIntrinsicSubscript as Sub;
        if matches!(intrinsic, Sub::Simd) {
            let Some((dtype, width)) = self.func.reg_types.get(&base.0).and_then(simd_dims) else {
                return Err(self.unsupported_reg("SIMD subscript base type".into(), dest));
            };
            self.emit_index_guard(ctx, index, width as usize, dest)?;
            // A width-1 vector is a scalar register (`LowerTy::Scalar`): its
            // only lane is the value itself.
            if width == 1 {
                let value = self.reg_value(ctx, base, ScalarTy::of_dtype(dtype))?;
                self.reg_values.insert(dest.0, value);
                return Ok(());
            }
            // The guard above makes the dynamic `extractelement` index
            // in range (an out-of-range LLVM index is poison).
            let vector = self.simd_load_vector(ctx, base, dtype, width as usize, dest)?;
            let position = self.reg_value(ctx, index, ScalarTy::Int)?;
            let extract = ExtractElementOp::new(ctx, vector, position);
            return self.define(ctx, dest, extract.get_operation(), extract.get_result(ctx));
        }
        if !matches!(intrinsic, Sub::TupleStorage | Sub::VariadicStorage) {
            return Err(self.unsupported_reg("intrinsic subscript".into(), dest));
        }
        let elements = match self.func.reg_types.get(&base.0).map(Ty::kind) {
            Some(TyKind::Tuple(elements) | TyKind::RuntimePack(elements)) => elements.clone(),
            other => {
                return Err(self.unsupported_reg(
                    format!(
                        "intrinsic subscript on `{}`",
                        other
                            .map(std::string::ToString::to_string)
                            .unwrap_or_default()
                    ),
                    dest,
                ));
            }
        };
        let Some(element) = self.constant_pack_index(index, elements.len()) else {
            return self.lower_runtime_pack_index(ctx, dest, base, index, &elements);
        };
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

    /// The element a constant pack index selects, when it is in `0..len`.
    pub(super) fn constant_pack_index(&self, index: Reg, len: usize) -> Option<usize> {
        let value = match self.pending_literals.get(&index.0) {
            Some(PendingLiteral::Int(literal)) => literal.to_i64(),
            _ => self.int_consts.get(&index.0).copied(),
        }?;
        usize::try_from(value).ok().filter(|value| *value < len)
    }

    /// A runtime (or out-of-range literal) index into homogeneous pack
    /// storage: the VM's `bounds_check` as a trap on an index outside
    /// `0..len` (one unsigned compare), then the element at
    /// `index * stride`. A read the instance never executes must not
    /// reject it, so an empty pack's read compiles to the trap alone.
    pub(super) fn lower_runtime_pack_index(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
        elements: &[Ty],
    ) -> Result<(), PlironError> {
        self.emit_index_guard(ctx, index, elements.len(), dest)?;
        let Some(element) = elements.first() else {
            return self.define_zeroed(ctx, dest);
        };
        let stride = self.homogeneous_pack_stride(elements, "runtime pack index", dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let position = self.reg_value(ctx, index, ScalarTy::Int)?;
        let address = self.strided_element_address(ctx, base_ptr, position, stride, dest);
        self.load_from(ctx, address, element, dest)
    }

    pub(super) fn emit_index_guard(
        &mut self,
        ctx: &mut Context,
        index: Reg,
        width: usize,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let value = self.reg_value(ctx, index, ScalarTy::Int)?;
        let limit = self.int_constant(ctx, width as i64);
        self.emit_bound_guard(ctx, value, limit, dest)
    }

    /// Trap unless `0 <= index < limit` (one unsigned compare): the VM's
    /// `bounds_check`.
    pub(super) fn emit_bound_guard(
        &mut self,
        ctx: &mut Context,
        value: Value,
        limit: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let invalid = ICmpOp::new(ctx, ICmpPredicateAttr::UGE, value, limit);
        self.append(ctx, invalid.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            invalid.get_result(ctx),
            TrapCategory::UnhandledError,
            dest,
        )
    }
}
