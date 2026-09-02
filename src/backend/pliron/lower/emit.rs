//! SSA emission plumbing: constants, op append/define, register
//! values, pending-literal materialization, and diagnostics.

use super::*;

impl<'a> FnLowering<'a> {
    /// Mask a shift amount with `& 63`, matching the VM's
    /// `wrapping_shl`/`wrapping_shr` modulo-width semantics.
    pub(super) fn masked_shift_amount(
        &mut self,
        ctx: &mut Context,
        amount: Value,
        dest: Reg,
    ) -> Value {
        let mask = self.int_constant(ctx, 63);
        let masked = AndOp::new(ctx, amount, mask);
        self.append(ctx, masked.get_operation(), Some(dest));
        masked.get_result(ctx)
    }

    /// Emit an i64 constant in the current block and return its value.
    pub(super) fn int_constant(&mut self, ctx: &mut Context, value: i64) -> Value {
        let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_ty, APInt::from_u64(value as u64, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an i64 constant carrying `value`'s unsigned bits.
    pub(super) fn uint_constant(&mut self, ctx: &mut Context, value: u64) -> Value {
        let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_ty, APInt::from_u64(value, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an f64 constant in the current block and return its value.
    pub(super) fn float_constant(&mut self, ctx: &mut Context, value: f64) -> Value {
        let attr = FPDoubleAttr::from(value);
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an f32 constant in the current block and return its value.
    pub(super) fn f32_constant(&mut self, ctx: &mut Context, value: f32) -> Value {
        let attr = FPSingleAttr::from(value);
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an integer constant at a sized lane width, carrying `value`'s
    /// low `bits` bits.
    pub(super) fn sized_int_constant(
        &mut self,
        ctx: &mut Context,
        dtype: Dtype,
        value: u64,
    ) -> Value {
        let (bits, _) = crate::runtime::integer_dtype_bits(dtype)
            .expect("sized_int_constant takes integer dtypes only");
        let masked = if bits == 64 {
            value
        } else {
            value & ((1u64 << bits) - 1)
        };
        let int_ty = IntegerType::get(ctx, bits, Signedness::Signless);
        let attr = IntegerAttr::new(int_ty, APInt::from_u64(masked, bw(bits as usize)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an i1 constant in the current block and return its value.
    pub(super) fn bool_constant(&mut self, ctx: &mut Context, value: bool) -> Value {
        let i1 = IntegerType::get(ctx, 1, Signedness::Signless);
        let attr = IntegerAttr::new(i1, APInt::from_u64(u64::from(value), bw(1)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Append `op` to the current block, stamping the span of `span_reg`
    /// (usually the instruction's dest) as its location when available.
    pub(super) fn append(&mut self, ctx: &mut Context, op: Ptr<Operation>, span_reg: Option<Reg>) {
        let block = self.current.expect("lowering is inside a block");
        op.insert_at_back(block, ctx);
        if let Some(reg) = span_reg
            && let Some((span, _)) = self.func.spans.0.get(&reg.0)
            && let Some(location) = self.locator.locate(span)
        {
            op.deref_mut(ctx).set_loc(location);
        }
    }

    /// Append a value-producing op and record its result for `dest`.
    pub(super) fn define(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        op: Ptr<Operation>,
        result: Value,
    ) -> Result<(), PlironError> {
        self.append(ctx, op, Some(dest));
        self.reg_values.insert(dest.0, result);
        Ok(())
    }

    /// The SSA value of `reg`, materializing a pending literal at `expected`
    /// on demand (instructions may consume literal-typed operands directly,
    /// e.g. shift amounts; the VM materializes them at their consumer's kind).
    pub(super) fn reg_value(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        expected: ScalarTy,
    ) -> Result<Value, PlironError> {
        if let Some(value) = self.reg_values.get(&reg.0) {
            return Ok(*value);
        }
        if let Some(literal) = self.pending_literals.get(&reg.0).cloned() {
            let value = self.materialize_pending(ctx, &literal, expected, reg)?;
            self.reg_values.insert(reg.0, value);
            return Ok(value);
        }
        let construct = if self.erased.contains(&reg.0) {
            format!("read of erased analysis register %r{}", reg.0)
        } else if self.str_consts.contains_key(&reg.0) {
            format!(
                "StringLiteral value in register %r{} outside the supported constant contexts",
                reg.0
            )
        } else {
            format!("read of undefined register %r{}", reg.0)
        };
        Err(self.unsupported(construct, self.reg_span(reg)))
    }

    /// Fold a pending literal into one constant of the target scalar type
    /// with the VM's exact semantics (`runtime::materialize_literal`):
    /// integers wrap modulo 2^64, floats convert exactly.
    pub(super) fn materialize_pending(
        &mut self,
        ctx: &mut Context,
        literal: &PendingLiteral,
        expected: ScalarTy,
        span_reg: Reg,
    ) -> Result<Value, PlironError> {
        match (literal, expected) {
            (PendingLiteral::Int(literal), ScalarTy::Int) => {
                let value = literal.wrapping_signed(64).ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "Int (i64)",
                        span_reg,
                    )
                })?;
                Ok(self.int_constant(ctx, value))
            }
            (PendingLiteral::Int(literal), ScalarTy::UInt) => {
                let value = literal.wrapping_unsigned(64).ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "UInt (u64)",
                        span_reg,
                    )
                })?;
                Ok(self.uint_constant(ctx, value))
            }
            (PendingLiteral::Int(literal), ScalarTy::Float64) => {
                let value = literal.to_f64().ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "Float64 (f64)",
                        span_reg,
                    )
                })?;
                Ok(self.float_constant(ctx, value))
            }
            (PendingLiteral::Float(literal), ScalarTy::Float64) => {
                let value = literal.to_f64().ok_or_else(|| {
                    self.literal_out_of_range(literal.to_string(), "Float64 (f64)", span_reg)
                })?;
                Ok(self.float_constant(ctx, value))
            }
            // Sized lanes materialize with the VM's exact conversions:
            // integers wrap at the lane width, `Float32` rounds correctly
            // from the exact literal (never through an f64 intermediate).
            (PendingLiteral::Int(literal), ScalarTy::Sized(Dtype::Float32)) => {
                let value = FloatLiteral::from_int(literal).to_f32().ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        "Float32 (f32)",
                        span_reg,
                    )
                })?;
                Ok(self.f32_constant(ctx, value))
            }
            (PendingLiteral::Float(literal), ScalarTy::Sized(Dtype::Float32)) => {
                let value = literal.to_f32().ok_or_else(|| {
                    self.literal_out_of_range(literal.to_string(), "Float32 (f32)", span_reg)
                })?;
                Ok(self.f32_constant(ctx, value))
            }
            (PendingLiteral::Int(literal), ScalarTy::Sized(dtype)) => {
                let (bits, signed) =
                    crate::runtime::integer_dtype_bits(dtype).ok_or_else(|| {
                        self.unsupported(
                            format!("literal materialization to `{}`", expected.name()),
                            self.reg_span(span_reg),
                        )
                    })?;
                let value = if signed {
                    literal.wrapping_signed(bits).map(|value| value as u64)
                } else {
                    literal.wrapping_unsigned(bits)
                }
                .ok_or_else(|| {
                    self.literal_out_of_range(
                        literal.as_bigint().to_string(),
                        ScalarTy::Sized(dtype).name(),
                        span_reg,
                    )
                })?;
                Ok(self.sized_int_constant(ctx, dtype, value))
            }
            (PendingLiteral::Float(literal), other) => Err(self.unsupported(
                format!(
                    "float literal `{literal}` materialization to `{}`",
                    other.name()
                ),
                self.reg_span(span_reg),
            )),
            (PendingLiteral::Int(literal), ScalarTy::Bool | ScalarTy::Ptr) => Err(self
                .unsupported(
                    format!(
                        "integer literal `{}` used as {}",
                        literal.as_bigint(),
                        expected.name()
                    ),
                    self.reg_span(span_reg),
                )),
        }
    }

    pub(super) fn literal_out_of_range(
        &self,
        literal: String,
        target: &'static str,
        span_reg: Reg,
    ) -> PlironError {
        PlironError {
            function: Some(self.name.to_string()),
            kind: PlironErrorKind::LiteralOutOfRange { literal, target },
            location: self.reg_span(span_reg),
        }
    }

    /// Both operands' shared scalar kind: the first concrete operand type
    /// wins (the checker rejects mixing concrete kinds); two literal operands
    /// promote to Float64 when either is a float literal, else Int.
    pub(super) fn binop_operand_ty(&self, a: Reg, b: Reg) -> Result<ScalarTy, PlironError> {
        if let Some(ty) = self.concrete_scalar_ty(a)? {
            return Ok(ty);
        }
        if let Some(ty) = self.concrete_scalar_ty(b)? {
            return Ok(ty);
        }
        let float = matches!(self.func.reg_types.get(&a.0), Some(Ty::FloatLiteral))
            || matches!(self.func.reg_types.get(&b.0), Some(Ty::FloatLiteral));
        Ok(if float {
            ScalarTy::Float64
        } else {
            ScalarTy::Int
        })
    }

    /// `reg`'s scalar type, or `None` when it holds an unmaterialized literal
    /// (whose kind the consumer decides).
    pub(super) fn concrete_scalar_ty(&self, reg: Reg) -> Result<Option<ScalarTy>, PlironError> {
        let Some(ty) = self.func.reg_types.get(&reg.0) else {
            return Err(self.unsupported(format!("untyped register %r{}", reg.0), None));
        };
        if matches!(ty, Ty::IntLiteral | Ty::FloatLiteral) {
            return Ok(None);
        }
        scalar_type(self.name, ty, self.reg_span(reg)).map(Some)
    }

    pub(super) fn var_lower_ty(&self, var: u32) -> Result<LowerTy, PlironError> {
        let Some(ty) = self.func.var_tys.get(&var) else {
            return Err(self.unsupported(
                format!(
                    "untyped variable `{}`",
                    self.func
                        .var_names
                        .get(var as usize)
                        .map(String::as_str)
                        .unwrap_or("?")
                ),
                None,
            ));
        };
        lower_ty(self.name, ty, &self.layout, None)
    }

    pub(super) fn block(&self, id: MirBlockId) -> Result<Ptr<BasicBlock>, PlironError> {
        self.blocks
            .get(id)
            .copied()
            .ok_or_else(|| self.unsupported(format!("branch to missing block bb{id}"), None))
    }

    pub(super) fn reg_span(&self, reg: Reg) -> Option<SourceSpan> {
        self.func.spans.0.get(&reg.0).map(|(span, _)| span.clone())
    }

    pub(super) fn unsupported(
        &self,
        construct: String,
        location: Option<SourceSpan>,
    ) -> PlironError {
        PlironError {
            function: Some(self.name.to_string()),
            kind: PlironErrorKind::Unsupported { construct },
            location,
        }
    }

    pub(super) fn unsupported_reg(&self, construct: String, dest: Reg) -> PlironError {
        self.unsupported(construct, self.reg_span(dest))
    }
}
