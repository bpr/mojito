//! Constant and literal materialization plus unary operators.

use super::*;

impl<'a> FnLowering<'a> {
    pub(super) fn lower_const(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        k: &MirConst,
    ) -> Result<(), PlironError> {
        match k {
            MirConst::Int(value) => {
                let constant = self.int_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            MirConst::Float(value) => {
                let constant = self.float_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            MirConst::Bool(value) => {
                let constant = self.bool_constant(ctx, *value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            MirConst::IntLiteral(literal) => {
                self.pending_literals
                    .insert(dest.0, PendingLiteral::Int(literal.clone()));
                Ok(())
            }
            MirConst::FloatLiteral(literal) => {
                self.pending_literals
                    .insert(dest.0, PendingLiteral::Float(literal.clone()));
                Ok(())
            }
            MirConst::Str(text) => {
                self.str_consts.insert(dest.0, text.as_bytes().to_vec());
                Ok(())
            }
            // The unit constant is zero-sized: consumers type it `None` and
            // never read a materialized value (`print` writes its constant
            // text, stores are no-ops).
            MirConst::None => {
                self.erased.insert(dest.0);
                Ok(())
            }
            // A bare function value is the two-word callable with a null
            // environment; its thunk ignores the environment argument.
            MirConst::Function(name) => self.lower_make_closure(ctx, dest, name, &[]),
        }
    }

    pub(super) fn lower_materialize(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        target: &Ty,
    ) -> Result<(), PlironError> {
        let target = match target {
            Ty::Int => ScalarTy::Int,
            Ty::UInt => ScalarTy::UInt,
            Ty::Float64 => ScalarTy::Float64,
            Ty::Simd { dtype, width: 1 } => ScalarTy::of_dtype(*dtype),
            // Literal-typed storage holds the exact value at its default
            // width, rejecting what i64/f64 cannot represent.
            Ty::IntLiteral | Ty::FloatLiteral => {
                let Some(literal) = self.pending_literals.get(&value.0).cloned() else {
                    if let Some(materialized) = self.reg_values.get(&value.0).copied() {
                        self.reg_values.insert(dest.0, materialized);
                        return Ok(());
                    }
                    return Err(self.unsupported_reg(
                        "literal materialization of a non-literal register".into(),
                        dest,
                    ));
                };
                let constant = self.exact_literal_storage(ctx, &literal, target, dest)?;
                self.reg_values.insert(dest.0, constant);
                return Ok(());
            }
            other => {
                return Err(
                    self.unsupported_reg(format!("literal materialization to `{other:?}`"), dest)
                );
            }
        };
        // A literal register may also have been materialized on demand by an
        // earlier direct consumer; alias its value in that case.
        let Some(literal) = self.pending_literals.get(&value.0).cloned() else {
            if let Some(materialized) = self.reg_values.get(&value.0).copied() {
                self.reg_values.insert(dest.0, materialized);
                return Ok(());
            }
            return Err(self.unsupported_reg(
                "literal materialization of a non-literal register".into(),
                dest,
            ));
        };
        let constant = self.materialize_pending(ctx, &literal, target, dest)?;
        self.reg_values.insert(dest.0, constant);
        Ok(())
    }

    /// A pending literal as its typed-storage constant: `IntLiteral` storage
    /// is an exact i64 and `FloatLiteral` storage the literal's f64 value.
    /// A constant the storage cannot hold rejects — the VM keeps arbitrary
    /// precision in literal-typed slots, so wrapping here would silently
    /// diverge from the oracle (the recorded reject-never-wrap policy).
    pub(super) fn exact_literal_storage(
        &mut self,
        ctx: &mut Context,
        literal: &PendingLiteral,
        target: &Ty,
        span_reg: Reg,
    ) -> Result<Value, PlironError> {
        match (literal, target) {
            (PendingLiteral::Int(literal), Ty::IntLiteral) => {
                let value = literal.to_i64().ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "IntLiteral storage (i64)",
                        span_reg,
                    )
                })?;
                Ok(self.int_constant(ctx, value))
            }
            (PendingLiteral::Int(literal), _) => {
                let value = literal.to_f64().ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "FloatLiteral storage (f64)",
                        span_reg,
                    )
                })?;
                Ok(self.float_constant(ctx, value))
            }
            (PendingLiteral::Float(literal), Ty::FloatLiteral) => {
                let value = literal.to_f64().ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.to_string(),
                        "FloatLiteral storage (f64)",
                        span_reg,
                    )
                })?;
                Ok(self.float_constant(ctx, value))
            }
            (PendingLiteral::Float(literal), _) => Err(self.unsupported(
                format!("float literal `{literal}` as IntLiteral storage"),
                self.reg_span(span_reg),
            )),
        }
    }

    pub(super) fn lower_unop(
        &mut self,
        ctx: &mut Context,
        op: PrefixOp,
        dest: Reg,
        a: Reg,
    ) -> Result<(), PlironError> {
        if let Some(Ty::Simd { dtype, width }) = self.func.reg_types.get(&a.0).cloned()
            && width > 1
        {
            return self.lower_simd_unop(ctx, op, dest, a, dtype, width as usize);
        }
        // Negation of a pending literal stays a pending literal (the
        // materialization folds the sign into one constant).
        if let Some(literal) = self.pending_literals.get(&a.0).cloned() {
            if !matches!(op, PrefixOp::Neg) {
                return Err(self.unsupported_reg(format!("operator `{op:?}` on a literal"), dest));
            }
            let negated = match literal {
                PendingLiteral::Int(literal) => PendingLiteral::Int(literal.neg()),
                PendingLiteral::Float(literal) => PendingLiteral::Float(literal.neg()),
            };
            self.pending_literals.insert(dest.0, negated);
            return Ok(());
        }
        let operand_ty = self
            .concrete_scalar_ty(a)?
            .ok_or_else(|| self.unsupported_reg("untyped unary operand".into(), dest))?;
        let value = self.reg_value(ctx, a, operand_ty)?;
        match (op, operand_ty) {
            (PrefixOp::Neg, ScalarTy::Int) => {
                let zero = self.int_constant(ctx, 0);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Neg, ScalarTy::Float64) => {
                let neg =
                    FNegOp::new_with_fast_math_flags(ctx, value, FastmathFlagsAttr::default());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Not, ScalarTy::Bool) => {
                let one = self.bool_constant(ctx, true);
                let not = XorOp::new(ctx, value, one);
                self.define(ctx, dest, not.get_operation(), not.get_result(ctx))
            }
            // Sized-lane negation: `0 - x` wraps at the lane width for
            // integers; f32 negation is exact, so no widen/round dance.
            (PrefixOp::Neg, ScalarTy::Sized(Dtype::Float32)) => {
                let neg =
                    FNegOp::new_with_fast_math_flags(ctx, value, FastmathFlagsAttr::default());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (PrefixOp::Neg, ScalarTy::Sized(dtype)) => {
                let zero = self.sized_int_constant(ctx, dtype, 0);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            (op, other) => Err(self.unsupported_reg(
                format!("operator `{op:?}` on `{}` operand", other.name()),
                dest,
            )),
        }
    }
}
