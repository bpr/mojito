//! SIMD lowering: constructors, casts, shuffles, lane conversions,
//! methods/reductions/select, and elementwise unary/binary operators.

use super::*;

impl<'a> FnLowering<'a> {
    /// Construct a SIMD value with the VM's per-lane conversions. Width-one
    /// aliases remain SSA scalars; wider values use contiguous scalar storage.
    pub(super) fn lower_make_simd(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        dtype: Dtype,
        width: usize,
        elems: &[Reg],
    ) -> Result<(), PlironError> {
        if elems.len() != 1 && elems.len() != width {
            return Err(self.unsupported_reg(
                format!(
                    "SIMD construction with {} elements for width {width}",
                    elems.len()
                ),
                dest,
            ));
        }
        let target = ScalarTy::of_dtype(dtype);
        if width > 1 {
            let ty = Ty::Simd {
                dtype,
                width: width as i64,
            };
            let layout = self
                .layout
                .layout_of(&ty)
                .map_err(|error| self.unsupported_reg(format!("SIMD layout ({error})"), dest))?;
            let lane_layout = self
                .layout
                .layout_of(&Ty::Simd { dtype, width: 1 })
                .expect("SIMD lane has a native layout");
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            for lane in 0..width {
                let elem = elems[if elems.len() == 1 { 0 } else { lane }];
                let converted = self.simd_constructor_lane(ctx, elem, target, dest)?;
                let address = self.offset_address(ctx, storage, lane_layout.size * lane as u64);
                let store = StoreOp::new(ctx, converted, address);
                self.append(ctx, store.get_operation(), Some(dest));
            }
            self.reg_values.insert(dest.0, storage);
            return Ok(());
        }
        let elem = elems[0];
        let converted = self.simd_constructor_lane(ctx, elem, target, dest)?;
        self.reg_values.insert(dest.0, converted);
        Ok(())
    }

    pub(super) fn simd_constructor_lane(
        &mut self,
        ctx: &mut Context,
        elem: Reg,
        target: ScalarTy,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        // A literal element folds with the exact conversions (integers wrap
        // at the lane width, `Float32` rounds from the exact rational).
        if let Some(literal) = self.pending_literals.get(&elem.0).cloned() {
            return self.materialize_pending(ctx, &literal, target, dest);
        }
        if let Some(value) = self.intable_struct_value(ctx, elem, dest)? {
            return self.convert_lane(ctx, ScalarTy::Int, target, value, dest);
        }
        let source = match self.concrete_scalar_ty(elem)? {
            Some(ty) => ty,
            None => match self.func.reg_types.get(&elem.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, elem, source)?;
        self.convert_lane(ctx, source, target, value, dest)
    }

    /// Invoke a concrete nominal `__int__` selected by the checker for a
    /// scalar constructor operand. Returns `None` for non-struct operands.
    pub(super) fn intable_struct_value(
        &mut self,
        ctx: &mut Context,
        source: Reg,
        anchor: Reg,
    ) -> Result<Option<Value>, PlironError> {
        let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&source.0).cloned() else {
            return Ok(None);
        };
        let method = format!("{name}.__int__");
        let Some(signature) = self.signatures.get(&method) else {
            return Err(
                self.unsupported_reg(format!("`{name}` without compiled `__int__`"), anchor)
            );
        };
        if signature.outcome.is_some() || signature.sret.is_some() {
            return Err(self.unsupported_reg(format!("non-scalar `{method}`"), anchor));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let receiver = self.reg_ptr(ctx, source)?;
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            signature.func_ty,
            vec![receiver],
        );
        self.append(ctx, call.get_operation(), Some(anchor));
        Ok(Some(call.get_result(ctx)))
    }

    /// `SimdCast` (`x.cast[DType.<dt>]()`) — the VM's
    /// `runtime::simd_cast`: int→int rewraps at the new width, int→float
    /// converts through f64 (`Float32` rounds), float→float widens or
    /// rounds, and float→int truncates toward zero saturating at the
    /// 128-bit intermediate before wrapping — saturation must happen at
    /// i128, not the target width, or large magnitudes wrap differently
    /// than the VM. Bool casts reject (VM parity); multi-lane casts stay
    /// i128, not the target width, or large magnitudes wrap differently.
    pub(super) fn lower_simd_cast(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        if dtype == Dtype::Bool {
            return Err(self.unsupported_reg("bool SIMD dtype cast".into(), dest));
        }
        if width > 1 {
            let Some(Ty::Simd {
                dtype: source_dtype,
                width: source_width,
            }) = self.func.reg_types.get(&value.0).cloned()
            else {
                return Err(self.unsupported_reg("SIMD cast source type".into(), dest));
            };
            if source_width != width as i64 {
                return Err(self.unsupported_reg("SIMD cast width mismatch".into(), dest));
            }
            let source_ty = ScalarTy::of_dtype(source_dtype);
            if matches!(source_ty, ScalarTy::Bool | ScalarTy::Ptr) {
                return Err(self.unsupported_reg("SIMD cast of a Bool operand".into(), dest));
            }
            let source_ptr = self.reg_ptr(ctx, value)?;
            let source_lane = self
                .layout
                .layout_of(&Ty::Simd {
                    dtype: source_dtype,
                    width: 1,
                })
                .expect("SIMD lane layout");
            let target_ty = Ty::Simd {
                dtype,
                width: width as i64,
            };
            let target_layout = self.layout.layout_of(&target_ty).expect("SIMD layout");
            let target_lane = self
                .layout
                .layout_of(&Ty::Simd { dtype, width: 1 })
                .expect("SIMD lane layout");
            let storage = self.entry_alloca(ctx, target_layout.size, target_layout.align);
            let source_handle = source_ty.handle(ctx);
            for lane in 0..width {
                let source_address =
                    self.offset_address(ctx, source_ptr, source_lane.size * lane as u64);
                let load = LoadOp::new(ctx, source_address, source_handle);
                self.append(ctx, load.get_operation(), Some(dest));
                let converted =
                    self.simd_cast_lane(ctx, load.get_result(ctx), source_ty, dtype, dest)?;
                let target_address =
                    self.offset_address(ctx, storage, target_lane.size * lane as u64);
                let store = StoreOp::new(ctx, converted, target_address);
                self.append(ctx, store.get_operation(), Some(dest));
            }
            self.reg_values.insert(dest.0, storage);
            return Ok(());
        }
        let source = self.concrete_scalar_ty(value)?.ok_or_else(|| {
            self.unsupported_reg("SIMD cast of an unmaterialized literal".into(), dest)
        })?;
        if matches!(source, ScalarTy::Bool | ScalarTy::Ptr) {
            return Err(
                self.unsupported_reg(format!("SIMD cast of a {} operand", source.name()), dest)
            );
        }
        let lane = self.reg_value(ctx, value, source)?;
        let converted = self.simd_cast_lane(ctx, lane, source, dtype, dest)?;
        self.reg_values.insert(dest.0, converted);
        Ok(())
    }

    pub(super) fn simd_cast_lane(
        &mut self,
        ctx: &mut Context,
        lane: Value,
        source: ScalarTy,
        dtype: Dtype,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let target = ScalarTy::of_dtype(dtype);
        Ok(match target {
            ScalarTy::Float64 => self.lane_to_f64(ctx, source, lane, dest)?,
            ScalarTy::Sized(Dtype::Float32) => {
                let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                self.f64_to_f32(ctx, wide, dest)
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("bool targets are rejected above");
                match source.int_shape() {
                    Some(from) => self.resize_int(ctx, lane, from, to_bits, dest),
                    // Float source: truncate toward zero, saturating at the
                    // 128-bit intermediate (Rust `as i128`, NaN → 0), then
                    // wrap to the lane width.
                    None => {
                        let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                        let saturated = self.fptosi_sat_i128(ctx, wide, dest);
                        self.resize_int(ctx, saturated, (128, true), to_bits, dest)
                    }
                }
            }
        })
    }

    pub(super) fn lower_simd_shuffle(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        mask: &[usize],
    ) -> Result<(), PlironError> {
        let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg("SIMD shuffle source type".into(), dest));
        };
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane_handle = lane_ty.handle(ctx);
        let source = self.reg_ptr(ctx, value)?;
        let lane = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        if mask.len() == 1 {
            let address = self.offset_address(ctx, source, lane.size * mask[0] as u64);
            let load = LoadOp::new(ctx, address, lane_handle);
            return self.define(ctx, dest, load.get_operation(), load.get_result(ctx));
        }
        if mask.iter().any(|index| *index >= width as usize) {
            return Err(self.unsupported_reg("SIMD shuffle index out of range".into(), dest));
        }
        let result_ty = Ty::Simd {
            dtype,
            width: mask.len() as i64,
        };
        let layout = self
            .layout
            .layout_of(&result_ty)
            .expect("SIMD result layout");
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for (result_lane, source_lane) in mask.iter().enumerate() {
            let source_address = self.offset_address(ctx, source, lane.size * *source_lane as u64);
            let load = LoadOp::new(ctx, source_address, lane_handle);
            self.append(ctx, load.get_operation(), Some(dest));
            let target_address = self.offset_address(ctx, storage, lane.size * result_lane as u64);
            let store = StoreOp::new(ctx, load.get_result(ctx), target_address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// One scalar value as a `target` SIMD lane — the VM's lane builders:
    /// integer lanes wrap the source's mathematical value at the lane width
    /// (`value_to_int_lane`; Bool reads as 0/1), float lanes convert through
    /// f64 with `Float32` rounding (`value_to_float_lane`), bool lanes only
    /// accept Bool. Sources the VM cannot read as the lane's kind reject.
    pub(super) fn convert_lane(
        &mut self,
        ctx: &mut Context,
        source: ScalarTy,
        target: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        match target {
            ScalarTy::Bool => {
                match source {
                    ScalarTy::Bool => Ok(value),
                    other => Err(self
                        .unsupported_reg(format!("{} as a bool SIMD element", other.name()), dest)),
                }
            }
            ScalarTy::Float64 => self.lane_to_f64(ctx, source, value, dest),
            ScalarTy::Sized(Dtype::Float32) => {
                let wide = self.lane_to_f64(ctx, source, value, dest)?;
                Ok(self.f64_to_f32(ctx, wide, dest))
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("of_dtype yields scalars, Bool, or floats only");
                let widened = match source {
                    // `value_to_int` reads Bool as 0/1.
                    ScalarTy::Bool => {
                        let i64_ty: TypeHandle =
                            IntegerType::get(ctx, 64, Signedness::Signless).into();
                        let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                        self.append(ctx, cast.get_operation(), Some(dest));
                        (cast.get_result(ctx), (64, false))
                    }
                    other => match other.int_shape() {
                        Some(from) => (value, from),
                        None => {
                            return Err(self.unsupported_reg(
                                format!("{} as an integer SIMD element", other.name()),
                                dest,
                            ));
                        }
                    },
                };
                let (value, from) = widened;
                Ok(self.resize_int(ctx, value, from, to_bits, dest))
            }
        }
    }

    /// One scalar value's floating content as f64 (the VM's
    /// `value_to_float`): integers convert by signedness, a `Float32` widens
    /// to its exact f64 view, Bool and pointers reject.
    pub(super) fn lane_to_f64(
        &mut self,
        ctx: &mut Context,
        source: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        match source {
            ScalarTy::Float64 => Ok(value),
            ScalarTy::Sized(Dtype::Float32) => Ok(self.f32_to_f64(ctx, value, dest)),
            ScalarTy::Int => Ok(self.int_to_f64(ctx, value, dest)),
            ScalarTy::UInt => Ok(self.uint_to_f64(ctx, value, dest)),
            ScalarTy::Sized(dtype) => {
                let (_, signed) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("Float32 is matched above");
                let wide = self.sized_to_i64(ctx, value, dtype, dest);
                Ok(if signed {
                    self.int_to_f64(ctx, wide, dest)
                } else {
                    self.uint_to_f64(ctx, wide, dest)
                })
            }
            other => {
                Err(self.unsupported_reg(format!("{} as a float SIMD element", other.name()), dest))
            }
        }
    }

    /// `llvm.fptosi.sat.i128.f64` — Rust's saturating `as i128` on an f64
    /// (NaN becomes 0, infinities clamp to the i128 bounds).
    pub(super) fn fptosi_sat_i128(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let i128_ty: TypeHandle = IntegerType::get(ctx, 128, Signedness::Signless).into();
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let fn_ty = FuncType::get(ctx, i128_ty, vec![f64_ty], false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.fptosi.sat.i128.f64".to_string()),
            fn_ty,
            vec![value],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_simd_method(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        dtype: Dtype,
        width: usize,
        method: &str,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        match method {
            "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max" | "reduce_and"
            | "reduce_or"
                if args.is_empty() =>
            {
                self.lower_simd_reduce(ctx, dest, recv, dtype, width, method)
            }
            "select" if dtype == Dtype::Bool && args.len() == 2 => {
                self.lower_simd_select(ctx, dest, recv, args[0], args[1], width)
            }
            _ => Err(self.unsupported_reg(format!("SIMD method `{method}`"), dest)),
        }
    }

    pub(super) fn lower_simd_reduce(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        dtype: Dtype,
        width: usize,
        method: &str,
    ) -> Result<(), PlironError> {
        let lane_ty = ScalarTy::of_dtype(dtype);
        let handle = lane_ty.handle(ctx);
        let lane_layout = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        let ptr = self.reg_ptr(ctx, recv)?;
        let first = LoadOp::new(ctx, ptr, handle);
        self.append(ctx, first.get_operation(), Some(dest));
        let mut accumulator = first.get_result(ctx);
        for lane in 1..width {
            let address = self.offset_address(ctx, ptr, lane_layout.size * lane as u64);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            let next = load.get_result(ctx);
            accumulator = match method {
                "reduce_add" => {
                    if matches!(lane_ty, ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32)) {
                        let add = FAddOp::new_with_fast_math_flags(
                            ctx,
                            accumulator,
                            next,
                            FastmathFlagsAttr::default(),
                        );
                        self.append(ctx, add.get_operation(), Some(dest));
                        add.get_result(ctx)
                    } else {
                        let add = AddOp::new_with_overflow_flag(
                            ctx,
                            accumulator,
                            next,
                            no_overflow_flags(),
                        );
                        self.append(ctx, add.get_operation(), Some(dest));
                        add.get_result(ctx)
                    }
                }
                "reduce_mul" => {
                    if matches!(lane_ty, ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32)) {
                        let mul = FMulOp::new_with_fast_math_flags(
                            ctx,
                            accumulator,
                            next,
                            FastmathFlagsAttr::default(),
                        );
                        self.append(ctx, mul.get_operation(), Some(dest));
                        mul.get_result(ctx)
                    } else {
                        let mul = MulOp::new_with_overflow_flag(
                            ctx,
                            accumulator,
                            next,
                            no_overflow_flags(),
                        );
                        self.append(ctx, mul.get_operation(), Some(dest));
                        mul.get_result(ctx)
                    }
                }
                "reduce_and" => {
                    let and = AndOp::new(ctx, accumulator, next);
                    self.append(ctx, and.get_operation(), Some(dest));
                    and.get_result(ctx)
                }
                "reduce_or" => {
                    let or = OrOp::new(ctx, accumulator, next);
                    self.append(ctx, or.get_operation(), Some(dest));
                    or.get_result(ctx)
                }
                "reduce_min" | "reduce_max" => {
                    let is_min = method == "reduce_min";
                    let predicate = match lane_ty {
                        ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => None,
                        ScalarTy::Sized(kind) => {
                            let signed = mojito_vm::runtime::integer_dtype_bits(kind)
                                .is_some_and(|(_, signed)| signed);
                            Some(match (is_min, signed) {
                                (true, true) => ICmpPredicateAttr::SLT,
                                (false, true) => ICmpPredicateAttr::SGT,
                                (true, false) => ICmpPredicateAttr::ULT,
                                (false, false) => ICmpPredicateAttr::UGT,
                            })
                        }
                        _ => None,
                    };
                    let condition = if let Some(predicate) = predicate {
                        let cmp = ICmpOp::new(ctx, predicate, next, accumulator);
                        self.append(ctx, cmp.get_operation(), Some(dest));
                        cmp.get_result(ctx)
                    } else {
                        let predicate = if is_min {
                            FCmpPredicateAttr::OLT
                        } else {
                            FCmpPredicateAttr::OGT
                        };
                        let cmp = self.fcmp(ctx, predicate, next, accumulator);
                        self.append(ctx, cmp.get_operation(), Some(dest));
                        cmp.get_result(ctx)
                    };
                    let select = SelectOp::new(ctx, condition, next, accumulator);
                    self.append(ctx, select.get_operation(), Some(dest));
                    select.get_result(ctx)
                }
                _ => unreachable!(),
            };
        }
        self.reg_values.insert(dest.0, accumulator);
        Ok(())
    }

    pub(super) fn lower_simd_select(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        mask: Reg,
        yes: Reg,
        no: Reg,
        width: usize,
    ) -> Result<(), PlironError> {
        let Some(Ty::Simd { dtype, .. }) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("SIMD select result type".into(), dest));
        };
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane_handle = lane_ty.handle(ctx);
        let lane_layout = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        let mask_ptr = self.reg_ptr(ctx, mask)?;
        let yes_ptr = self.reg_ptr(ctx, yes)?;
        let no_ptr = if matches!(self.func.reg_types.get(&no.0), Some(Ty::Simd { width, .. }) if *width > 1)
        {
            Some(self.reg_ptr(ctx, no)?)
        } else {
            None
        };
        let no_splat = if no_ptr.is_none() {
            if let Some(literal) = self.pending_literals.get(&no.0).cloned() {
                Some(self.materialize_pending(ctx, &literal, lane_ty, dest)?)
            } else {
                let source = self
                    .concrete_scalar_ty(no)?
                    .ok_or_else(|| self.unsupported_reg("SIMD select splat".into(), dest))?;
                let value = self.reg_value(ctx, no, source)?;
                Some(self.convert_lane(ctx, source, lane_ty, value, dest)?)
            }
        } else {
            None
        };
        let layout = self
            .layout
            .layout_of(&Ty::Simd {
                dtype,
                width: width as i64,
            })
            .expect("SIMD layout");
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        let bool_handle = ScalarTy::Bool.handle(ctx);
        for lane in 0..width {
            let offset = lane_layout.size * lane as u64;
            let mask_address = self.offset_address(ctx, mask_ptr, lane as u64);
            let condition = LoadOp::new(ctx, mask_address, bool_handle);
            self.append(ctx, condition.get_operation(), Some(dest));
            let yes_address = self.offset_address(ctx, yes_ptr, offset);
            let yes_value = LoadOp::new(ctx, yes_address, lane_handle);
            self.append(ctx, yes_value.get_operation(), Some(dest));
            let no_value = if let Some(ptr) = no_ptr {
                let address = self.offset_address(ctx, ptr, offset);
                let load = LoadOp::new(ctx, address, lane_handle);
                self.append(ctx, load.get_operation(), Some(dest));
                load.get_result(ctx)
            } else {
                no_splat.expect("SIMD select splat")
            };
            let select = SelectOp::new(
                ctx,
                condition.get_result(ctx),
                yes_value.get_result(ctx),
                no_value,
            );
            self.append(ctx, select.get_operation(), Some(dest));
            let target = self.offset_address(ctx, storage, offset);
            let store = StoreOp::new(ctx, select.get_result(ctx), target);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    pub(super) fn lower_simd_unop(
        &mut self,
        ctx: &mut Context,
        op: PrefixOp,
        dest: Reg,
        operand: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane_handle = lane_ty.handle(ctx);
        let lane_layout = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        let layout = self
            .layout
            .layout_of(&Ty::Simd {
                dtype,
                width: width as i64,
            })
            .expect("SIMD layout");
        let source = self.reg_ptr(ctx, operand)?;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for lane in 0..width {
            let address = self.offset_address(ctx, source, lane_layout.size * lane as u64);
            let load = LoadOp::new(ctx, address, lane_handle);
            self.append(ctx, load.get_operation(), Some(dest));
            let value = load.get_result(ctx);
            let result = match (op, lane_ty) {
                (PrefixOp::Neg, ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32)) => {
                    let neg =
                        FNegOp::new_with_fast_math_flags(ctx, value, FastmathFlagsAttr::default());
                    self.append(ctx, neg.get_operation(), Some(dest));
                    neg.get_result(ctx)
                }
                (PrefixOp::Neg, ScalarTy::Sized(kind)) => {
                    let zero = self.sized_int_constant(ctx, kind, 0);
                    let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                    self.append(ctx, neg.get_operation(), Some(dest));
                    neg.get_result(ctx)
                }
                (PrefixOp::Invert, ScalarTy::Sized(kind)) if !kind.is_float() => {
                    let ones = self.sized_int_constant(ctx, kind, u64::MAX);
                    let inverted = XorOp::new(ctx, value, ones);
                    self.append(ctx, inverted.get_operation(), Some(dest));
                    inverted.get_result(ctx)
                }
                (PrefixOp::Invert, ScalarTy::Bool) => {
                    let one = self.bool_constant(ctx, true);
                    let inverted = XorOp::new(ctx, value, one);
                    self.append(ctx, inverted.get_operation(), Some(dest));
                    inverted.get_result(ctx)
                }
                _ => {
                    return Err(self.unsupported_reg(format!("SIMD unary operator `{op:?}`"), dest));
                }
            };
            let target = self.offset_address(ctx, storage, lane_layout.size * lane as u64);
            let store = StoreOp::new(ctx, result, target);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_simd_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane_handle = lane_ty.handle(ctx);
        let result_dtype = if is_comparison(op) {
            Dtype::Bool
        } else {
            dtype
        };
        let source_lane = self
            .layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane layout");
        let result_lane = self
            .layout
            .layout_of(&Ty::Simd {
                dtype: result_dtype,
                width: 1,
            })
            .expect("SIMD lane layout");
        let layout = self
            .layout
            .layout_of(&Ty::Simd {
                dtype: result_dtype,
                width: width as i64,
            })
            .expect("SIMD layout");
        let lhs_ptr = self.reg_ptr(ctx, a)?;
        let rhs_ptr = if matches!(self.func.reg_types.get(&b.0), Some(Ty::Simd { width, .. }) if *width > 1)
        {
            Some(self.reg_ptr(ctx, b)?)
        } else {
            None
        };
        let rhs_splat = if rhs_ptr.is_none() {
            if let Some(literal) = self.pending_literals.get(&b.0).cloned() {
                Some(self.materialize_pending(ctx, &literal, lane_ty, dest)?)
            } else {
                let source = self.concrete_scalar_ty(b)?.ok_or_else(|| {
                    self.unsupported_reg("unmaterialized SIMD splat operand".into(), dest)
                })?;
                let value = self.reg_value(ctx, b, source)?;
                Some(self.convert_lane(ctx, source, lane_ty, value, dest)?)
            }
        } else {
            None
        };
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for lane in 0..width {
            let offset = source_lane.size * lane as u64;
            let lhs_address = self.offset_address(ctx, lhs_ptr, offset);
            let lhs = LoadOp::new(ctx, lhs_address, lane_handle);
            self.append(ctx, lhs.get_operation(), Some(dest));
            let rhs = if let Some(rhs_ptr) = rhs_ptr {
                let rhs_address = self.offset_address(ctx, rhs_ptr, offset);
                let rhs = LoadOp::new(ctx, rhs_address, lane_handle);
                self.append(ctx, rhs.get_operation(), Some(dest));
                rhs.get_result(ctx)
            } else {
                rhs_splat.expect("scalar SIMD operand was materialized")
            };
            if is_comparison(op) {
                self.lower_compare(ctx, op, dest, lhs.get_result(ctx), rhs, lane_ty)?;
            } else {
                match lane_ty {
                    ScalarTy::Sized(Dtype::Float32) => {
                        self.lower_f32_binop(ctx, op, dest, lhs.get_result(ctx), rhs)?
                    }
                    ScalarTy::Float64 => {
                        self.lower_float_binop(ctx, op, dest, lhs.get_result(ctx), rhs)?
                    }
                    ScalarTy::Sized(kind) => {
                        self.lower_sized_int_binop(ctx, op, dest, lhs.get_result(ctx), rhs, kind)?
                    }
                    _ => {
                        return Err(
                            self.unsupported_reg(format!("SIMD binary operator `{op:?}`"), dest)
                        );
                    }
                }
            }
            let result = self.reg_values[&dest.0];
            let target = self.offset_address(ctx, storage, result_lane.size * lane as u64);
            let store = StoreOp::new(ctx, result, target);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }
}
