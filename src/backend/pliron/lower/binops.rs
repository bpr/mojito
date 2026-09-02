//! Binary operator lowering across the sized-int/float lattice.

use super::*;

impl<'a> FnLowering<'a> {
    pub(super) fn lower_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
        resolved: Option<&str>,
    ) -> Result<(), PlironError> {
        if let Some(target) = resolved {
            return Err(self.unsupported_reg(format!("nominal operator overload `{target}`"), dest));
        }
        if let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&a.0).cloned()
            && width > 1
        {
            return self.lower_simd_binop(ctx, op, dest, a, b, dtype, width as usize);
        }
        if self.str_consts.contains_key(&a.0) && self.str_consts.contains_key(&b.0) {
            return self.lower_str_literal_binop(ctx, op, dest, a, b);
        }
        // Equality over runtime string-literal values (a `Dict[StringLiteral,
        // …]` key probe) compares bytes — the VM's `Value::Str` equality.
        let string_shaped = |lowering: &Self, reg: Reg| {
            lowering.str_consts.contains_key(&reg.0)
                || lowering.str_runtime.contains_key(&reg.0)
                || matches!(lowering.func.reg_types.get(&reg.0), Some(Ty::StringLiteral))
        };
        if matches!(op, InfixOp::Eq | InfixOp::Ne)
            && string_shaped(self, a)
            && string_shaped(self, b)
        {
            return self.lower_str_runtime_eq(ctx, op, dest, a, b);
        }
        if self.str_consts.contains_key(&a.0) || self.str_consts.contains_key(&b.0) {
            return self.lower_str_literal_binop(ctx, op, dest, a, b);
        }
        // `pointer + i` — provenance-preserving element arithmetic (the MIR
        // form of `unsafe_offset`): the address `i * sizeof(element)` bytes
        // on (the VM adds `i` to its element-counted offset).
        if let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&a.0).cloned() {
            if !matches!(op, InfixOp::Add) {
                return Err(
                    self.unsupported_reg(format!("operator `{op:?}` on Pointer operands"), dest)
                );
            }
            let ptr = self.reg_value(ctx, a, ScalarTy::Ptr)?;
            let address = self.pointer_element_address(ctx, ptr, b, &element, dest)?;
            self.reg_values.insert(dest.0, address);
            return Ok(());
        }
        let operand_ty = self.binop_operand_ty(a, b)?;

        // True division always computes in f64 and yields Float64
        // (`runtime::numeric_op`), regardless of operand kind.
        if matches!(op, InfixOp::Div) {
            return self.lower_true_div(ctx, dest, a, b, operand_ty);
        }

        let lhs = self.reg_value(ctx, a, operand_ty)?;
        let rhs = self.reg_value(ctx, b, operand_ty)?;

        if is_comparison(op) {
            return self.lower_compare(ctx, op, dest, lhs, rhs, operand_ty);
        }

        match operand_ty {
            ScalarTy::Bool => {
                match op {
                    InfixOp::BitAnd => {
                        let and = AndOp::new(ctx, lhs, rhs);
                        self.define(ctx, dest, and.get_operation(), and.get_result(ctx))
                    }
                    InfixOp::BitOr => {
                        let or = OrOp::new(ctx, lhs, rhs);
                        self.define(ctx, dest, or.get_operation(), or.get_result(ctx))
                    }
                    InfixOp::BitXor => {
                        let xor = XorOp::new(ctx, lhs, rhs);
                        self.define(ctx, dest, xor.get_operation(), xor.get_result(ctx))
                    }
                    other => Err(self
                        .unsupported_reg(format!("operator `{other:?}` on Bool operands"), dest)),
                }
            }
            ScalarTy::Float64 => self.lower_float_binop(ctx, op, dest, lhs, rhs),
            ScalarTy::Int => self.lower_int_binop(ctx, op, dest, lhs, rhs),
            ScalarTy::UInt => self.lower_uint_binop(ctx, op, dest, lhs, rhs),
            ScalarTy::Sized(Dtype::Float32) => self.lower_f32_binop(ctx, op, dest, lhs, rhs),
            ScalarTy::Sized(dtype) => self.lower_sized_int_binop(ctx, op, dest, lhs, rhs, dtype),
            ScalarTy::Ptr => {
                Err(self.unsupported_reg(format!("operator `{op:?}` on Pointer operands"), dest))
            }
        }
    }

    /// Sized integer lanes support exactly the checker's SIMD operator set:
    /// wrapping `+`/`-`/`*` at the lane width (native iN arithmetic wraps by
    /// construction, matching `runtime::wrap` after exact i128 arithmetic).
    /// Comparisons split off earlier; everything else is rejected here as a
    /// backstop — the checker refuses it before MIR exists.
    pub(super) fn lower_sized_int_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
        dtype: Dtype,
    ) -> Result<(), PlironError> {
        match op {
            InfixOp::Add => {
                let add = AddOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, add.get_operation(), add.get_result(ctx))
            }
            InfixOp::Sub => {
                let sub = SubOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, sub.get_operation(), sub.get_result(ctx))
            }
            InfixOp::Mul => {
                let mul = MulOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, mul.get_operation(), mul.get_result(ctx))
            }
            InfixOp::BitAnd => {
                let value = AndOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
            }
            InfixOp::BitOr => {
                let value = OrOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
            }
            InfixOp::BitXor => {
                let value = XorOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
            }
            InfixOp::Shl | InfixOp::Shr => {
                let (bits, signed) =
                    crate::runtime::integer_dtype_bits(dtype).expect("sized integer SIMD dtype");
                let mask = self.sized_int_constant(ctx, dtype, u64::from(bits - 1));
                let masked = AndOp::new(ctx, rhs, mask);
                self.append(ctx, masked.get_operation(), Some(dest));
                match op {
                    InfixOp::Shl => {
                        let value = ShlOp::new_with_overflow_flag(
                            ctx,
                            lhs,
                            masked.get_result(ctx),
                            no_overflow_flags(),
                        );
                        self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
                    }
                    InfixOp::Shr if signed => {
                        let value = AShrOp::new(ctx, lhs, masked.get_result(ctx));
                        self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
                    }
                    InfixOp::Shr => {
                        let value = LShrOp::new(ctx, lhs, masked.get_result(ctx));
                        self.define(ctx, dest, value.get_operation(), value.get_result(ctx))
                    }
                    _ => unreachable!(),
                }
            }
            other => Err(self.unsupported_reg(
                format!(
                    "operator `{other:?}` on {} operands",
                    ScalarTy::Sized(dtype).name()
                ),
                dest,
            )),
        }
    }

    /// `Float32` arithmetic: the VM computes each operation at f64 and rounds
    /// the result to single precision (`round_lane`), so the lowering widens,
    /// operates at f64, and truncates — never direct f32 arithmetic, whose
    /// single rounding differs from the VM's double rounding in edge cases.
    pub(super) fn lower_f32_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let flags = FastmathFlagsAttr::default;
        let wide_lhs = self.f32_to_f64(ctx, lhs, dest);
        let wide_rhs = self.f32_to_f64(ctx, rhs, dest);
        let wide = match op {
            InfixOp::Add => {
                let add = FAddOp::new_with_fast_math_flags(ctx, wide_lhs, wide_rhs, flags());
                self.append(ctx, add.get_operation(), Some(dest));
                add.get_result(ctx)
            }
            InfixOp::Sub => {
                let sub = FSubOp::new_with_fast_math_flags(ctx, wide_lhs, wide_rhs, flags());
                self.append(ctx, sub.get_operation(), Some(dest));
                sub.get_result(ctx)
            }
            InfixOp::Mul => {
                let mul = FMulOp::new_with_fast_math_flags(ctx, wide_lhs, wide_rhs, flags());
                self.append(ctx, mul.get_operation(), Some(dest));
                mul.get_result(ctx)
            }
            other => {
                return Err(
                    self.unsupported_reg(format!("operator `{other:?}` on Float32 operands"), dest)
                );
            }
        };
        let rounded = self.f64_to_f32(ctx, wide, dest);
        self.reg_values.insert(dest.0, rounded);
        Ok(())
    }

    pub(super) fn lower_int_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        match op {
            InfixOp::Add => {
                let add = AddOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, add.get_operation(), add.get_result(ctx))
            }
            InfixOp::Sub => {
                let sub = SubOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, sub.get_operation(), sub.get_result(ctx))
            }
            InfixOp::Mul => {
                let mul = MulOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, mul.get_operation(), mul.get_result(ctx))
            }
            InfixOp::BitAnd => {
                let and = AndOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, and.get_operation(), and.get_result(ctx))
            }
            InfixOp::BitOr => {
                let or = OrOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, or.get_operation(), or.get_result(ctx))
            }
            InfixOp::BitXor => {
                let xor = XorOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, xor.get_operation(), xor.get_result(ctx))
            }
            InfixOp::Shl => {
                let masked = self.masked_shift_amount(ctx, rhs, dest);
                let shl = ShlOp::new_with_overflow_flag(ctx, lhs, masked, no_overflow_flags());
                self.define(ctx, dest, shl.get_operation(), shl.get_result(ctx))
            }
            InfixOp::Shr => {
                let masked = self.masked_shift_amount(ctx, rhs, dest);
                let shr = AShrOp::new(ctx, lhs, masked);
                self.define(ctx, dest, shr.get_operation(), shr.get_result(ctx))
            }
            InfixOp::FloorDiv => {
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let rhs = self.sanitized_divisor(ctx, dest, lhs, rhs)?;
                self.lower_floor_div(ctx, dest, lhs, rhs)
            }
            InfixOp::Mod => {
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let rhs = self.sanitized_divisor(ctx, dest, lhs, rhs)?;
                self.lower_floor_mod(ctx, dest, lhs, rhs)
            }
            InfixOp::Pow => self.lower_pow(ctx, dest, lhs, rhs),
            other => {
                Err(self.unsupported_reg(format!("operator `{other:?}` on Int operands"), dest))
            }
        }
    }

    pub(super) fn lower_uint_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        match op {
            InfixOp::Add => {
                let add = AddOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, add.get_operation(), add.get_result(ctx))
            }
            InfixOp::Sub => {
                let sub = SubOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, sub.get_operation(), sub.get_result(ctx))
            }
            InfixOp::Mul => {
                let mul = MulOp::new_with_overflow_flag(ctx, lhs, rhs, no_overflow_flags());
                self.define(ctx, dest, mul.get_operation(), mul.get_result(ctx))
            }
            InfixOp::BitAnd => {
                let and = AndOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, and.get_operation(), and.get_result(ctx))
            }
            InfixOp::BitOr => {
                let or = OrOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, or.get_operation(), or.get_result(ctx))
            }
            InfixOp::BitXor => {
                let xor = XorOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, xor.get_operation(), xor.get_result(ctx))
            }
            InfixOp::Shl => {
                let masked = self.masked_shift_amount(ctx, rhs, dest);
                let shl = ShlOp::new_with_overflow_flag(ctx, lhs, masked, no_overflow_flags());
                self.define(ctx, dest, shl.get_operation(), shl.get_result(ctx))
            }
            // `>>` on UInt is a logical shift (the VM's `wrapping_shr` over
            // u64), unlike the arithmetic shift on Int.
            InfixOp::Shr => {
                let masked = self.masked_shift_amount(ctx, rhs, dest);
                let shr = LShrOp::new(ctx, lhs, masked);
                self.define(ctx, dest, shr.get_operation(), shr.get_result(ctx))
            }
            // UInt floor division/modulo are plain unsigned `/` and `%`
            // (`runtime::uint_op`), behind the same zero trap.
            InfixOp::FloorDiv => {
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let div = UDivOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, div.get_operation(), div.get_result(ctx))
            }
            InfixOp::Mod => {
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let rem = URemOp::new(ctx, lhs, rhs);
                self.define(ctx, dest, rem.get_operation(), rem.get_result(ctx))
            }
            InfixOp::Pow => self.lower_pow(ctx, dest, lhs, rhs),
            other => {
                Err(self.unsupported_reg(format!("operator `{other:?}` on UInt operands"), dest))
            }
        }
    }

    pub(super) fn lower_float_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let flags = FastmathFlagsAttr::default;
        match op {
            InfixOp::Add => {
                let add = FAddOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.define(ctx, dest, add.get_operation(), add.get_result(ctx))
            }
            InfixOp::Sub => {
                let sub = FSubOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.define(ctx, dest, sub.get_operation(), sub.get_result(ctx))
            }
            InfixOp::Mul => {
                let mul = FMulOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.define(ctx, dest, mul.get_operation(), mul.get_result(ctx))
            }
            // Float floor division/modulo have no zero trap: `(x/y).floor()`
            // and `x - y*(x/y).floor()` (`runtime::float_op`) — division by
            // zero flows through as inf/NaN, and `%` is NOT `frem`.
            InfixOp::FloorDiv => {
                let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, div.get_operation(), Some(dest));
                let floored = self.float_floor(ctx, div.get_result(ctx), dest);
                self.reg_values.insert(dest.0, floored);
                Ok(())
            }
            InfixOp::Mod => {
                let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, div.get_operation(), Some(dest));
                let floored = self.float_floor(ctx, div.get_result(ctx), dest);
                let scaled = FMulOp::new_with_fast_math_flags(ctx, rhs, floored, flags());
                self.append(ctx, scaled.get_operation(), Some(dest));
                let rem =
                    FSubOp::new_with_fast_math_flags(ctx, lhs, scaled.get_result(ctx), flags());
                self.define(ctx, dest, rem.get_operation(), rem.get_result(ctx))
            }
            // Float `**` is the VM's `f64::powf` — both resolve to the host
            // libm `pow`.
            InfixOp::Pow => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let fn_ty = FuncType::get(ctx, f64_ty, vec![f64_ty, f64_ty], false);
                let call = CallIntrinsicOp::new(
                    ctx,
                    StringAttr::new("llvm.pow.f64".to_string()),
                    fn_ty,
                    vec![lhs, rhs],
                );
                self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
            }
            other => {
                Err(self.unsupported_reg(format!("operator `{other:?}` on Float64 operands"), dest))
            }
        }
    }
}
