//! Scalar builtin calls: `len`, `abs`, `min`/`max`, rounding, `divmod`,
//! `input`, and pointer methods.

use super::*;

impl<'a> FnLowering<'a> {
    /// `UnsafePointer.unsafe_dangling` / `Pointer.unsafe_dangling`: the null
    /// pointer (the VM's `allocation: 0` sentinel). Dereference and free
    /// misuse are off-gate runtime errors; the VM rejects `free` of a
    /// dangling pointer while `mjrt_free(null)` is a no-op — a recorded
    /// divergence.
    pub(super) fn lower_dangling_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let null = ZeroOp::new(ctx, ptr_ty);
        self.define(ctx, dest, null.get_operation(), null.get_result(ctx))
    }

    /// `len(x)` over the non-nominal shapes (the VM's `call_named` arm):
    /// string byte length, or the static element count of a pack. Nominal
    /// receivers were rewritten to `__len__` method calls during
    /// monomorphization.
    pub(super) fn lower_len_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 1 {
            return Err(self.unsupported_reg("`len` call contract".into(), dest));
        }
        let arg = args[0];
        if let Some(bytes) = self.str_consts.get(&arg.0) {
            let length = self.int_constant(ctx, bytes.len() as i64);
            self.reg_values.insert(dest.0, length);
            return Ok(());
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            self.reg_values.insert(dest.0, descriptor.len);
            return Ok(());
        }
        match self.func.reg_types.get(&arg.0).cloned() {
            Some(Ty::StringLiteral) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                let (_, len) = self.string_parts(ctx, ptr, dest);
                self.reg_values.insert(dest.0, len);
                Ok(())
            }
            Some(Ty::Tuple(elements) | Ty::RuntimePack(elements)) => {
                let length = self.int_constant(ctx, elements.len() as i64);
                self.reg_values.insert(dest.0, length);
                Ok(())
            }
            other => Err(self.unsupported_reg(
                format!(
                    "`len` over `{}`",
                    other.map_or_else(|| "an untyped value".to_string(), |ty| ty.to_string())
                ),
                dest,
            )),
        }
    }

    /// `abs(x)` — the VM's `builtin_abs`: `wrapping_abs` on Int (including
    /// `abs(i64::MIN) == i64::MIN`), identity on UInt, `fabs` on Float64.
    pub(super) fn lower_abs_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 1 {
            return Err(self.unsupported_reg("`abs` call contract".into(), dest));
        }
        let arg = args[0];
        match self.func.reg_types.get(&arg.0).cloned() {
            Some(Ty::Int | Ty::IntLiteral) => {
                let value = self.reg_value(ctx, arg, ScalarTy::Int)?;
                let zero = self.int_constant(ctx, 0);
                let negated = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.append(ctx, negated.get_operation(), Some(dest));
                let negative = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, value, zero);
                self.append(ctx, negative.get_operation(), Some(dest));
                let select = SelectOp::new(
                    ctx,
                    negative.get_result(ctx),
                    negated.get_result(ctx),
                    value,
                );
                self.define(ctx, dest, select.get_operation(), select.get_result(ctx))
            }
            Some(Ty::UInt) => {
                let value = self.reg_value(ctx, arg, ScalarTy::UInt)?;
                self.reg_values.insert(dest.0, value);
                Ok(())
            }
            Some(Ty::Float64 | Ty::FloatLiteral) => {
                let value = self.reg_value(ctx, arg, ScalarTy::Float64)?;
                let result = self.float_unary(ctx, "llvm.fabs.f64", value, dest);
                self.reg_values.insert(dest.0, result);
                Ok(())
            }
            other => Err(self.unsupported_reg(
                format!(
                    "`abs` over `{}`",
                    other.map_or_else(|| "an untyped value".to_string(), |ty| ty.to_string())
                ),
                dest,
            )),
        }
    }

    /// `min(a, b)` / `max(a, b)` — the VM's `builtin_min_max`: promote to the
    /// higher numeric kind (Int < UInt < Float64) and pick by `x <= y`
    /// (left-biased on ties; NaN loses either side, matching the VM's
    /// ordered `<=`). Post-mono both operand types are concrete, so the
    /// promotion is static. Mixed concrete Int/UInt rejects: the VM compares
    /// those exactly, which one unsigned compare cannot reproduce.
    pub(super) fn lower_min_max_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        is_min: bool,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 2 {
            return Err(self.unsupported_reg("`min`/`max` call contract".into(), dest));
        }
        let rank = |ty: &Ty| match ty {
            Ty::Int | Ty::IntLiteral => Some(0),
            Ty::UInt => Some(1),
            Ty::Float64 | Ty::FloatLiteral => Some(2),
            _ => None,
        };
        let ty_of = |this: &Self, reg: Reg| this.func.reg_types.get(&reg.0).cloned();
        let (Some(lhs_ty), Some(rhs_ty)) = (ty_of(self, args[0]), ty_of(self, args[1])) else {
            return Err(self.unsupported_reg("`min`/`max` over untyped operands".into(), dest));
        };
        let (Some(lhs_rank), Some(rhs_rank)) = (rank(&lhs_ty), rank(&rhs_ty)) else {
            return Err(
                self.unsupported_reg(format!("`min`/`max` over `{lhs_ty}` and `{rhs_ty}`"), dest)
            );
        };
        let common = lhs_rank.max(rhs_rank);
        if common == 1 && (lhs_ty == Ty::Int || rhs_ty == Ty::Int) {
            return Err(
                self.unsupported_reg("`min`/`max` over mixed Int and UInt operands".into(), dest)
            );
        }
        let promote = |this: &mut Self, ctx: &mut Context, reg: Reg, ty: &Ty| match (common, ty) {
            (2, Ty::Int) => {
                let value = this.reg_value(ctx, reg, ScalarTy::Int)?;
                Ok(this.int_to_f64(ctx, value, dest))
            }
            (2, Ty::UInt) => {
                let value = this.reg_value(ctx, reg, ScalarTy::UInt)?;
                Ok(this.uint_to_f64(ctx, value, dest))
            }
            (2, _) => this.reg_value(ctx, reg, ScalarTy::Float64),
            (1, _) => this.reg_value(ctx, reg, ScalarTy::UInt),
            _ => this.reg_value(ctx, reg, ScalarTy::Int),
        };
        let x = promote(self, ctx, args[0], &lhs_ty)?;
        let y = promote(self, ctx, args[1], &rhs_ty)?;
        let le = match common {
            2 => {
                let cmp = self.fcmp(ctx, FCmpPredicateAttr::OLE, x, y);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
            1 => {
                let cmp = ICmpOp::new(ctx, ICmpPredicateAttr::ULE, x, y);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
            _ => {
                let cmp = ICmpOp::new(ctx, ICmpPredicateAttr::SLE, x, y);
                self.append(ctx, cmp.get_operation(), Some(dest));
                cmp.get_result(ctx)
            }
        };
        let (on_le, on_gt) = if is_min { (x, y) } else { (y, x) };
        let select = SelectOp::new(ctx, le, on_le, on_gt);
        self.define(ctx, dest, select.get_operation(), select.get_result(ctx))
    }

    /// `round(x)` — the VM's `builtin_round`: nearest `Float64`, ties away
    /// from zero (`llvm.round.f64` == `f64::round`); integers convert first
    /// and the result is always `Float64`.
    pub(super) fn lower_round_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 1 {
            return Err(self.unsupported_reg("`round` call contract".into(), dest));
        }
        let arg = args[0];
        let value = match self.func.reg_types.get(&arg.0).cloned() {
            Some(Ty::Int) => {
                let value = self.reg_value(ctx, arg, ScalarTy::Int)?;
                self.int_to_f64(ctx, value, dest)
            }
            Some(Ty::UInt) => {
                let value = self.reg_value(ctx, arg, ScalarTy::UInt)?;
                self.uint_to_f64(ctx, value, dest)
            }
            Some(Ty::Float64 | Ty::FloatLiteral | Ty::IntLiteral) => {
                self.reg_value(ctx, arg, ScalarTy::Float64)?
            }
            other => {
                return Err(self.unsupported_reg(
                    format!(
                        "`round` over `{}`",
                        other.map_or_else(|| "an untyped value".to_string(), |ty| ty.to_string())
                    ),
                    dest,
                ));
            }
        };
        let result = self.float_unary(ctx, "llvm.round.f64", value, dest);
        self.reg_values.insert(dest.0, result);
        Ok(())
    }

    /// `divmod(a, b)` — the VM's `builtin_divmod`: `(a // b, a % b)` with
    /// the operators' exact flooring rules and zero traps, stored into the
    /// checker-selected nominal `Tuple` (whose single `storage` field is the
    /// private pack).
    pub(super) fn lower_divmod_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 2 {
            return Err(self.unsupported_reg("`divmod` call contract".into(), dest));
        }
        let Some(dest_ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped `divmod` result".into(), dest));
        };
        // The result pack: either the nominal Tuple's single `storage` field
        // or (defensively) a bare private pack.
        let elements = match &dest_ty {
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements.clone(),
            Ty::Struct(name, _) => match self.struct_decls.get(name.as_str()) {
                Some(decl)
                    if decl.fields.len() == 1
                        && matches!(&decl.fields[0].1, Ty::Tuple(_) | Ty::RuntimePack(_)) =>
                {
                    let (Ty::Tuple(elements) | Ty::RuntimePack(elements)) = &decl.fields[0].1
                    else {
                        unreachable!("guard matched a pack field");
                    };
                    elements.clone()
                }
                _ => {
                    return Err(
                        self.unsupported_reg(format!("`divmod` result shape `{dest_ty}`"), dest)
                    );
                }
            },
            _ => {
                return Err(
                    self.unsupported_reg(format!("`divmod` result shape `{dest_ty}`"), dest)
                );
            }
        };
        let [element, _] = elements.as_slice() else {
            return Err(self.unsupported_reg(format!("`divmod` result shape `{dest_ty}`"), dest));
        };
        let element = element.clone();
        let (quotient, remainder) = match &element {
            Ty::Int | Ty::IntLiteral => {
                let lhs = self.reg_value(ctx, args[0], ScalarTy::Int)?;
                let rhs = self.reg_value(ctx, args[1], ScalarTy::Int)?;
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let rhs = self.sanitized_divisor(ctx, dest, lhs, rhs)?;
                let quotient = self.floor_div_value(ctx, dest, lhs, rhs)?;
                let remainder = self.floor_mod_value(ctx, dest, lhs, rhs)?;
                (quotient, remainder)
            }
            Ty::UInt => {
                let lhs = self.reg_value(ctx, args[0], ScalarTy::UInt)?;
                let rhs = self.reg_value(ctx, args[1], ScalarTy::UInt)?;
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                let div = UDivOp::new(ctx, lhs, rhs);
                self.append(ctx, div.get_operation(), Some(dest));
                let rem = URemOp::new(ctx, lhs, rhs);
                self.append(ctx, rem.get_operation(), Some(dest));
                (div.get_result(ctx), rem.get_result(ctx))
            }
            Ty::Float64 | Ty::FloatLiteral => {
                let lhs = self.reg_value(ctx, args[0], ScalarTy::Float64)?;
                let rhs = self.reg_value(ctx, args[1], ScalarTy::Float64)?;
                let flags = FastmathFlagsAttr::default;
                let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, flags());
                self.append(ctx, div.get_operation(), Some(dest));
                let floored = self.float_floor(ctx, div.get_result(ctx), dest);
                let scaled = FMulOp::new_with_fast_math_flags(ctx, rhs, floored, flags());
                self.append(ctx, scaled.get_operation(), Some(dest));
                let rem =
                    FSubOp::new_with_fast_math_flags(ctx, lhs, scaled.get_result(ctx), flags());
                self.append(ctx, rem.get_operation(), Some(dest));
                (floored, rem.get_result(ctx))
            }
            other => {
                return Err(self.unsupported_reg(format!("`divmod` over `{other}` operands"), dest));
            }
        };
        let layout = self
            .layout
            .layout_of(&dest_ty)
            .map_err(|error| self.unsupported_reg(format!("`divmod` result ({error})"), dest))?;
        let inner = self.struct_layout_of(&elements, dest)?;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for (value, offset) in [(quotient, inner.offsets[0]), (remainder, inner.offsets[1])] {
            let address = if offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, offset, dest)
            };
            let store = StoreOp::new(ctx, value, address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `input(prompt)` — the VM's `builtin_input`: write the prompt bytes
    /// (no newline; `mjrt_write_stdout` flushes per call, so the prompt lands
    /// before the read even when piped), then `mjrt_read_line` fills the
    /// nominal String result. The caller owns the line buffer under the
    /// existing String release rule.
    pub(super) fn lower_input_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !kwargs.is_empty() || args.len() != 1 {
            return Err(self.unsupported_reg("`input` call contract".into(), dest));
        }
        // The checker types the call result `StringLiteral` (a separate
        // constructor conversion wraps it into the nominal String when
        // needed). The 24-byte `MjString` the runtime fills starts with the
        // same `{data, len}` words an `MjStrDesc` reads, so one storage
        // shape serves either destination type.
        let dest_ty = self.func.reg_types.get(&dest.0).cloned();
        let Some(dest_ty) = dest_ty.filter(|ty| {
            matches!(ty, Ty::StringLiteral)
                || matches!(ty, Ty::Struct(name, _)
                    if crate::symbol::is_stdlib_string_struct(name))
        }) else {
            return Err(self.unsupported_reg("`input` without a String result".into(), dest));
        };
        if !self.try_write_string_bytes(ctx, args[0], dest)? {
            return Err(self.unsupported_reg("`input` prompt shape".into(), dest));
        }
        let layout = self.layout.mj_string();
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        let read_ty = self.shared.ensure_rt(ctx, "mjrt_read_line");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_read_line".try_into().expect("valid identifier")),
            read_ty,
            vec![storage],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        // The line buffer is owned: a StringLiteral result registers the
        // owned runtime descriptor (the release rule frees StringLiteral
        // temporaries through `str_runtime`, exactly like `String(x)`
        // stringify); a nominal String result releases through its storage.
        if matches!(dest_ty, Ty::StringLiteral) {
            let (data, len) = self.string_parts(ctx, storage, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
        }
        self.mark_owned_temp(dest, dest_ty)
    }

    /// Pointer-receiver method intrinsics — the VM's `Value::Pointer` method
    /// dispatch: `free`/`unsafe_free` release through the runtime's size-less
    /// free. Everything else stays unsupported.
    pub(super) fn lower_pointer_method(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        method: &str,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        match method {
            "free" | "unsafe_free" if args.is_empty() => {
                let ptr = self.reg_value(ctx, recv, ScalarTy::Ptr)?;
                let free_ty = self.shared.ensure_rt(ctx, "mjrt_free");
                let call = CallOp::new(
                    ctx,
                    CallOpCallable::Direct("mjrt_free".try_into().expect("valid identifier")),
                    free_ty,
                    vec![ptr],
                );
                self.append(ctx, call.get_operation(), Some(dest));
                self.erased.insert(dest.0);
                Ok(())
            }
            other => Err(self.unsupported_reg(format!("Pointer method `{other}`"), dest)),
        }
    }

    /// `__floor__`/`__ceil__`/`__trunc__` on a scalar receiver — the VM's
    /// `builtin_round_dir`: integers are already whole (identity), Float64
    /// rounds toward the requested direction.
    pub(super) fn lower_round_dir(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        recv_ty: &Ty,
        method: &str,
    ) -> Result<(), PlironError> {
        match recv_ty {
            Ty::Int | Ty::UInt => {
                let scalar = if matches!(recv_ty, Ty::UInt) {
                    ScalarTy::UInt
                } else {
                    ScalarTy::Int
                };
                let value = self.reg_value(ctx, recv, scalar)?;
                self.reg_values.insert(dest.0, value);
                Ok(())
            }
            _ => {
                let intrinsic = match method {
                    "__floor__" => "llvm.floor.f64",
                    "__ceil__" => "llvm.ceil.f64",
                    _ => "llvm.trunc.f64",
                };
                let value = self.reg_value(ctx, recv, ScalarTy::Float64)?;
                let result = self.float_unary(ctx, intrinsic, value, dest);
                self.reg_values.insert(dest.0, result);
                Ok(())
            }
        }
    }

    /// `__ceildiv__` on a scalar receiver — the VM's `builtin_ceildiv`:
    /// ceiling division preserving the operand type. Int is the negated
    /// flooring division of the negated numerator (with the shared zero trap
    /// and `i64::MIN` divisor sanitizing; the VM's non-wrapping negate would
    /// panic on `-i64::MIN` — an unexercised recorded divergence, native
    /// wraps). UInt adds one when the remainder is nonzero; Float64 is
    /// `ceil(a / b)` with no trap.
    pub(super) fn lower_ceildiv(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        denominator: Reg,
        recv_ty: &Ty,
    ) -> Result<(), PlironError> {
        match recv_ty {
            Ty::Int => {
                let numerator = self.reg_value(ctx, recv, ScalarTy::Int)?;
                let divisor = self.reg_value(ctx, denominator, ScalarTy::Int)?;
                self.emit_div_zero_guard(ctx, divisor, dest)?;
                let zero = self.int_constant(ctx, 0);
                let negated =
                    SubOp::new_with_overflow_flag(ctx, zero, numerator, no_overflow_flags());
                self.append(ctx, negated.get_operation(), Some(dest));
                let divisor =
                    self.sanitized_divisor(ctx, dest, negated.get_result(ctx), divisor)?;
                let floored = self.floor_div_value(ctx, dest, negated.get_result(ctx), divisor)?;
                let result = SubOp::new_with_overflow_flag(ctx, zero, floored, no_overflow_flags());
                self.define(ctx, dest, result.get_operation(), result.get_result(ctx))
            }
            Ty::UInt => {
                let numerator = self.reg_value(ctx, recv, ScalarTy::UInt)?;
                let divisor = self.reg_value(ctx, denominator, ScalarTy::UInt)?;
                self.emit_div_zero_guard(ctx, divisor, dest)?;
                let quotient = UDivOp::new(ctx, numerator, divisor);
                self.append(ctx, quotient.get_operation(), Some(dest));
                let remainder = URemOp::new(ctx, numerator, divisor);
                self.append(ctx, remainder.get_operation(), Some(dest));
                let zero = self.int_constant(ctx, 0);
                let inexact =
                    ICmpOp::new(ctx, ICmpPredicateAttr::NE, remainder.get_result(ctx), zero);
                self.append(ctx, inexact.get_operation(), Some(dest));
                let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let carry = ZExtOp::new_with_nneg(ctx, inexact.get_result(ctx), i64_ty, false);
                self.append(ctx, carry.get_operation(), Some(dest));
                let result = AddOp::new_with_overflow_flag(
                    ctx,
                    quotient.get_result(ctx),
                    carry.get_result(ctx),
                    no_overflow_flags(),
                );
                self.define(ctx, dest, result.get_operation(), result.get_result(ctx))
            }
            _ => {
                let numerator = self.reg_value(ctx, recv, ScalarTy::Float64)?;
                let divisor = self.reg_value(ctx, denominator, ScalarTy::Float64)?;
                let div = FDivOp::new_with_fast_math_flags(
                    ctx,
                    numerator,
                    divisor,
                    FastmathFlagsAttr::default(),
                );
                self.append(ctx, div.get_operation(), Some(dest));
                let result = self.float_unary(ctx, "llvm.ceil.f64", div.get_result(ctx), dest);
                self.reg_values.insert(dest.0, result);
                Ok(())
            }
        }
    }
}
