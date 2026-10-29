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
use mojito_types::llvm_intrinsic::{LlvmIntrinsic, OverloadSlot};
use mojito_types::mlir::PopOp;
use mojito_types::types::TyKind;
use pliron_llvm::ops::ExtractValueOp;
use pliron_llvm::types::{StructLayout, StructType};

impl FnLowering<'_> {
    /// Construct a SIMD value with the VM's per-lane conversions. Width-one
    /// aliases remain SSA scalars; wider values assemble a vector lane by
    /// lane (one element splats).
    /// The known lane dtype and width of a SIMD instruction. Concrete MIR
    /// holds known slots; a symbolic one is a generator form the elaborator
    /// closes before lowering, so meeting it here is the unsupported boundary.
    pub(super) fn concrete_simd_slots(
        &self,
        dest: Reg,
        dtype: &mojito_types::types::SimdDtype,
        width: &mojito_types::types::SimdWidth,
    ) -> Result<(Dtype, usize), PlironError> {
        match (
            dtype.known(),
            width.known().and_then(|width| usize::try_from(width).ok()),
        ) {
            (Some(dtype), Some(width)) => Ok((dtype, width)),
            _ => Err(self.unsupported_reg(
                format!("SIMD instruction over the symbolic slots `SIMD[{dtype}, {width}]`"),
                dest,
            )),
        }
    }

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
        // at the lane width, narrow floats round from the exact rational).
        if let Some(literal) = self.pending_literals.get(&elem.0).cloned() {
            // Upstream's `IntLiteral` initializer: a mask lane is `True`
            // when the literal is nonzero.
            if let (PendingLiteral::Int(literal), ScalarTy::Bool) = (&literal, target) {
                return Ok(self.bool_constant(ctx, !literal.is_zero()));
            }
            return self.materialize_pending(ctx, &literal, target, dest);
        }
        if let Some(value) = self.intable_struct_value(ctx, elem, dest)? {
            return self.convert_lane(ctx, ScalarTy::Int, target, value, dest);
        }
        let source = match self.concrete_scalar_ty(elem)? {
            Some(ty) => ty,
            None => match self.func.reg_types.get(&elem.0).map(Ty::kind) {
                Some(TyKind::FloatLiteral) => ScalarTy::Float64,
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
        let register_ty = self.func.reg_types.get(&source.0).cloned();
        let Some(TyKind::Struct(name, _)) = register_ty.as_ref().map(Ty::kind) else {
            return Ok(None);
        };
        let Some((method, signature)) = self
            .struct_member(name, "__int__", 0)
            .and_then(|method| Some((method, self.signatures.get(method)?)))
        else {
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

    /// `SimdCast` (upstream's `pop.cast`) — the VM's
    /// `runtime::simd_cast`: int→int rewraps at the new width, int→float
    /// converts through f64 (narrow lanes round), float→float widens or
    /// rounds, and float→int truncates toward zero saturating at the
    /// 128-bit intermediate before wrapping — saturation must happen at
    /// i128, not the target width, or large magnitudes wrap differently
    /// than the VM. A cast to or from `bool` lanes is [`Self::bool_lane_cast`].
    pub(super) fn lower_simd_cast(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        if width > 1 {
            let Some((source_dtype, source_width)) =
                self.func.reg_types.get(&value.0).and_then(simd_dims)
            else {
                return Err(self.unsupported_reg("SIMD cast source type".into(), dest));
            };
            if source_width != width as i64 {
                return Err(self.unsupported_reg("SIMD cast width mismatch".into(), dest));
            }
            let source_ty = ScalarTy::of_dtype(source_dtype);
            if source_ty == ScalarTy::Ptr {
                return Err(self.unsupported_reg("SIMD cast of a pointer operand".into(), dest));
            }
            let source = self.simd_load_vector(ctx, value, source_dtype, width, dest)?;
            let converted = if dtype == Dtype::Bool || source_ty == ScalarTy::Bool {
                self.bool_lane_cast(ctx, source, source_ty, dtype, Some(width), dest)?
            } else {
                self.simd_cast_vector(ctx, source, source_ty, dtype, width, dest)?
            };
            self.simd_store_vector(ctx, dest, dtype, width, converted);
            return Ok(());
        }
        let source = self.concrete_scalar_ty(value)?.ok_or_else(|| {
            self.unsupported_reg("SIMD cast of an unmaterialized literal".into(), dest)
        })?;
        if source == ScalarTy::Ptr {
            return Err(
                self.unsupported_reg(format!("SIMD cast of a {} operand", source.name()), dest)
            );
        }
        let lane = self.reg_value(ctx, value, source)?;
        let converted = if dtype == Dtype::Bool || source == ScalarTy::Bool {
            self.bool_lane_cast(ctx, lane, source, dtype, None, dest)?
        } else {
            self.simd_cast_lane(ctx, lane, source, dtype, dest)?
        };
        self.reg_values.insert(dest.0, converted);
        Ok(())
    }

    /// `SimdBitcast` (upstream's `pop.bitcast`) — the VM's
    /// `runtime::simd_bitcast`: one LLVM `bitcast` between the compute
    /// types of the same total width, a multi-lane `bool` value an
    /// `<N x i1>`, so lanes pack little-endian with a `bool` lane one bit.
    pub(super) fn lower_simd_bitcast(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        let Some((source_dtype, source_width)) =
            self.func.reg_types.get(&value.0).and_then(simd_dims)
        else {
            return Err(self.unsupported_reg("SIMD bitcast source type".into(), dest));
        };
        if ScalarTy::of_dtype(source_dtype) == ScalarTy::Ptr
            || ScalarTy::of_dtype(dtype) == ScalarTy::Ptr
        {
            return Err(self.unsupported_reg("SIMD bitcast of a pointer lane".into(), dest));
        }
        let source = if source_width > 1 {
            self.simd_load_vector(ctx, value, source_dtype, source_width as usize, dest)?
        } else {
            let source = self.concrete_scalar_ty(value)?.ok_or_else(|| {
                self.unsupported_reg("SIMD bitcast of an unmaterialized literal".into(), dest)
            })?;
            self.reg_value(ctx, value, source)?
        };
        let target_ty = if width > 1 {
            self.simd_vector_ty(ctx, dtype, width)
        } else {
            ScalarTy::of_dtype(dtype).handle(ctx)
        };
        let cast = BitcastOp::new(ctx, source, target_ty);
        self.append(ctx, cast.get_operation(), Some(dest));
        let bits = cast.get_result(ctx);
        if width > 1 {
            self.simd_store_vector(ctx, dest, dtype, width, bits);
        } else {
            self.reg_values.insert(dest.0, bits);
        }
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
            ScalarTy::Sized(narrow) if narrow.is_narrow_float() => {
                let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                self.round_float_lane(ctx, narrow, wide, dest)
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("a bool target is `bool_lane_cast`");
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

    /// A compile-time lane gather over one vector, or over a joined pair
    /// (`other`, whose lanes the mask indexes past `value`'s, as
    /// `shufflevector`'s second operand): a one-lane mask extracts the
    /// scalar, a wider mask is one `shufflevector` (the checker bounds every
    /// index).
    pub(super) fn lower_simd_shuffle(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        other: Option<Reg>,
        mask: &[usize],
    ) -> Result<(), PlironError> {
        let Some((dtype, width)) = self.func.reg_types.get(&value.0).and_then(simd_dims) else {
            return Err(self.unsupported_reg("SIMD shuffle source type".into(), dest));
        };
        let operands = if other.is_some() { 2 } else { 1 };
        if mask.iter().any(|index| *index >= operands * width as usize) {
            return Err(self.unsupported_reg("SIMD shuffle index out of range".into(), dest));
        }
        // A width-1 operand of a join is a scalar register, which the operand
        // path lifts to a one-lane vector.
        let source = self.simd_operand_vector(ctx, value, dtype, width as usize, dest)?;
        let second = match other {
            Some(other) => self.simd_operand_vector(ctx, other, dtype, width as usize, dest)?,
            None => source,
        };
        if mask.len() == 1 && other.is_none() {
            let position = self.int_constant(ctx, mask[0] as i64);
            let extract = ExtractElementOp::new(ctx, source, position);
            return self.define(ctx, dest, extract.get_operation(), extract.get_result(ctx));
        }
        let shuffle = ShuffleVectorOp::new(
            ctx,
            source,
            second,
            mask.iter().map(|index| *index as i32).collect(),
        );
        self.append(ctx, shuffle.get_operation(), Some(dest));
        self.simd_store_vector(ctx, dest, dtype, mask.len(), shuffle.get_result(ctx));
        Ok(())
    }

    /// One scalar value as a `target` SIMD lane — the VM's lane builders:
    /// integer lanes wrap the source's mathematical value at the lane width
    /// (`value_to_int_lane`; Bool reads as 0/1), float lanes convert through
    /// f64 with narrow-lane rounding (`value_to_float_lane`), bool lanes only
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
            ScalarTy::Sized(narrow) if narrow.is_narrow_float() => {
                let wide = self.lane_to_f64(ctx, source, value, dest)?;
                Ok(self.round_float_lane(ctx, narrow, wide, dest))
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
    /// `value_to_float`): integers convert by signedness, a narrow float widens
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
            ScalarTy::Sized(dtype) if dtype.is_narrow_float() => {
                Ok(self.widen_float_lane(ctx, value, dest))
            }
            ScalarTy::Int => Ok(self.int_to_f64(ctx, value, dest)),
            ScalarTy::UInt => Ok(self.uint_to_f64(ctx, value, dest)),
            ScalarTy::Sized(dtype) => {
                let (_, signed) = mojito_vm::runtime::integer_dtype_bits(dtype)
                    .expect("narrow floats are matched above");
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
            "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max" if args.is_empty() => {
                self.lower_simd_reduce(ctx, dest, recv, dtype, width, method)
            }
            _ => Err(self.unsupported_reg(format!("SIMD method `{method}`"), dest)),
        }
    }

    /// The six reductions (`runtime::simd_reduce`, and `simd.reduce`'s
    /// `runtime::simd_reduce_lanes`): a strict left fold in the VM. Integer
    /// add/mul wrap per step, so the wrapping vector reductions are exact;
    /// min/max select the signed or unsigned family; `and`/`or` are the
    /// bitwise reductions over integer or `<N x i1>` lanes; float add/mul
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
            // Half-precision lanes fold lane by lane at f64, rounding each
            // step as the VM does: LLVM computes `half` arithmetic through
            // `float`, which double-rounds a product.
            "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max"
                if dtype == Dtype::Float16 =>
            {
                let mut accumulator = self.simd_extract(ctx, vector, 0, dest);
                for lane in 1..width {
                    let next = self.simd_extract(ctx, vector, lane, dest);
                    let wide_accumulator = self.widen_float_lane(ctx, accumulator, dest);
                    let wide_next = self.widen_float_lane(ctx, next, dest);
                    let wide = match method {
                        "reduce_add" => self.simd_float_binop_vectors(
                            ctx,
                            InfixOp::Add,
                            wide_accumulator,
                            wide_next,
                            dest,
                        )?,
                        "reduce_mul" => self.simd_float_binop_vectors(
                            ctx,
                            InfixOp::Mul,
                            wide_accumulator,
                            wide_next,
                            dest,
                        )?,
                        _ => self.float_min_max(
                            ctx,
                            ScalarTy::Float64,
                            method == "reduce_min",
                            wide_accumulator,
                            wide_next,
                            dest,
                        ),
                    };
                    accumulator = self.round_float_lane(ctx, dtype, wide, dest);
                }
                accumulator
            }
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
                    ("reduce_and", _) => "and",
                    ("reduce_or", _) => "or",
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

    /// `simd.select` — one `select` over the `<N x i1>` mask, or the `i1`
    /// of a width-one mask (`runtime::simd_select`).
    pub(super) fn lower_simd_select(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        mask: Reg,
        yes: Reg,
        no: Reg,
    ) -> Result<(), PlironError> {
        let Some((dtype, width)) = self.func.reg_types.get(&dest.0).and_then(simd_dims) else {
            return Err(self.unsupported_reg("SIMD select result type".into(), dest));
        };
        let width = width as usize;
        if width == 1 {
            let lane_ty = ScalarTy::of_dtype(dtype);
            let condition = self.reg_value(ctx, mask, ScalarTy::Bool)?;
            let yes_value = self.reg_value(ctx, yes, lane_ty)?;
            let no_value = self.reg_value(ctx, no, lane_ty)?;
            let select = SelectOp::new(ctx, condition, yes_value, no_value);
            return self.define(ctx, dest, select.get_operation(), select.get_result(ctx));
        }
        let condition = self.simd_load_vector(ctx, mask, Dtype::Bool, width, dest)?;
        let yes_value = self.simd_operand_vector(ctx, yes, dtype, width, dest)?;
        let no_value = self.simd_operand_vector(ctx, no, dtype, width, dest)?;
        let select = SelectOp::new(ctx, condition, yes_value, no_value);
        self.append(ctx, select.get_operation(), Some(dest));
        self.simd_store_vector(ctx, dest, dtype, width, select.get_result(ctx));
        Ok(())
    }

    /// `simd.extract` — lane `index` of a vector, its index guarded as a
    /// lane read is (`runtime::read_value_lane`).
    pub(super) fn lower_simd_extract(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        index: Reg,
    ) -> Result<(), PlironError> {
        let Some((dtype, width)) = self.func.reg_types.get(&value.0).and_then(simd_dims) else {
            return Err(self.unsupported_reg("SIMD lane read operand type".into(), dest));
        };
        self.emit_index_guard(ctx, index, width as usize, dest)?;
        // A width-1 vector is a scalar register (`LowerTy::Scalar`): its
        // only lane is the value itself.
        if width == 1 {
            let lane = self.reg_value(ctx, value, ScalarTy::of_dtype(dtype))?;
            self.reg_values.insert(dest.0, lane);
            return Ok(());
        }
        // The guard above makes the dynamic `extractelement` index in range
        // (an out-of-range LLVM index is poison).
        let vector = self.simd_load_vector(ctx, value, dtype, width as usize, dest)?;
        let position = self.reg_value(ctx, index, ScalarTy::Int)?;
        let extract = ExtractElementOp::new(ctx, vector, position);
        self.define(ctx, dest, extract.get_operation(), extract.get_result(ctx))
    }

    /// `simd.insert` — a vector with lane `index` replaced, its index
    /// guarded as [`Self::lower_simd_extract`]'s is
    /// (`runtime::set_value_lane`).
    pub(super) fn lower_simd_insert(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        element: Reg,
        index: Reg,
    ) -> Result<(), PlironError> {
        let Some((dtype, width)) = self.func.reg_types.get(&value.0).and_then(simd_dims) else {
            return Err(self.unsupported_reg("SIMD lane write operand type".into(), dest));
        };
        let width = width as usize;
        self.emit_index_guard(ctx, index, width, dest)?;
        let lane = self.reg_value(ctx, element, ScalarTy::of_dtype(dtype))?;
        if width == 1 {
            self.reg_values.insert(dest.0, lane);
            return Ok(());
        }
        let vector = self.simd_load_vector(ctx, value, dtype, width, dest)?;
        let position = self.reg_value(ctx, index, ScalarTy::Int)?;
        let insert = InsertElementOp::new(ctx, vector, lane, position);
        self.append(ctx, insert.get_operation(), Some(dest));
        self.simd_store_vector(ctx, dest, dtype, width, insert.get_result(ctx));
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
            (
                PrefixOp::Neg,
                ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float16 | Dtype::Float32),
            ) => {
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
            // `pop.floordiv`: the floor of the lane-rounded quotient.
            _ if op == InfixOp::FloorDiv && dtype.is_float() => {
                let quotient = self.lower_simd_float_quotient(ctx, lhs, rhs, dtype, width, dest)?;
                let floor = format!("llvm.floor.{}", simd_mangle(dtype, width));
                let vector_ty = quotient.get_type(ctx);
                self.float_intrinsic(ctx, &floor, vector_ty, vec![quotient], dest)
            }
            // Half-precision lanes compute at f64 and round once, like the
            // scalar lowering (`lower_narrow_float_binop`).
            ScalarTy::Sized(Dtype::Float16) => {
                let wide_lhs = self.simd_lanes_to_f64(ctx, lhs, lane_ty, width, dest)?;
                let wide_rhs = self.simd_lanes_to_f64(ctx, rhs, lane_ty, width, dest)?;
                let wide = self.simd_float_binop_vectors(ctx, op, wide_lhs, wide_rhs, dest)?;
                self.simd_round_float_vector(ctx, dtype, wide, width, dest)
            }
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
            ScalarTy::Ptr | ScalarTy::Dtype => {
                return Err(self.unsupported_reg(format!("SIMD binary operator `{op:?}`"), dest));
            }
        };
        self.simd_store_vector(ctx, dest, dtype, width, result);
        Ok(())
    }

    /// The lane-rounded quotient of two float vectors: `fdiv` at the lane
    /// type, or at f64 and rounded once for `Float16` lanes.
    fn lower_simd_float_quotient(
        &mut self,
        ctx: &mut Context,
        lhs: Value,
        rhs: Value,
        dtype: Dtype,
        width: usize,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if dtype == Dtype::Float16 {
            let lane_ty = ScalarTy::Sized(dtype);
            let wide_lhs = self.simd_lanes_to_f64(ctx, lhs, lane_ty, width, dest)?;
            let wide_rhs = self.simd_lanes_to_f64(ctx, rhs, lane_ty, width, dest)?;
            let wide =
                self.simd_float_binop_vectors(ctx, InfixOp::Div, wide_lhs, wide_rhs, dest)?;
            return Ok(self.simd_round_float_vector(ctx, dtype, wide, width, dest));
        }
        self.simd_float_binop_vectors(ctx, InfixOp::Div, lhs, rhs, dest)
    }

    /// Upstream's `pop` lane op over operands of `dest`'s SIMD type, each
    /// lane as `mlir::pop_int_lane`/`pop_float_lane` computes it: a vector
    /// computes `<N x lane>` at once, a width-one value its scalar lane.
    pub(super) fn lower_pop(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        op: PopOp,
        operands: &[Reg],
    ) -> Result<(), PlironError> {
        if op.is_compare() {
            return self.lower_unsigned_compare(ctx, dest, op, operands);
        }
        // `index.divs` divides `index` values, held as `Int` lanes.
        let Some((dtype, width)) = self.func.reg_types.get(&dest.0).and_then(|ty| {
            simd_dims(ty).or_else(|| (*ty == Ty::index()).then_some((Dtype::Int, 1)))
        }) else {
            return Err(self.unsupported_reg("`pop` over a non-SIMD register".to_string(), dest));
        };
        let shape = usize::try_from(width).ok().filter(|width| *width > 1);
        let mut values = Vec::with_capacity(operands.len());
        for operand in operands {
            values.push(match shape {
                Some(width) => self.simd_load_vector(ctx, *operand, dtype, width, dest)?,
                None => self.reg_value(ctx, *operand, ScalarTy::of_dtype(dtype))?,
            });
        }
        let value = if dtype.is_float() {
            self.pop_float_value(ctx, op, dtype, shape, values, dest)?
        } else if dtype == Dtype::Bool {
            self.pop_bool_value(ctx, op, &values, dest)?
        } else {
            self.pop_int_value(ctx, op, dtype, shape, &values, dest)?
        };
        match shape {
            Some(width) => self.simd_store_vector(ctx, dest, dtype, width, value),
            None => {
                self.reg_values.insert(dest.0, value);
            }
        }
        Ok(())
    }

    /// `llvm.intrinsic` — a call of the LLVM intrinsic itself, its name
    /// mangled by the types [`LlvmIntrinsic::overloads`] names: a
    /// multi-lane operand or result is a vector, a width-one one the lane.
    /// The `ctlz`/`cttz` flag is the `immarg` constant `false`, whose
    /// defined result for a zero lane the VM computes; a shuffle's index
    /// ([`LlvmIntrinsic::index_operand`]) is its register's value, which
    /// LLVM's verifier rejects as an `immarg` unless it is a constant.
    /// `llvm.donothing` returns nothing. A result LLVM translation sees as a
    /// literal struct (`MirDeclarations::translated_type`, a
    /// `_RegisterPackType`'s) is LLVM's literal struct of its elements'
    /// vectors, each extracted into the destination's storage at its
    /// offset.
    pub(super) fn lower_llvm_intrinsic(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        intrinsic: LlvmIntrinsic,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        if intrinsic == LlvmIntrinsic::Donothing {
            let void = VoidType::get(ctx).to_handle();
            let fn_ty = FuncType::get(ctx, void, Vec::new(), false);
            let call = CallIntrinsicOp::new(
                ctx,
                StringAttr::new(intrinsic.name().to_string()),
                fn_ty,
                Vec::new(),
            );
            self.append(ctx, call.get_operation(), Some(dest));
            return Ok(());
        }
        let shaped = |(dtype, width): (Dtype, i64)| {
            (
                dtype,
                usize::try_from(width).ok().filter(|width| *width > 1),
            )
        };
        let dims = |reg: &Reg| {
            self.func
                .reg_types
                .get(&reg.0)
                .and_then(simd_dims)
                .map(shaped)
        };
        let non_simd = |lowering: &Self| {
            lowering.unsupported_reg(
                format!("`{}` over non-SIMD registers", intrinsic.name()),
                dest,
            )
        };
        let elements = match self
            .func
            .reg_types
            .get(&dest.0)
            .map(|ty| self.mir_declarations.translated_type(ty))
            .as_ref()
            .map(Ty::kind)
        {
            Some(TyKind::Tuple(elements)) => Some(elements.clone()),
            _ => None,
        };
        let result = match &elements {
            Some(elements) => elements.first().and_then(simd_dims).map(shaped),
            None => dims(&dest),
        };
        let Some(result) = result else {
            return Err(non_simd(self));
        };
        let mut operand_shapes = Vec::with_capacity(args.len());
        let mut values = Vec::with_capacity(args.len());
        let mut types = Vec::with_capacity(args.len());
        for (position, arg) in args.iter().enumerate() {
            if intrinsic.is_flag(position) {
                values.push(self.bool_constant(ctx, false));
                types.push(IntegerType::get(ctx, 1, Signedness::Signless).into());
                operand_shapes.push(None);
                continue;
            }
            let Some((dtype, shape)) = dims(arg) else {
                return Err(non_simd(self));
            };
            values.push(match shape {
                Some(width) => self.simd_load_vector(ctx, *arg, dtype, width, dest)?,
                None => self.reg_value(ctx, *arg, ScalarTy::of_dtype(dtype))?,
            });
            types.push(self.llvm_intrinsic_ty(ctx, dtype, shape));
            operand_shapes.push(Some((dtype, shape)));
        }
        let result_ty = match &elements {
            Some(elements) => {
                let fields = vec![self.llvm_intrinsic_ty(ctx, result.0, result.1); elements.len()];
                StructType::get_unnamed(ctx, (fields, StructLayout::Unpacked)).into()
            }
            None => self.llvm_intrinsic_ty(ctx, result.0, result.1),
        };
        let suffix = |(dtype, shape): (Dtype, Option<usize>)| match shape {
            Some(width) => simd_mangle(dtype, width),
            None => lane_suffix(dtype),
        };
        let mut name = intrinsic.name().to_string();
        for slot in intrinsic.overloads() {
            let shape = match slot {
                OverloadSlot::Result => Some(result),
                OverloadSlot::Operand(position) => operand_shapes.get(*position).copied().flatten(),
            };
            let Some(shape) = shape else {
                return Err(self.unsupported_reg(
                    format!("`{}` overloaded on a flag operand", intrinsic.name()),
                    dest,
                ));
            };
            name.push('.');
            name.push_str(&suffix(shape));
        }
        let fn_ty = FuncType::get(ctx, result_ty, types, false);
        let call = CallIntrinsicOp::new(ctx, StringAttr::new(name), fn_ty, values);
        self.append(ctx, call.get_operation(), Some(dest));
        let value = call.get_result(ctx);
        if let Some(elements) = elements {
            return self.store_struct_result(ctx, dest, &elements, value);
        }
        match result.1 {
            Some(width) => self.simd_store_vector(ctx, dest, result.0, width, value),
            None => {
                self.reg_values.insert(dest.0, value);
            }
        }
        Ok(())
    }

    /// Store each vector of an intrinsic's literal-struct result `value`
    /// into fresh storage laid out as `elements`, and define `dest` as it.
    fn store_struct_result(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        elements: &[Ty],
        value: Value,
    ) -> Result<(), PlironError> {
        let composed = self.struct_layout_of(elements, dest)?;
        let storage = self.entry_alloca(
            ctx,
            composed.layout.size.max(1),
            composed.layout.align.max(1),
        );
        for (index, (element, offset)) in elements.iter().zip(&composed.offsets).enumerate() {
            let Some((dtype, width)) = simd_dims(element)
                .and_then(|(dtype, width)| Some((dtype, usize::try_from(width).ok()?)))
            else {
                return Err(self.unsupported_reg(
                    format!("an LLVM intrinsic struct element of type `{element}`"),
                    dest,
                ));
            };
            let extract = ExtractValueOp::new(ctx, value, vec![index as u32]).map_err(|error| {
                self.unsupported_reg(format!("LLVM intrinsic struct result ({error})"), dest)
            })?;
            self.append(ctx, extract.get_operation(), Some(dest));
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            let element = extract.get_result(ctx);
            self.simd_store_vector_to(ctx, address, dtype, width, element, dest);
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// The LLVM type of an intrinsic's operand or result: a vector of
    /// `width` lanes, or the lane.
    fn llvm_intrinsic_ty(
        &mut self,
        ctx: &Context,
        dtype: Dtype,
        width: Option<usize>,
    ) -> TypeHandle {
        match width {
            Some(width) => self.simd_vector_ty(ctx, dtype, width),
            None => ScalarTy::of_dtype(dtype).handle(ctx),
        }
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
        if matches!(self.func.reg_types.get(&reg.0).and_then(simd_dims), Some((_, width)) if width > 1)
        {
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
            .layout_of(&canonical_simd_ty(dtype, 1))
            .expect("SIMD lane has a native layout")
    }

    /// A `pop.cast` to or from `bool` lanes over a lane (`shape` `None`)
    /// or a vector of `shape` lanes, as the VM's `runtime::simd_cast`
    /// computes it: a `bool` lane widens to 0 or 1 (`zext` to a byte, then
    /// the byte's cast), and a lane cast to `bool` is its low bit (`trunc`
    /// to `i1`), a float lane first cast to an `int64` lane as the integer
    /// cast does.
    fn bool_lane_cast(
        &mut self,
        ctx: &mut Context,
        value: Value,
        source: ScalarTy,
        dtype: Dtype,
        shape: Option<usize>,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if source == ScalarTy::Bool && dtype == Dtype::Bool {
            return Ok(value);
        }
        if source == ScalarTy::Bool {
            let byte = self.resize_int_lanes(ctx, value, (1, false), 8, shape, dest);
            let byte_ty = ScalarTy::of_dtype(Dtype::UInt8);
            return match shape {
                Some(width) => self.simd_cast_vector(ctx, byte, byte_ty, dtype, width, dest),
                None => self.simd_cast_lane(ctx, byte, byte_ty, dtype, dest),
            };
        }
        if let Some(from) = source.int_shape() {
            return Ok(self.resize_int_lanes(ctx, value, from, 1, shape, dest));
        }
        let int = match shape {
            Some(width) => self.simd_cast_vector(ctx, value, source, Dtype::Int64, width, dest)?,
            None => self.simd_cast_lane(ctx, value, source, Dtype::Int64, dest)?,
        };
        Ok(self.resize_int_lanes(ctx, int, (64, true), 1, shape, dest))
    }

    /// [`Self::resize_int`] over a lane (`shape` `None`) or a vector of
    /// `shape` lanes.
    fn resize_int_lanes(
        &mut self,
        ctx: &mut Context,
        value: Value,
        from: (u32, bool),
        to: u32,
        shape: Option<usize>,
        dest: Reg,
    ) -> Value {
        match shape {
            Some(width) => self.simd_resize_int(ctx, value, from, to, width, dest),
            None => self.resize_int(ctx, value, from, to, dest),
        }
    }

    /// `simd_cast_lane` over a whole vector: integer resizes and int↔float
    /// conversions are vector casts (narrow float targets round through f64,
    /// as the VM does); float→int splits into a guarded vector conversion
    /// and an exact per-lane fallback (`simd_cast_float_to_int_vector`), a
    /// `Float16` source widening to f64 first.
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
            ScalarTy::Sized(narrow) if narrow.is_narrow_float() => {
                let wide = self.simd_lanes_to_f64(ctx, source, source_ty, width, dest)?;
                self.simd_round_float_vector(ctx, narrow, wide, width, dest)
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("a bool target is `bool_lane_cast`");
                if let Some(from) = source_ty.int_shape() {
                    self.simd_resize_int(ctx, source, from, to_bits, width, dest)
                } else if source_ty == ScalarTy::Sized(Dtype::Float16) {
                    let wide = self.simd_lanes_to_f64(ctx, source, source_ty, width, dest)?;
                    self.simd_cast_float_to_int_vector(
                        ctx,
                        wide,
                        ScalarTy::Float64,
                        dtype,
                        to_bits,
                        width,
                        dest,
                    )?
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

    /// A float `pop` op at the lane type; `Float16` lanes compute at f64
    /// and round once, as the VM does.
    fn pop_float_value(
        &mut self,
        ctx: &mut Context,
        op: PopOp,
        dtype: Dtype,
        shape: Option<usize>,
        values: Vec<Value>,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if dtype == Dtype::Float16 {
            let mut wide = Vec::with_capacity(values.len());
            for value in values {
                wide.push(match shape {
                    Some(width) => {
                        self.simd_lanes_to_f64(ctx, value, ScalarTy::Sized(dtype), width, dest)?
                    }
                    None => self.widen_float_lane(ctx, value, dest),
                });
            }
            let result = self.pop_float_value(ctx, op, Dtype::Float64, shape, wide, dest)?;
            return Ok(match shape {
                Some(width) => self.simd_round_float_vector(ctx, dtype, result, width, dest),
                None => self.round_float_lane(ctx, dtype, result, dest),
            });
        }
        let flags = FastmathFlagsAttr::default;
        Ok(match (op, values.as_slice()) {
            (PopOp::Div, [lhs, rhs]) => {
                let div = FDivOp::new_with_fast_math_flags(ctx, *lhs, *rhs, flags());
                self.append(ctx, div.get_operation(), Some(dest));
                div.get_result(ctx)
            }
            (PopOp::Rem, [lhs, rhs]) => {
                let rem = FRemOp::new_with_fast_math_flags(ctx, *lhs, *rhs, flags());
                self.append(ctx, rem.get_operation(), Some(dest));
                rem.get_result(ctx)
            }
            _ => {
                let intrinsic = match op {
                    PopOp::Abs => "fabs",
                    PopOp::Round => "roundeven",
                    PopOp::Floor => "floor",
                    PopOp::Ceil => "ceil",
                    PopOp::Trunc => "trunc",
                    PopOp::Fma => "fma",
                    PopOp::Max => "maxnum",
                    PopOp::Min => "minnum",
                    PopOp::Div | PopOp::Rem | PopOp::Ult | PopOp::Ule | PopOp::Ugt | PopOp::Uge => {
                        return Err(self.unsupported_reg(
                            format!("`pop.{}` operand count", op.mnemonic()),
                            dest,
                        ));
                    }
                };
                let name = format!("llvm.{intrinsic}.{}", lane_mangle(dtype, shape));
                let float_ty = values[0].get_type(ctx);
                self.float_intrinsic(ctx, &name, float_ty, values, dest)
            }
        })
    }

    /// An integer `pop` op: `div`/`rem` truncate (`sdiv`/`udiv`,
    /// `srem`/`urem`) after trapping on a zero divisor lane, with the
    /// overflowing `MIN / -1` lane divided by one so it wraps as the VM's
    /// does; `abs` wraps (`llvm.abs` with `is_int_min_poison` false) and is
    /// the identity on unsigned lanes, the rounding ops are the identity,
    /// `fma` wraps `a * b + c`, and `max`/`min` select by the lanes'
    /// signed or unsigned order.
    fn pop_int_value(
        &mut self,
        ctx: &mut Context,
        op: PopOp,
        dtype: Dtype,
        shape: Option<usize>,
        values: &[Value],
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let Some((bits, signed)) = mojito_vm::runtime::integer_dtype_bits(dtype) else {
            return Err(
                self.unsupported_reg(format!("`pop.{}` over bool lanes", op.mnemonic()), dest)
            );
        };
        Ok(match (op, values) {
            (PopOp::Div | PopOp::Rem, [lhs, rhs]) => {
                let zero = self.lane_constant(ctx, dtype, 0, shape, dest);
                let is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, *rhs, zero);
                self.append(ctx, is_zero.get_operation(), Some(dest));
                let any_zero = match shape {
                    Some(width) => self.simd_mask_any(ctx, is_zero.get_result(ctx), width, dest),
                    None => is_zero.get_result(ctx),
                };
                self.emit_trap_guard(ctx, any_zero, TrapCategory::DivModZero, dest)?;
                let divisor = if signed {
                    let min = self.lane_constant(ctx, dtype, 1 << (bits - 1), shape, dest);
                    let minus_one = self.lane_constant(ctx, dtype, u64::MAX, shape, dest);
                    let lhs_is_min = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, *lhs, min);
                    self.append(ctx, lhs_is_min.get_operation(), Some(dest));
                    let rhs_is_minus_one = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, *rhs, minus_one);
                    self.append(ctx, rhs_is_minus_one.get_operation(), Some(dest));
                    let overflowing = AndOp::new(
                        ctx,
                        lhs_is_min.get_result(ctx),
                        rhs_is_minus_one.get_result(ctx),
                    );
                    self.append(ctx, overflowing.get_operation(), Some(dest));
                    let one = self.lane_constant(ctx, dtype, 1, shape, dest);
                    let safe = SelectOp::new(ctx, overflowing.get_result(ctx), one, *rhs);
                    self.append(ctx, safe.get_operation(), Some(dest));
                    safe.get_result(ctx)
                } else {
                    *rhs
                };
                let operation = match (op, signed) {
                    (PopOp::Div, true) => SDivOp::new(ctx, *lhs, divisor).get_operation(),
                    (PopOp::Div, false) => UDivOp::new(ctx, *lhs, divisor).get_operation(),
                    (_, true) => SRemOp::new(ctx, *lhs, divisor).get_operation(),
                    (_, false) => URemOp::new(ctx, *lhs, divisor).get_operation(),
                };
                self.append(ctx, operation, Some(dest));
                operation.deref(ctx).get_result(0)
            }
            (PopOp::Abs, [value]) if signed => {
                let value_ty = value.get_type(ctx);
                let flag_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
                let fn_ty = FuncType::get(ctx, value_ty, vec![value_ty, flag_ty], false);
                let poison_on_min = self.bool_constant(ctx, false);
                let name = format!("llvm.abs.{}", lane_mangle(dtype, shape));
                let call = CallIntrinsicOp::new(
                    ctx,
                    StringAttr::new(name),
                    fn_ty,
                    vec![*value, poison_on_min],
                );
                self.append(ctx, call.get_operation(), Some(dest));
                call.get_result(ctx)
            }
            (PopOp::Abs | PopOp::Round | PopOp::Floor | PopOp::Ceil | PopOp::Trunc, [value]) => {
                *value
            }
            (PopOp::Max | PopOp::Min, [a, b]) => {
                let predicate = match (op, signed) {
                    (PopOp::Max, true) => ICmpPredicateAttr::SGT,
                    (PopOp::Max, false) => ICmpPredicateAttr::UGT,
                    (_, true) => ICmpPredicateAttr::SLT,
                    (_, false) => ICmpPredicateAttr::ULT,
                };
                let first = ICmpOp::new(ctx, predicate, *a, *b);
                self.append(ctx, first.get_operation(), Some(dest));
                let select = SelectOp::new(ctx, first.get_result(ctx), *a, *b);
                self.append(ctx, select.get_operation(), Some(dest));
                select.get_result(ctx)
            }
            (PopOp::Fma, [a, b, c]) => {
                let product = MulOp::new_with_overflow_flag(ctx, *a, *b, no_overflow_flags());
                self.append(ctx, product.get_operation(), Some(dest));
                let sum = AddOp::new_with_overflow_flag(
                    ctx,
                    product.get_result(ctx),
                    *c,
                    no_overflow_flags(),
                );
                self.append(ctx, sum.get_operation(), Some(dest));
                sum.get_result(ctx)
            }
            _ => {
                return Err(
                    self.unsupported_reg(format!("`pop.{}` operand count", op.mnemonic()), dest)
                );
            }
        })
    }

    /// A `pop` op over `bool` lanes (`i1` or `<N x i1>`): `max` is their
    /// `or` and `min` their `and`.
    fn pop_bool_value(
        &mut self,
        ctx: &mut Context,
        op: PopOp,
        values: &[Value],
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let operation = match (op, values) {
            (PopOp::Max, [a, b]) => OrOp::new(ctx, *a, *b).get_operation(),
            (PopOp::Min, [a, b]) => AndOp::new(ctx, *a, *b).get_operation(),
            _ => {
                return Err(
                    self.unsupported_reg(format!("`pop.{}` over bool lanes", op.mnemonic()), dest)
                );
            }
        };
        self.append(ctx, operation, Some(dest));
        Ok(operation.deref(ctx).get_result(0))
    }

    /// `index.cmp`'s unsigned compare of two `index` operands, held as
    /// `i64`, into its `i1`.
    fn lower_unsigned_compare(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        op: PopOp,
        operands: &[Reg],
    ) -> Result<(), PlironError> {
        let [lhs, rhs] = operands else {
            return Err(
                self.unsupported_reg(format!("`pop.{}` operand count", op.mnemonic()), dest)
            );
        };
        let predicate = match op {
            PopOp::Ult => ICmpPredicateAttr::ULT,
            PopOp::Ule => ICmpPredicateAttr::ULE,
            PopOp::Ugt => ICmpPredicateAttr::UGT,
            _ => ICmpPredicateAttr::UGE,
        };
        let lhs = self.reg_value(ctx, *lhs, ScalarTy::Int)?;
        let rhs = self.reg_value(ctx, *rhs, ScalarTy::Int)?;
        let compare = ICmpOp::new(ctx, predicate, lhs, rhs);
        self.define(ctx, dest, compare.get_operation(), compare.get_result(ctx))
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
            ScalarTy::Sized(dtype) if dtype.is_narrow_float() => {
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
            // `i1` unsigned predicates order `False` below `True`, which
            // is what `SIMD.lt`/`le`/`gt`/`ge` on a mask mean; the infix
            // operators never reach here, the checker rejecting all but
            // `==`/`!=` on bool lanes.
            ScalarTy::Bool => {
                if !matches!(
                    op,
                    InfixOp::Eq
                        | InfixOp::Ne
                        | InfixOp::Lt
                        | InfixOp::Le
                        | InfixOp::Gt
                        | InfixOp::Ge
                ) {
                    return Err(
                        self.unsupported_reg(format!("SIMD bool-lane operator `{op:?}`"), dest)
                    );
                }
                let cmp = ICmpOp::new(ctx, unsigned_predicate(op), lhs, rhs);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
            ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float16 | Dtype::Float32) => {
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
    /// the VM's f64 computation rounded once, and `Float16` lanes arrive
    /// widened). A zero divisor flows through as inf/NaN lanes.
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

    /// Round an f64 vector to the narrow float lane `dtype`, lane by lane
    /// (`Dtype::round_lane`).
    fn simd_round_float_vector(
        &mut self,
        ctx: &mut Context,
        dtype: Dtype,
        wide: Value,
        width: usize,
        dest: Reg,
    ) -> Value {
        let lane_vec = self.simd_vector_ty(ctx, dtype, width);
        let cast = FPTruncOp::new(ctx, wide, lane_vec);
        cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
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

/// LLVM's overloaded-intrinsic suffix for a lane of `dtype` (`f32`, `i64`),
/// or for `<width x lane>` when `shape` is a width.
fn lane_mangle(dtype: Dtype, shape: Option<usize>) -> String {
    match shape {
        Some(width) => simd_mangle(dtype, width),
        None => lane_suffix(dtype),
    }
}

/// LLVM's overloaded-intrinsic suffix for a `<width x lane>` vector
/// (`v4i32`, `v8f32`, `v16i1`).
fn simd_mangle(dtype: Dtype, width: usize) -> String {
    format!("v{width}{}", lane_suffix(dtype))
}

/// LLVM's overloaded-intrinsic suffix for one lane of `dtype`.
fn lane_suffix(dtype: Dtype) -> String {
    match dtype {
        Dtype::Float16 => "f16".to_string(),
        Dtype::Float32 => "f32".to_string(),
        Dtype::Float64 => "f64".to_string(),
        Dtype::Bool => "i1".to_string(),
        integer => {
            let (bits, _) =
                mojito_vm::runtime::integer_dtype_bits(integer).expect("integer SIMD dtype");
            format!("i{bits}")
        }
    }
}
