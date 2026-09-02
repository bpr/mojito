//! Floating/integer arithmetic helpers: pow, division variants,
//! conversions, comparisons, and trap guards.

use super::*;

impl<'a> FnLowering<'a> {
    /// `llvm.floor.f64` over one value.
    pub(super) fn float_floor(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        self.float_unary(ctx, "llvm.floor.f64", value, dest)
    }

    /// One unary f64 → f64 LLVM intrinsic (`llvm.floor.f64`,
    /// `llvm.ceil.f64`, `llvm.trunc.f64`, `llvm.round.f64`, `llvm.fabs.f64`).
    pub(super) fn float_unary(
        &mut self,
        ctx: &mut Context,
        intrinsic: &str,
        value: Value,
        dest: Reg,
    ) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let fn_ty = FuncType::get(ctx, f64_ty, vec![f64_ty], false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new(intrinsic.to_string()),
            fn_ty,
            vec![value],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// `x ** y` on Int/UInt: guard the exponent to `pow_exp`'s accepted range
    /// (`0 ..= u32::MAX`, one unsigned compare covers negative-as-i64 too),
    /// then call the wrapping `mjrt_pow` helper.
    pub(super) fn lower_pow(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let limit = self.int_constant(ctx, u32::MAX as i64);
        let out_of_range = ICmpOp::new(ctx, ICmpPredicateAttr::UGT, rhs, limit);
        self.append(ctx, out_of_range.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            out_of_range.get_result(ctx),
            TrapCategory::PowExponent,
            dest,
        )?;
        let pow_ty = self.shared.ensure_pow(ctx);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_pow".try_into().expect("valid identifier")),
            pow_ty,
            vec![lhs, rhs],
        );
        self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
    }

    /// `/`: promote both operands to f64 (`sitofp`/`uitofp`; float operands
    /// pass through) and divide.
    pub(super) fn lower_true_div(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        a: Reg,
        b: Reg,
        operand_ty: ScalarTy,
    ) -> Result<(), PlironError> {
        if matches!(operand_ty, ScalarTy::Bool | ScalarTy::Ptr) {
            return Err(self.unsupported_reg(
                format!("operator `Div` on {} operands", operand_ty.name()),
                dest,
            ));
        }
        // Sized integer lanes have no `/` (the checker admits SIMD division
        // on float lanes only); reject as a backstop rather than promote.
        if let ScalarTy::Sized(dtype) = operand_ty
            && dtype != Dtype::Float32
        {
            return Err(self.unsupported_reg(
                format!("operator `Div` on {} operands", operand_ty.name()),
                dest,
            ));
        }
        let lhs = self.reg_value(ctx, a, operand_ty)?;
        let rhs = self.reg_value(ctx, b, operand_ty)?;
        // `Float32 / Float32` stays a Float32 lane: divide at f64 and round
        // (`runtime::simd_binop`), unlike the scalar promotions below.
        if operand_ty == ScalarTy::Sized(Dtype::Float32) {
            let wide_lhs = self.f32_to_f64(ctx, lhs, dest);
            let wide_rhs = self.f32_to_f64(ctx, rhs, dest);
            let div = FDivOp::new_with_fast_math_flags(
                ctx,
                wide_lhs,
                wide_rhs,
                FastmathFlagsAttr::default(),
            );
            self.append(ctx, div.get_operation(), Some(dest));
            let rounded = self.f64_to_f32(ctx, div.get_result(ctx), dest);
            self.reg_values.insert(dest.0, rounded);
            return Ok(());
        }
        let (lhs, rhs) = match operand_ty {
            ScalarTy::Float64 => (lhs, rhs),
            ScalarTy::Int => (
                self.int_to_f64(ctx, lhs, dest),
                self.int_to_f64(ctx, rhs, dest),
            ),
            ScalarTy::UInt => (
                self.uint_to_f64(ctx, lhs, dest),
                self.uint_to_f64(ctx, rhs, dest),
            ),
            ScalarTy::Bool | ScalarTy::Ptr | ScalarTy::Sized(_) => unreachable!("rejected above"),
        };
        let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, FastmathFlagsAttr::default());
        self.define(ctx, dest, div.get_operation(), div.get_result(ctx))
    }

    pub(super) fn int_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = SIToFPOp::new(ctx, value, f64_ty);
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    pub(super) fn uint_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    /// Widen a `Float32` SSA value to its f64 view (exact — the VM stores
    /// f32 lanes as f64 views).
    pub(super) fn f32_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = FPExtOp::new(ctx, value, f64_ty);
        cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    /// Round an f64 value to single precision (the VM's `round_f32`).
    pub(super) fn f64_to_f32(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f32_ty: TypeHandle = FP32Type::get(ctx).into();
        let cast = FPTruncOp::new(ctx, value, f32_ty);
        cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    /// A sized integer lane's mathematical value as i64: sign-extend a
    /// signed lane, zero-extend an unsigned one (the VM's i128 lane content,
    /// which always fits i64 bits for 64-bit-and-under lanes).
    pub(super) fn sized_to_i64(
        &mut self,
        ctx: &mut Context,
        value: Value,
        dtype: Dtype,
        dest: Reg,
    ) -> Value {
        let (bits, signed) = crate::runtime::integer_dtype_bits(dtype)
            .expect("sized_to_i64 takes integer dtypes only");
        if bits == 64 {
            return value;
        }
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        if signed {
            let cast = SExtOp::new(ctx, value, i64_ty);
            self.append(ctx, cast.get_operation(), Some(dest));
            cast.get_result(ctx)
        } else {
            let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
            self.append(ctx, cast.get_operation(), Some(dest));
            cast.get_result(ctx)
        }
    }

    /// Resize an integer value from `from` to `to` bits along its
    /// mathematical value (`from_signed` selects the extension): the VM's
    /// `wrap` at the target width.
    pub(super) fn resize_int(
        &mut self,
        ctx: &mut Context,
        value: Value,
        from: (u32, bool),
        to: u32,
        dest: Reg,
    ) -> Value {
        let (from_bits, from_signed) = from;
        if from_bits == to {
            return value;
        }
        let to_ty: TypeHandle = IntegerType::get(ctx, to, Signedness::Signless).into();
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

    pub(super) fn lower_compare(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        lhs: Value,
        rhs: Value,
        operand_ty: ScalarTy,
    ) -> Result<(), PlironError> {
        match operand_ty {
            ScalarTy::Bool => {
                if !matches!(op, InfixOp::Eq | InfixOp::Ne) {
                    return Err(
                        self.unsupported_reg(format!("operator `{op:?}` on Bool operands"), dest)
                    );
                }
                let predicate = if matches!(op, InfixOp::Eq) {
                    ICmpPredicateAttr::EQ
                } else {
                    ICmpPredicateAttr::NE
                };
                let cmp = ICmpOp::new(ctx, predicate, lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            ScalarTy::Int => {
                let cmp = ICmpOp::new(ctx, signed_predicate(op), lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            ScalarTy::UInt => {
                let cmp = ICmpOp::new(ctx, unsigned_predicate(op), lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            // Pointer identity: `==`/`!=` compare addresses (the VM compares
            // allocation identity); ordered comparisons stay unsupported.
            ScalarTy::Ptr => {
                if !matches!(op, InfixOp::Eq | InfixOp::Ne) {
                    return Err(self
                        .unsupported_reg(format!("operator `{op:?}` on Pointer operands"), dest));
                }
                let predicate = if matches!(op, InfixOp::Eq) {
                    ICmpPredicateAttr::EQ
                } else {
                    ICmpPredicateAttr::NE
                };
                let cmp = ICmpOp::new(ctx, predicate, lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            // Rust f64 comparisons: `!=` is true for NaN operands (UNE), the
            // ordered comparisons are false (`runtime::float_op`).
            ScalarTy::Float64 | ScalarTy::Sized(Dtype::Float32) => {
                let cmp = self.fcmp(ctx, float_predicate(op), lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            // Sized integer lanes compare on their mathematical values
            // (`runtime::int_cmp` over the sign-carrying i128 lane).
            ScalarTy::Sized(dtype) => {
                let (_, signed) = crate::runtime::integer_dtype_bits(dtype)
                    .expect("float dtypes are matched above");
                let predicate = if signed {
                    signed_predicate(op)
                } else {
                    unsigned_predicate(op)
                };
                let cmp = ICmpOp::new(ctx, predicate, lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
        }
    }

    pub(super) fn fcmp(
        &mut self,
        ctx: &mut Context,
        predicate: FCmpPredicateAttr,
        lhs: Value,
        rhs: Value,
    ) -> FCmpOp {
        let cmp = FCmpOp::new(ctx, predicate, lhs, rhs);
        cmp.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        cmp
    }

    /// `floor_div`: `sdiv` rounds toward zero; subtract one when the remainder
    /// is non-zero and the operand signs differ (matches `runtime.rs`).
    pub(super) fn lower_floor_div(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let value = self.floor_div_value(ctx, dest, lhs, rhs)?;
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// The flooring quotient as a bare value (shared with `divmod`, which
    /// computes both halves for one destination).
    pub(super) fn floor_div_value(
        &mut self,
        ctx: &mut Context,
        span_reg: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, PlironError> {
        let quotient = SDivOp::new(ctx, lhs, rhs);
        self.append(ctx, quotient.get_operation(), Some(span_reg));
        let adjust = self.floor_adjust_flag(ctx, span_reg, lhs, rhs)?;
        let one = self.int_constant(ctx, 1);
        let minus_one =
            SubOp::new_with_overflow_flag(ctx, quotient.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, minus_one.get_operation(), Some(span_reg));
        let select = SelectOp::new(
            ctx,
            adjust,
            minus_one.get_result(ctx),
            quotient.get_result(ctx),
        );
        self.append(ctx, select.get_operation(), Some(span_reg));
        Ok(select.get_result(ctx))
    }

    /// `floor_mod`: `srem` takes the dividend's sign; add the divisor when the
    /// remainder is non-zero and the operand signs differ.
    pub(super) fn lower_floor_mod(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let value = self.floor_mod_value(ctx, dest, lhs, rhs)?;
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// The flooring remainder as a bare value (shared with `divmod`).
    pub(super) fn floor_mod_value(
        &mut self,
        ctx: &mut Context,
        span_reg: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, PlironError> {
        let adjust = self.floor_adjust_flag(ctx, span_reg, lhs, rhs)?;
        let remainder = SRemOp::new(ctx, lhs, rhs);
        self.append(ctx, remainder.get_operation(), Some(span_reg));
        let plus_divisor =
            AddOp::new_with_overflow_flag(ctx, remainder.get_result(ctx), rhs, no_overflow_flags());
        self.append(ctx, plus_divisor.get_operation(), Some(span_reg));
        let select = SelectOp::new(
            ctx,
            adjust,
            plus_divisor.get_result(ctx),
            remainder.get_result(ctx),
        );
        self.append(ctx, select.get_operation(), Some(span_reg));
        Ok(select.get_result(ctx))
    }

    /// Replace the divisor with `1` in the single overflowing signed case
    /// (`lhs == i64::MIN && rhs == -1`): LLVM `sdiv`/`srem` are poison there,
    /// while the ABI defines the wrapped results `i64::MIN` and `0` — exactly
    /// what the floor expansions produce for a divisor of `1`.
    pub(super) fn sanitized_divisor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, PlironError> {
        let min = self.int_constant(ctx, i64::MIN);
        let minus_one = self.int_constant(ctx, -1);
        let lhs_is_min = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, lhs, min);
        self.append(ctx, lhs_is_min.get_operation(), Some(dest));
        let rhs_is_minus_one = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, rhs, minus_one);
        self.append(ctx, rhs_is_minus_one.get_operation(), Some(dest));
        let overflowing = AndOp::new(
            ctx,
            lhs_is_min.get_result(ctx),
            rhs_is_minus_one.get_result(ctx),
        );
        self.append(ctx, overflowing.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let safe = SelectOp::new(ctx, overflowing.get_result(ctx), one, rhs);
        self.append(ctx, safe.get_operation(), Some(dest));
        Ok(safe.get_result(ctx))
    }

    /// `(srem(lhs, rhs) != 0) & ((srem(lhs, rhs) ^ rhs) < 0)` — true exactly
    /// when truncating division must be floored.
    pub(super) fn floor_adjust_flag(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, PlironError> {
        let remainder = SRemOp::new(ctx, lhs, rhs);
        self.append(ctx, remainder.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let non_zero = ICmpOp::new(ctx, ICmpPredicateAttr::NE, remainder.get_result(ctx), zero);
        self.append(ctx, non_zero.get_operation(), Some(dest));
        let mixed = XorOp::new(ctx, remainder.get_result(ctx), rhs);
        self.append(ctx, mixed.get_operation(), Some(dest));
        let negative = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, mixed.get_result(ctx), zero);
        self.append(ctx, negative.get_operation(), Some(dest));
        let adjust = AndOp::new(ctx, non_zero.get_result(ctx), negative.get_result(ctx));
        self.append(ctx, adjust.get_operation(), Some(dest));
        Ok(adjust.get_result(ctx))
    }

    /// Trap when the divisor is zero (the VM's `nonzero`/`nonzero_u` check
    /// behind "integer division or modulo by zero").
    pub(super) fn emit_div_zero_guard(
        &mut self,
        ctx: &mut Context,
        divisor: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let zero = self.int_constant(ctx, 0);
        let is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, divisor, zero);
        self.append(ctx, is_zero.get_operation(), Some(dest));
        self.emit_trap_guard(ctx, is_zero.get_result(ctx), TrapCategory::DivModZero, dest)
    }

    /// Split the current block on `cond`: branch to the per-category trap
    /// block when true, continue lowering in a fresh block when false.
    pub(super) fn emit_trap_guard(
        &mut self,
        ctx: &mut Context,
        cond: Value,
        category: TrapCategory,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let trap = self.trap_block(ctx, category);
        let region = self.region.expect("lowering is inside a function region");
        let cont = BasicBlock::new(ctx, None, vec![]);
        cont.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, cond, trap, vec![], cont, vec![]);
        self.append(ctx, branch.get_operation(), Some(dest));
        self.current = Some(cont);
        Ok(())
    }

    /// Trap when an inline uninit-storage presence bit is false.
    pub(super) fn guard_uninit_present(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        category: TrapCategory,
        span: Reg,
    ) -> Result<(), PlironError> {
        let i1: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let flag = LoadOp::new(ctx, storage, i1);
        self.append(ctx, flag.get_operation(), Some(span));
        let absent = self.bool_constant(ctx, false);
        let missing = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, flag.get_result(ctx), absent);
        self.append(ctx, missing.get_operation(), Some(span));
        self.emit_trap_guard(ctx, missing.get_result(ctx), category, span)
    }

    /// The function's trap block for `category`: `mjrt_trap(code)` (which
    /// reports on stderr and exits `64 + code`) then `unreachable`, created on
    /// first use.
    pub(super) fn trap_block(
        &mut self,
        ctx: &mut Context,
        category: TrapCategory,
    ) -> Ptr<BasicBlock> {
        if let Some(block) = self.trap_blocks.get(&category.code()) {
            return *block;
        }
        let region = self.region.expect("lowering is inside a function region");
        let trap_ty = self.shared.ensure_rt(ctx, "mjrt_trap");
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
        let code_attr =
            IntegerAttr::new(i32_ty, APInt::from_u64(u64::from(category.code()), bw(32)));
        let code = ConstantOp::new(ctx, Box::new(code_attr));
        code.get_operation().insert_at_back(block, ctx);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_trap".try_into().expect("valid identifier")),
            trap_ty,
            vec![code.get_result(ctx)],
        );
        call.get_operation().insert_at_back(block, ctx);
        let unreachable = UnreachableOp::new(ctx);
        unreachable.get_operation().insert_at_back(block, ctx);
        self.trap_blocks.insert(category.code(), block);
        block
    }
}
