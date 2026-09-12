//! SIMD lowering: constructors, casts, shuffles, lane conversions,
//! methods/reductions/select, and elementwise unary/binary operators.
//!
//! Multi-lane values compute as LLVM fixed vectors (`<N x lane>`) in SSA and
//! rest in lane-aligned storage between operations: the storage and call ABI
//! stay the `LayoutCx` aggregate (`docs/native-abi.md`), so every vector
//! load and store declares the *lane* alignment, Bool lanes convert between
//! `<N x i1>` compute and byte-per-lane storage at that boundary, and
//! width-one aliases stay scalars. A value's storage is touched at its base
//! only by whole-vector typed loads and stores — pliron's mem2reg forwards
//! a store to a load at the same pointer without comparing their types — so
//! a lane read extracts from the loaded vector and a lane write inserts into
//! it and stores the whole vector back, rather than addressing a lane by
//! byte offset.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl FnLowering<'_> {
    /// Construct a SIMD value with the VM's per-lane conversions. Width-one
    /// aliases remain SSA scalars; wider values assemble a vector lane by
    /// lane (one element splats).
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
        if width == 1 {
            let converted = self.simd_constructor_lane(ctx, elems[0], target, dest)?;
            self.reg_values.insert(dest.0, converted);
            return Ok(());
        }
        let vector = if elems.len() == 1 {
            let lane = self.simd_constructor_lane(ctx, elems[0], target, dest)?;
            self.simd_splat_value(ctx, lane, width, dest)
        } else {
            let vector_ty = self.simd_vector_ty(ctx, dtype, width);
            let poison = PoisonOp::new(ctx, vector_ty);
            self.append(ctx, poison.get_operation(), Some(dest));
            let mut vector = poison.get_result(ctx);
            for (index, elem) in elems.iter().enumerate() {
                let lane = self.simd_constructor_lane(ctx, *elem, target, dest)?;
                let position = self.int_constant(ctx, index as i64);
                let insert = InsertElementOp::new(ctx, vector, lane, position);
                self.append(ctx, insert.get_operation(), Some(dest));
                vector = insert.get_result(ctx);
            }
            vector
        };
        self.simd_store_vector(ctx, dest, dtype, width, vector);
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
    /// than the VM. Bool casts reject (VM parity).
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
            let source = self.simd_load_vector(ctx, value, source_dtype, width, dest)?;
            let converted = self.simd_cast_vector(ctx, source, source_ty, dtype, width, dest)?;
            self.simd_store_vector(ctx, dest, dtype, width, converted);
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

    /// `SimdBitcast` (`x.to_bits[DType.<dt>]()`) — the VM's
    /// `runtime::simd_to_bits`: each lane's bit pattern zero-extended into
    /// the unsigned target lane (floats through `bitcast`, `bool` as 0/1).
    pub(super) fn lower_simd_bitcast(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        if width > 1 {
            let Some(Ty::Simd {
                dtype: source_dtype,
                width: source_width,
            }) = self.func.reg_types.get(&value.0).cloned()
            else {
                return Err(self.unsupported_reg("SIMD to_bits source type".into(), dest));
            };
            if source_width != width as i64 {
                return Err(self.unsupported_reg("SIMD to_bits width mismatch".into(), dest));
            }
            let source_ty = ScalarTy::of_dtype(source_dtype);
            let source = self.simd_load_vector(ctx, value, source_dtype, width, dest)?;
            let bits = self.simd_bits_lanes(ctx, source, source_ty, dtype, Some(width), dest)?;
            self.simd_store_vector(ctx, dest, dtype, width, bits);
            return Ok(());
        }
        let source = self.concrete_scalar_ty(value)?.ok_or_else(|| {
            self.unsupported_reg("SIMD to_bits of an unmaterialized literal".into(), dest)
        })?;
        let lane = self.reg_value(ctx, value, source)?;
        let bits = self.simd_bits_lanes(ctx, lane, source, dtype, None, dest)?;
        self.reg_values.insert(dest.0, bits);
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
                if let Some(from) = source.int_shape() {
                    self.resize_int(ctx, lane, from, to_bits, dest)
                } else {
                    let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                    let saturated = self.fptosi_sat_i128(ctx, wide, dest);
                    self.resize_int(ctx, saturated, (128, true), to_bits, dest)
                }
            }
        })
    }

    /// A compile-time lane gather: a one-lane mask extracts the scalar, a
    /// wider mask is one `shufflevector` (the checker bounds every index).
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
        if mask.iter().any(|index| *index >= width as usize) {
            return Err(self.unsupported_reg("SIMD shuffle index out of range".into(), dest));
        }
        let source = self.simd_load_vector(ctx, value, dtype, width as usize, dest)?;
        if mask.len() == 1 {
            let position = self.int_constant(ctx, mask[0] as i64);
            let extract = ExtractElementOp::new(ctx, source, position);
            return self.define(ctx, dest, extract.get_operation(), extract.get_result(ctx));
        }
        let shuffle = ShuffleVectorOp::new(
            ctx,
            source,
            source,
            mask.iter().map(|index| *index as i32).collect(),
        );
        self.append(ctx, shuffle.get_operation(), Some(dest));
        self.simd_store_vector(ctx, dest, dtype, mask.len(), shuffle.get_result(ctx));
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

    /// The six reductions (`runtime::simd_reduce`): a strict left fold in
    /// the VM. Integer add/mul wrap per step, so the wrapping vector
    /// reductions are exact; min/max select the signed or unsigned family;
    /// the bool reductions are `and`/`or` over `<N x i1>`; float add/mul
    /// use the *ordered* `llvm.vector.reduce.fadd/fmul` from the exact
    /// identity (`-0.0`, `1.0`) with no reassociation, which is the VM's
    /// fold; float min/max fold lane by lane through `minnum`/`maxnum`
    /// (Rust `f64::min`/`max`, NaN-quieting).
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
        // A width-1 vector is a scalar register: every reduction is the
        // lane itself.
        if width == 1 {
            let value = self.reg_value(ctx, recv, lane_ty)?;
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }
        let vector = self.simd_load_vector(ctx, recv, dtype, width, dest)?;
        let lane_handle = lane_ty.handle(ctx);
        let vector_ty = self.simd_vector_ty(ctx, dtype, width);
        let is_float = lane_ty.int_shape().is_none() && lane_ty != ScalarTy::Bool;
        let value = match method {
            "reduce_min" | "reduce_max" if is_float => {
                let is_min = method == "reduce_min";
                let mut accumulator = self.simd_extract(ctx, vector, 0, dest);
                for lane in 1..width {
                    let next = self.simd_extract(ctx, vector, lane, dest);
                    accumulator = self.float_min_max(ctx, lane_ty, is_min, accumulator, next, dest);
                }
                accumulator
            }
            "reduce_add" | "reduce_mul" if is_float => {
                let (name, start) = match (method, lane_ty) {
                    ("reduce_add", ScalarTy::Float64) => ("fadd", self.float_constant(ctx, -0.0)),
                    ("reduce_add", _) => ("fadd", self.f32_constant(ctx, -0.0)),
                    (_, ScalarTy::Float64) => ("fmul", self.float_constant(ctx, 1.0)),
                    _ => ("fmul", self.f32_constant(ctx, 1.0)),
                };
                let fn_ty = FuncType::get(ctx, lane_handle, vec![lane_handle, vector_ty], false);
                let name = format!("llvm.vector.reduce.{name}.{}", simd_mangle(dtype, width));
                let call =
                    CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, vec![start, vector]);
                self.append(ctx, call.get_operation(), Some(dest));
                call.get_result(ctx)
            }
            _ => {
                let name = match (method, lane_ty.int_shape()) {
                    ("reduce_add", Some(_)) => "add",
                    ("reduce_mul", Some(_)) => "mul",
                    ("reduce_min", Some((_, true))) => "smin",
                    ("reduce_max", Some((_, true))) => "smax",
                    ("reduce_min", Some((_, false))) => "umin",
                    ("reduce_max", Some((_, false))) => "umax",
                    ("reduce_and", None) if lane_ty == ScalarTy::Bool => "and",
                    ("reduce_or", None) if lane_ty == ScalarTy::Bool => "or",
                    _ => {
                        return Err(self.unsupported_reg(
                            format!("SIMD `{method}` on {} lanes", lane_ty.name()),
                            dest,
                        ));
                    }
                };
                let fn_ty = FuncType::get(ctx, lane_handle, vec![vector_ty], false);
                let name = format!("llvm.vector.reduce.{name}.{}", simd_mangle(dtype, width));
                let call = CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, vec![vector]);
                self.append(ctx, call.get_operation(), Some(dest));
                call.get_result(ctx)
            }
        };
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// `mask.select(yes, no)` — one vector `select` over the `<N x i1>`
    /// mask; either case may be a splatting scalar (`runtime::simd_select`).
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
        let condition = self.simd_load_vector(ctx, mask, Dtype::Bool, width, dest)?;
        let yes_value = self.simd_operand_vector(ctx, yes, dtype, width, dest)?;
        let no_value = self.simd_operand_vector(ctx, no, dtype, width, dest)?;
        let select = SelectOp::new(ctx, condition, yes_value, no_value);
        self.append(ctx, select.get_operation(), Some(dest));
        self.simd_store_vector(ctx, dest, dtype, width, select.get_result(ctx));
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
        let vector = self.simd_load_vector(ctx, operand, dtype, width, dest)?;
        let result = match (op, lane_ty) {
            (PrefixOp::Neg, ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32)) => {
                let neg =
                    FNegOp::new_with_fast_math_flags(ctx, vector, FastmathFlagsAttr::default());
                self.append(ctx, neg.get_operation(), Some(dest));
                neg.get_result(ctx)
            }
            (PrefixOp::Neg, _) if lane_ty.int_shape().is_some() => {
                let zero = self.lane_constant(ctx, dtype, 0, Some(width), dest);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, vector, no_overflow_flags());
                self.append(ctx, neg.get_operation(), Some(dest));
                neg.get_result(ctx)
            }
            (PrefixOp::Invert, _) if lane_ty.int_shape().is_some() => {
                let ones = self.lane_constant(ctx, dtype, u64::MAX, Some(width), dest);
                let inverted = XorOp::new(ctx, vector, ones);
                self.append(ctx, inverted.get_operation(), Some(dest));
                inverted.get_result(ctx)
            }
            (PrefixOp::Invert, ScalarTy::Bool) => {
                let one = self.bool_constant(ctx, true);
                let ones = self.simd_splat_value(ctx, one, width, dest);
                let inverted = XorOp::new(ctx, vector, ones);
                self.append(ctx, inverted.get_operation(), Some(dest));
                inverted.get_result(ctx)
            }
            _ => {
                return Err(self.unsupported_reg(format!("SIMD unary operator `{op:?}`"), dest));
            }
        };
        self.simd_store_vector(ctx, dest, dtype, width, result);
        Ok(())
    }

    /// An elementwise binary operator over compute vectors: either side may
    /// be a splatting scalar/literal; comparisons yield a Bool vector.
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
        let lhs = self.simd_operand_vector(ctx, a, dtype, width, dest)?;
        let rhs = self.simd_operand_vector(ctx, b, dtype, width, dest)?;
        if is_comparison(op) {
            let mask = self.simd_compare_vectors(ctx, op, lane_ty, lhs, rhs, dest)?;
            self.simd_store_vector(ctx, dest, Dtype::Bool, width, mask);
            return Ok(());
        }
        let result = match lane_ty {
            ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => {
                self.simd_float_binop_vectors(ctx, op, lhs, rhs, dest)?
            }
            ScalarTy::Bool => self.simd_bool_binop_vectors(ctx, op, lhs, rhs, dest)?,
            // `DType.int` lanes are 64-bit signed lanes (the VM wraps them
            // at 64 bits like `Int64`), never the scalar `Int` path with
            // its zero-divisor trap.
            ScalarTy::Int | ScalarTy::UInt | ScalarTy::Sized(_) => {
                self.sized_int_binop_value(ctx, op, lhs, rhs, dtype, Some(width), dest)?
            }
            ScalarTy::Ptr => {
                return Err(self.unsupported_reg(format!("SIMD binary operator `{op:?}`"), dest));
            }
        };
        self.simd_store_vector(ctx, dest, dtype, width, result);
        Ok(())
    }

    /// A store into one lane of a multi-lane SIMD place — the lane read's
    /// mirror: load the whole vector, `insertelement`, store it back, so the
    /// designated storage is touched only at its base by whole-vector typed
    /// accesses (a byte-offset lane store is a non-promotable use that pins
    /// the slot in memory). Returns `false` for anything else, leaving the
    /// caller's lane-addressed path (`place_address`) untouched.
    pub(super) fn try_lower_simd_lane_store(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        src: Reg,
    ) -> Result<bool, PlironError> {
        let Some(&Proj::Index(index)) = place.proj.last() else {
            return Ok(false);
        };
        // The base type is read from the place's own projection types, so an
        // untyped compatibility place declines rather than guessing.
        if !place.is_typed() {
            return Ok(false);
        }
        let base_ty = match place.proj.len() {
            1 => place.root_ty.clone(),
            n => place.projection_tys.get(n - 2).cloned(),
        };
        let Some(Ty::Simd { dtype, width }) = base_ty else {
            return Ok(false);
        };
        let width = width as usize;
        if width <= 1 {
            return Ok(false);
        }
        // `place_address` derefs a reference at the top of each projection
        // step, so a base that is itself a reference would be left
        // undereferenced by truncating the place; and its recorded-prefix
        // branch accepts a prefix as long as the whole projection, which the
        // truncated place would silently disqualify.
        if let Some(through) = place.through.filter(|through| *through != place.root)
            && let Some(recorded) = self.reference_places.get(&through)
            && recorded.proj.len() >= place.proj.len()
        {
            return Ok(false);
        }
        let mut base = place.clone();
        base.proj.pop();
        base.projection_tys.pop();
        base.ty = Some(Ty::Simd {
            dtype,
            width: width as i64,
        });
        let (address, _) = self.place_address(ctx, &base, src)?;
        self.emit_simd_index_guard(ctx, index, width, src)?;
        let vector = self.simd_load_vector_from(ctx, address, dtype, width, src);
        let lane = self.reg_value(ctx, src, ScalarTy::of_dtype(dtype))?;
        // The guard above makes the dynamic insert index in range (an
        // out-of-range LLVM index is poison), as the lane read relies on.
        let position = self.reg_value(ctx, index, ScalarTy::Int)?;
        let insert = InsertElementOp::new(ctx, vector, lane, position);
        self.append(ctx, insert.get_operation(), Some(src));
        let inserted = insert.get_result(ctx);
        self.simd_store_vector_to(ctx, address, dtype, width, inserted, src);
        Ok(true)
    }

    // --- vector compute plumbing -------------------------------------------

    /// The `<width x lane>` compute type of a multi-lane value (`i1` lanes
    /// for Bool).
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "Op construction mutates the context"
    )]
    pub(super) fn simd_vector_ty(
        &mut self,
        ctx: &Context,
        dtype: Dtype,
        width: usize,
    ) -> TypeHandle {
        let lane = ScalarTy::of_dtype(dtype).handle(ctx);
        VectorType::get(ctx, lane, width as u32, VectorTypeKind::Fixed).into()
    }

    /// A multi-lane register's storage as its compute vector: one vector
    /// load at the lane alignment; Bool lanes load as bytes and compare
    /// non-zero into `<N x i1>`.
    pub(super) fn simd_load_vector(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let ptr = self.reg_ptr(ctx, reg)?;
        Ok(self.simd_load_vector_from(ctx, ptr, dtype, width, dest))
    }

    pub(super) fn simd_load_vector_from(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Value {
        let storage_ty = self.simd_storage_ty(ctx, dtype, width);
        let align = self.simd_lane_layout(dtype).align as u32;
        let load = LoadOp::new(ctx, ptr, storage_ty);
        load.set_alignment(ctx, align);
        self.append(ctx, load.get_operation(), Some(dest));
        let value = load.get_result(ctx);
        if dtype != Dtype::Bool {
            return value;
        }
        let zero = self.lane_constant(ctx, Dtype::UInt8, 0, Some(width), dest);
        let set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, value, zero);
        self.append(ctx, set.get_operation(), Some(dest));
        set.get_result(ctx)
    }

    /// Store a compute vector into fresh lane-aligned storage and define
    /// `dest` as that storage (Bool lanes widen to bytes first). Nothing
    /// else may store through this slot at its base: mem2reg forwards a
    /// same-pointer store to a load without comparing their types.
    pub(super) fn simd_store_vector(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        dtype: Dtype,
        width: usize,
        value: Value,
    ) {
        let storage = self.simd_storage_slot(ctx, dtype, width);
        self.simd_store_vector_to(ctx, storage, dtype, width, value, dest);
        self.reg_values.insert(dest.0, storage);
    }

    /// Fresh storage for a multi-lane value: typed at its storage vector so
    /// whole-value accesses stay promotable, aligned like one lane so the
    /// slot keeps the `LayoutCx` shape.
    pub(super) fn simd_storage_slot(
        &mut self,
        ctx: &mut Context,
        dtype: Dtype,
        width: usize,
    ) -> Value {
        let storage_ty = self.simd_storage_ty(ctx, dtype, width);
        let lane = self.simd_lane_layout(dtype);
        self.entry_typed_alloca_aligned(ctx, storage_ty, lane.align)
    }

    /// Store a compute vector into the lane-aligned storage at `ptr` (Bool
    /// lanes widen to bytes first) — [`simd_load_vector_from`]'s counterpart.
    pub(super) fn simd_store_vector_to(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        dtype: Dtype,
        width: usize,
        value: Value,
        anchor: Reg,
    ) {
        let storage_ty = self.simd_storage_ty(ctx, dtype, width);
        let lane = self.simd_lane_layout(dtype);
        let value = if dtype == Dtype::Bool {
            let widened = ZExtOp::new_with_nneg(ctx, value, storage_ty, false);
            self.append(ctx, widened.get_operation(), Some(anchor));
            widened.get_result(ctx)
        } else {
            value
        };
        let store = StoreOp::new(ctx, value, ptr);
        store.set_alignment(ctx, lane.align as u32);
        self.append(ctx, store.get_operation(), Some(anchor));
    }

    /// Move a whole multi-lane value between two lane-aligned storages as
    /// one typed vector load and store — `mem_copy`'s promotable
    /// counterpart, since a `memcpy` is a non-promotable use of both slots.
    /// Bool lanes copy as bytes: a copy moves storage, so it must not round
    /// trip through the `<N x i1>` compute mask.
    pub(super) fn simd_copy_storage(
        &mut self,
        ctx: &mut Context,
        dest: Value,
        src: Value,
        dtype: Dtype,
        width: usize,
        anchor: Reg,
    ) {
        let storage_ty = self.simd_storage_ty(ctx, dtype, width);
        let align = self.simd_lane_layout(dtype).align as u32;
        let load = LoadOp::new(ctx, src, storage_ty);
        load.set_alignment(ctx, align);
        self.append(ctx, load.get_operation(), Some(anchor));
        let store = StoreOp::new(ctx, load.get_result(ctx), dest);
        store.set_alignment(ctx, align);
        self.append(ctx, store.get_operation(), Some(anchor));
    }

    /// `lane` broadcast to every lane: an insert into lane 0 of a poison
    /// vector and a zero-mask shuffle (LLVM folds constant splats).
    pub(super) fn simd_splat_value(
        &mut self,
        ctx: &mut Context,
        lane: Value,
        width: usize,
        dest: Reg,
    ) -> Value {
        let lane_ty = lane.get_type(ctx);
        let vector_ty: TypeHandle =
            VectorType::get(ctx, lane_ty, width as u32, VectorTypeKind::Fixed).into();
        let poison = PoisonOp::new(ctx, vector_ty);
        self.append(ctx, poison.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let insert = InsertElementOp::new(ctx, poison.get_result(ctx), lane, zero);
        self.append(ctx, insert.get_operation(), Some(dest));
        let seeded = insert.get_result(ctx);
        let shuffle = ShuffleVectorOp::new(ctx, seeded, seeded, vec![0; width]);
        self.append(ctx, shuffle.get_operation(), Some(dest));
        shuffle.get_result(ctx)
    }

    /// Lane `index` of a compute vector as a scalar.
    pub(super) fn simd_extract(
        &mut self,
        ctx: &mut Context,
        vector: Value,
        index: usize,
        dest: Reg,
    ) -> Value {
        let position = self.int_constant(ctx, index as i64);
        let extract = ExtractElementOp::new(ctx, vector, position);
        self.append(ctx, extract.get_operation(), Some(dest));
        extract.get_result(ctx)
    }

    /// An operand beside a multi-lane vector as a compute vector: a vector
    /// register's loaded value, or a scalar/literal converted once to the
    /// lane type and splatted (`runtime::to_int_lanes`).
    pub(super) fn simd_operand_vector(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if matches!(self.func.reg_types.get(&reg.0), Some(Ty::Simd { width, .. }) if *width > 1) {
            return self.simd_load_vector(ctx, reg, dtype, width, dest);
        }
        let lane_ty = ScalarTy::of_dtype(dtype);
        let lane = if let Some(literal) = self.pending_literals.get(&reg.0).cloned() {
            self.materialize_pending(ctx, &literal, lane_ty, dest)?
        } else {
            let source = self.concrete_scalar_ty(reg)?.ok_or_else(|| {
                self.unsupported_reg("unmaterialized SIMD splat operand".into(), dest)
            })?;
            let value = self.reg_value(ctx, reg, source)?;
            self.convert_lane(ctx, source, lane_ty, value, dest)?
        };
        Ok(self.simd_splat_value(ctx, lane, width, dest))
    }

    /// The byte-per-lane storage type of a multi-lane value.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "Op construction mutates the context"
    )]
    fn simd_storage_ty(&mut self, ctx: &Context, dtype: Dtype, width: usize) -> TypeHandle {
        let lane: TypeHandle = if dtype == Dtype::Bool {
            IntegerType::get(ctx, 8, Signedness::Signless).into()
        } else {
            ScalarTy::of_dtype(dtype).handle(ctx)
        };
        VectorType::get(ctx, lane, width as u32, VectorTypeKind::Fixed).into()
    }

    fn simd_lane_layout(&self, dtype: Dtype) -> Layout {
        self.layout
            .layout_of(&Ty::Simd { dtype, width: 1 })
            .expect("SIMD lane has a native layout")
    }

    /// `simd_cast_lane` over a whole vector: integer resizes and int↔float
    /// conversions are vector casts (`Float32` targets round through f64,
    /// as the VM does); float→int splits into a guarded vector conversion
    /// and an exact per-lane fallback (`simd_cast_float_to_int_vector`).
    fn simd_cast_vector(
        &mut self,
        ctx: &mut Context,
        source: Value,
        source_ty: ScalarTy,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let target = ScalarTy::of_dtype(dtype);
        Ok(match target {
            ScalarTy::Float64 => self.simd_lanes_to_f64(ctx, source, source_ty, width, dest)?,
            ScalarTy::Sized(Dtype::Float32) => {
                let wide = self.simd_lanes_to_f64(ctx, source, source_ty, width, dest)?;
                let f32_vec = self.simd_vector_ty(ctx, Dtype::Float32, width);
                let cast = FPTruncOp::new(ctx, wide, f32_vec);
                cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
                self.append(ctx, cast.get_operation(), Some(dest));
                cast.get_result(ctx)
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("bool targets are rejected above");
                if let Some(from) = source_ty.int_shape() {
                    self.simd_resize_int(ctx, source, from, to_bits, width, dest)
                } else {
                    self.simd_cast_float_to_int_vector(
                        ctx, source, source_ty, dtype, to_bits, width, dest,
                    )?
                }
            }
        })
    }

    /// Float lanes to integer lanes. The VM truncates toward zero,
    /// saturates at i128, and only then wraps to the target width, so the
    /// whole-vector form has to reproduce that i128 wrap.
    ///
    /// A guard splits the cases. When every lane satisfies `|x| < 2^(k-1)`
    /// the i128 saturation is unreachable and the wrap is a plain
    /// truncation, so one vector `fptosi` at `k` bits followed by
    /// `simd_resize_int` *is* the contract — an unsigned target needs no
    /// separate argument, because the wrap is bit-level. `k` is 32 for any
    /// narrower target, the only width x86 converts packed
    /// (`cvttps2dq`/`cvttpd2dq`), and 64 for a 64-bit target, where it
    /// covers the whole representable range. Every other vector — a lane
    /// at or past the threshold, an infinity, a NaN — takes the exact
    /// per-lane `llvm.fptosi.sat.i128.f64` loop.
    ///
    /// The vector saturating intrinsic is not the alternative: LLVM
    /// legalizes `llvm.fptosi.sat.v{N}i128.v{N}f64` into one `__fixdfti`
    /// libcall per lane, which is what the scalar form already emits.
    ///
    /// The fast path is deliberately not exhaustive: an unsigned target
    /// gets no `fptoui` variant of its own, and a narrow target gets no
    /// second tier for `[2^31, 2^63)`. Both land on the slow path, which is
    /// correct for every input.
    #[allow(clippy::too_many_arguments)]
    fn simd_cast_float_to_int_vector(
        &mut self,
        ctx: &mut Context,
        source: Value,
        source_ty: ScalarTy,
        dtype: Dtype,
        to_bits: u32,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let source_dtype = match source_ty {
            ScalarTy::Float64 => Dtype::Float64,
            ScalarTy::Sized(Dtype::Float32) => Dtype::Float32,
            other => {
                return Err(
                    self.unsupported_reg(format!("{} as a float SIMD element", other.name()), dest)
                );
            }
        };
        let region = self.region.expect("a SIMD cast is inside a function");
        let target_vec = self.simd_vector_ty(ctx, dtype, width);
        let (exact_bits, threshold, threshold_f32) = if to_bits <= 32 {
            (32u32, 2_147_483_648.0_f64, 2_147_483_648.0_f32)
        } else {
            (
                64u32,
                9_223_372_036_854_775_808.0_f64,
                9_223_372_036_854_775_808.0_f32,
            )
        };

        // The guard runs at the source's own float width, so an f32 source
        // never widens just to be narrowed again. Both thresholds are
        // powers of two well inside f32's exponent range, so the splat is
        // exact at either width. `UGE` is unordered, so a NaN lane joins
        // the out-of-range lanes instead of reaching the fast path.
        let magnitude = self.simd_fabs(ctx, source, source_dtype, width, dest);
        let limit_lane = if source_dtype == Dtype::Float32 {
            self.f32_constant(ctx, threshold_f32)
        } else {
            self.float_constant(ctx, threshold)
        };
        let limit = self.simd_splat_value(ctx, limit_lane, width, dest);
        let outside = self.fcmp(ctx, FCmpPredicateAttr::UGE, magnitude, limit);
        self.append(ctx, outside.get_operation(), Some(dest));
        let any_outside = self.simd_mask_any(ctx, outside.get_result(ctx), width, dest);

        let fast = BasicBlock::new(ctx, None, vec![]);
        fast.insert_at_back(region, ctx);
        let slow = BasicBlock::new(ctx, None, vec![]);
        slow.insert_at_back(region, ctx);
        let join = BasicBlock::new(ctx, None, vec![target_vec]);
        join.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, any_outside, slow, vec![], fast, vec![]);
        self.append(ctx, branch.get_operation(), Some(dest));

        self.current = Some(fast);
        let exact_lane: TypeHandle = IntegerType::get(ctx, exact_bits, Signedness::Signless).into();
        let exact_vec: TypeHandle =
            VectorType::get(ctx, exact_lane, width as u32, VectorTypeKind::Fixed).into();
        let exact = FPToSIOp::new(ctx, source, exact_vec);
        self.append(ctx, exact.get_operation(), Some(dest));
        let converted = self.simd_resize_int(
            ctx,
            exact.get_result(ctx),
            (exact_bits, true),
            to_bits,
            width,
            dest,
        );
        let jump = BrOp::new(ctx, join, vec![converted]);
        self.append(ctx, jump.get_operation(), Some(dest));

        self.current = Some(slow);
        let wide = self.simd_lanes_to_f64(ctx, source, source_ty, width, dest)?;
        let poison = PoisonOp::new(ctx, target_vec);
        self.append(ctx, poison.get_operation(), Some(dest));
        let mut out = poison.get_result(ctx);
        for lane in 0..width {
            let value = self.simd_extract(ctx, wide, lane, dest);
            let saturated = self.fptosi_sat_i128(ctx, value, dest);
            let narrowed = self.resize_int(ctx, saturated, (128, true), to_bits, dest);
            let position = self.int_constant(ctx, lane as i64);
            let insert = InsertElementOp::new(ctx, out, narrowed, position);
            self.append(ctx, insert.get_operation(), Some(dest));
            out = insert.get_result(ctx);
        }
        let jump = BrOp::new(ctx, join, vec![out]);
        self.append(ctx, jump.get_operation(), Some(dest));

        self.current = Some(join);
        Ok(join.deref(ctx).get_argument(0))
    }

    /// `llvm.fabs` over a whole float vector.
    fn simd_fabs(
        &mut self,
        ctx: &mut Context,
        value: Value,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Value {
        let vector_ty = self.simd_vector_ty(ctx, dtype, width);
        let fn_ty = FuncType::get(ctx, vector_ty, vec![vector_ty], false);
        let name = format!("llvm.fabs.{}", simd_mangle(dtype, width));
        let call = CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, vec![value]);
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// Whether any lane of an `<N x i1>` mask is set.
    fn simd_mask_any(&mut self, ctx: &mut Context, mask: Value, width: usize, dest: Reg) -> Value {
        let mask_ty = self.simd_vector_ty(ctx, Dtype::Bool, width);
        let bit: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let fn_ty = FuncType::get(ctx, bit, vec![mask_ty], false);
        let name = format!("llvm.vector.reduce.or.{}", simd_mangle(Dtype::Bool, width));
        let call = CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, vec![mask]);
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// `lane_to_f64` over a whole vector.
    fn simd_lanes_to_f64(
        &mut self,
        ctx: &mut Context,
        source: Value,
        source_ty: ScalarTy,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let f64_vec = self.simd_vector_ty(ctx, Dtype::Float64, width);
        match source_ty {
            ScalarTy::Float64 => Ok(source),
            ScalarTy::Sized(Dtype::Float32) => {
                let cast = FPExtOp::new(ctx, source, f64_vec);
                cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
                self.append(ctx, cast.get_operation(), Some(dest));
                Ok(cast.get_result(ctx))
            }
            other => match other.int_shape() {
                Some((_, true)) => {
                    let cast = SIToFPOp::new(ctx, source, f64_vec);
                    self.append(ctx, cast.get_operation(), Some(dest));
                    Ok(cast.get_result(ctx))
                }
                Some((_, false)) => {
                    let cast = UIToFPOp::new_with_nneg(ctx, source, f64_vec, false);
                    self.append(ctx, cast.get_operation(), Some(dest));
                    Ok(cast.get_result(ctx))
                }
                None => {
                    Err(self
                        .unsupported_reg(format!("{} as a float SIMD element", other.name()), dest))
                }
            },
        }
    }

    /// `resize_int` over a whole vector (the VM's `wrap` at the target
    /// width): truncate, or extend by the source's signedness.
    fn simd_resize_int(
        &mut self,
        ctx: &mut Context,
        value: Value,
        from: (u32, bool),
        to: u32,
        width: usize,
        dest: Reg,
    ) -> Value {
        let (from_bits, from_signed) = from;
        if from_bits == to {
            return value;
        }
        let lane: TypeHandle = IntegerType::get(ctx, to, Signedness::Signless).into();
        let to_ty: TypeHandle =
            VectorType::get(ctx, lane, width as u32, VectorTypeKind::Fixed).into();
        if to < from_bits {
            let cast = TruncOp::new(ctx, value, to_ty);
            self.append(ctx, cast.get_operation(), Some(dest));
            cast.get_result(ctx)
        } else if from_signed {
            let cast = SExtOp::new(ctx, value, to_ty);
            self.append(ctx, cast.get_operation(), Some(dest));
            cast.get_result(ctx)
        } else {
            let cast = ZExtOp::new_with_nneg(ctx, value, to_ty, false);
            self.append(ctx, cast.get_operation(), Some(dest));
            cast.get_result(ctx)
        }
    }

    /// Lanes' bit patterns as unsigned `dtype` lanes: a float lane bitcasts
    /// to its own width, an integer/bool lane is already its bits; a
    /// narrower source zero-extends. `shape` is `None` for one scalar lane.
    fn simd_bits_lanes(
        &mut self,
        ctx: &mut Context,
        source: Value,
        source_ty: ScalarTy,
        dtype: Dtype,
        shape: Option<usize>,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let (target_bits, _) = ScalarTy::of_dtype(dtype)
            .int_shape()
            .ok_or_else(|| self.unsupported_reg("SIMD to_bits target".into(), dest))?;
        let int_ty = |ctx: &mut Context, bits: u32| -> TypeHandle {
            let lane: TypeHandle = IntegerType::get(ctx, bits, Signedness::Signless).into();
            match shape {
                Some(width) => {
                    VectorType::get(ctx, lane, width as u32, VectorTypeKind::Fixed).into()
                }
                None => lane,
            }
        };
        let (raw, raw_bits) = match source_ty {
            ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => {
                let bits = if source_ty == ScalarTy::Float64 {
                    64
                } else {
                    32
                };
                let as_int = int_ty(ctx, bits);
                let cast = BitcastOp::new(ctx, source, as_int);
                self.append(ctx, cast.get_operation(), Some(dest));
                (cast.get_result(ctx), bits)
            }
            ScalarTy::Bool => (source, 1),
            ScalarTy::Ptr => {
                return Err(self.unsupported_reg("SIMD to_bits of a pointer lane".into(), dest));
            }
            integer => {
                let (bits, _) = integer.int_shape().expect("integer lane shape");
                (source, bits)
            }
        };
        if raw_bits == target_bits {
            return Ok(raw);
        }
        let target_ty = int_ty(ctx, target_bits);
        let widened = ZExtOp::new_with_nneg(ctx, raw, target_ty, false);
        self.append(ctx, widened.get_operation(), Some(dest));
        Ok(widened.get_result(ctx))
    }

    /// An elementwise comparison as an `<N x i1>` mask, by the lane kind's
    /// predicate table (`lower_compare`).
    fn simd_compare_vectors(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        lane_ty: ScalarTy,
        lhs: Value,
        rhs: Value,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        Ok(match lane_ty {
            ScalarTy::Bool => {
                if !matches!(op, InfixOp::Eq | InfixOp::Ne) {
                    return Err(
                        self.unsupported_reg(format!("SIMD bool-lane operator `{op:?}`"), dest)
                    );
                }
                let cmp = ICmpOp::new(ctx, unsigned_predicate(op), lhs, rhs);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
            ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => {
                let cmp = self.fcmp(ctx, float_predicate(op), lhs, rhs);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
            other => {
                let (_, signed) = other
                    .int_shape()
                    .ok_or_else(|| self.unsupported_reg("SIMD comparison lane".into(), dest))?;
                let predicate = if signed {
                    signed_predicate(op)
                } else {
                    unsigned_predicate(op)
                };
                let cmp = ICmpOp::new(ctx, predicate, lhs, rhs);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
        })
    }

    /// Float lanes: IEEE `+ - * /` at the lane width with no fast-math
    /// (`runtime::float_arith`; a `Float32` result computed at f32 equals
    /// the VM's f64 computation rounded once). A zero divisor flows through
    /// as inf/NaN lanes.
    fn simd_float_binop_vectors(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        lhs: Value,
        rhs: Value,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let flags = FastmathFlagsAttr::default;
        Ok(match op {
            InfixOp::Add => {
                let add = FAddOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, add.get_operation(), Some(dest));
                add.get_result(ctx)
            }
            InfixOp::Sub => {
                let sub = FSubOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, sub.get_operation(), Some(dest));
                sub.get_result(ctx)
            }
            InfixOp::Mul => {
                let mul = FMulOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, mul.get_operation(), Some(dest));
                mul.get_result(ctx)
            }
            InfixOp::Div => {
                let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, div.get_operation(), Some(dest));
                div.get_result(ctx)
            }
            other => {
                return Err(
                    self.unsupported_reg(format!("SIMD float-lane operator `{other:?}`"), dest)
                );
            }
        })
    }

    /// Bool lanes combine with `& | ^` over `<N x i1>` (`==`/`!=` split off
    /// as comparisons); the checker rejects anything else.
    fn simd_bool_binop_vectors(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        lhs: Value,
        rhs: Value,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        Ok(match op {
            InfixOp::BitAnd => {
                let and = AndOp::new(ctx, lhs, rhs);
                self.append(ctx, and.get_operation(), Some(dest));
                and.get_result(ctx)
            }
            InfixOp::BitOr => {
                let or = OrOp::new(ctx, lhs, rhs);
                self.append(ctx, or.get_operation(), Some(dest));
                or.get_result(ctx)
            }
            InfixOp::BitXor => {
                let xor = XorOp::new(ctx, lhs, rhs);
                self.append(ctx, xor.get_operation(), Some(dest));
                xor.get_result(ctx)
            }
            other => {
                return Err(
                    self.unsupported_reg(format!("SIMD bool-lane operator `{other:?}`"), dest)
                );
            }
        })
    }

    /// `llvm.minnum`/`llvm.maxnum` on two lanes — Rust's `f64::min`/`max`
    /// (the VM's float `reduce_min`/`reduce_max` step): a NaN operand
    /// yields the other operand, so a NaN lane never poisons the fold.
    fn float_min_max(
        &mut self,
        ctx: &mut Context,
        lane_ty: ScalarTy,
        is_min: bool,
        a: Value,
        b: Value,
        dest: Reg,
    ) -> Value {
        let (suffix, handle) = match lane_ty {
            ScalarTy::Sized(Dtype::Float32) => ("f32", FP32Type::get(ctx).into()),
            _ => ("f64", FP64Type::get(ctx).into()),
        };
        let handle: TypeHandle = handle;
        let fn_ty = FuncType::get(ctx, handle, vec![handle, handle], false);
        let name = format!("llvm.{}.{suffix}", if is_min { "minnum" } else { "maxnum" });
        let call = CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, vec![a, b]);
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }
}

/// LLVM's overloaded-intrinsic suffix for a `<width x lane>` vector
/// (`v4i32`, `v8f32`, `v16i1`).
fn simd_mangle(dtype: Dtype, width: usize) -> String {
    let lane = match dtype {
        Dtype::Float32 => "f32".to_string(),
        Dtype::Float64 => "f64".to_string(),
        Dtype::Bool => "i1".to_string(),
        integer => {
            let (bits, _) =
                mojito_vm::runtime::integer_dtype_bits(integer).expect("integer SIMD dtype");
            format!("i{bits}")
        }
    };
    format!("v{width}{lane}")
}
