//! `external_call` lowering: an allowlisted libc callee
//! (`mojito_types::ffi::CALLEES`) becomes a real C call against an
//! on-demand `llvm.func` declaration. Integer operands and results resize
//! between the checked scalar width and the C width; pointer operands are
//! the checked `Pointer` value or the single pointer field of a
//! `CStringSlice` view.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::ffi::{self, CType, FfiCallee};

impl FnLowering<'_> {
    /// Lower `external_call["callee", Ret, ...](args)`: the callee name is
    /// the first parameter argument's string constant; the result type is
    /// the destination register's checked type.
    pub(super) fn lower_external_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        param_arg_regs: &[MirParamArg],
    ) -> Result<(), PlironError> {
        let callee_reg = param_arg_regs.first().and_then(|argument| argument.value);
        let name = callee_reg
            .and_then(|reg| self.str_consts.get(&reg.0))
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let Some(row) = name.as_deref().and_then(ffi::callee) else {
            return Err(
                self.unsupported_reg("external_call without an allowlisted callee".into(), dest)
            );
        };
        let (min, max) = row.arity();
        if args.len() < min || args.len() > max {
            return Err(self.unsupported_reg(
                format!("external_call[\"{}\"] argument count", row.name),
                dest,
            ));
        }
        let mut operands = Vec::with_capacity(args.len());
        for (index, reg) in args.iter().enumerate() {
            let kind = row.param(index).expect("arity checked above");
            let operand = if kind.is_integer() {
                self.c_integer_operand(ctx, *reg, kind, dest)?
            } else {
                self.c_pointer_operand(ctx, *reg, dest)?
            };
            operands.push(operand);
        }
        let func_ty = self.shared.ensure_extern(ctx, row);
        let identifier: Identifier = row
            .name
            .try_into()
            .expect("libc callee names are identifier-safe");
        let call = CallOp::new(ctx, CallOpCallable::Direct(identifier), func_ty, operands);
        if row.ret == CType::Void {
            self.append(ctx, call.get_operation(), Some(dest));
            self.erased.insert(dest.0);
            return Ok(());
        }
        self.append(ctx, call.get_operation(), Some(dest));
        let result = call.get_result(ctx);
        let value = if row.ret.is_integer() {
            self.c_integer_result(ctx, result, row.ret, dest)?
        } else {
            result
        };
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// A checked integer scalar (or literal) resized to the C kind's width.
    fn c_integer_operand(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        kind: CType,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let to = kind.bits();
        let (value, from) = match self.concrete_scalar_ty(reg)? {
            None => (self.reg_value(ctx, reg, ScalarTy::Int)?, (64, true)),
            Some(ScalarTy::Int) => (self.reg_value(ctx, reg, ScalarTy::Int)?, (64, true)),
            Some(ScalarTy::UInt) => (self.reg_value(ctx, reg, ScalarTy::UInt)?, (64, false)),
            Some(ScalarTy::Bool) => (self.reg_value(ctx, reg, ScalarTy::Bool)?, (1, false)),
            Some(ScalarTy::Sized(dtype)) => {
                let Some(from) = mojito_vm::runtime::integer_dtype_bits(dtype) else {
                    return Err(self
                        .unsupported_reg("floating-point argument to external_call".into(), dest));
                };
                (self.reg_value(ctx, reg, ScalarTy::Sized(dtype))?, from)
            }
            Some(ScalarTy::Ptr | ScalarTy::Float64) => {
                return Err(self.unsupported_reg(
                    "non-integer argument at an integer external_call parameter".into(),
                    dest,
                ));
            }
        };
        Ok(self.resize_int(ctx, value, from, to, dest))
    }

    /// A checked `Pointer` value, or the pointer field of a one-pointer view
    /// struct (`CStringSlice`) loaded from its storage.
    fn c_pointer_operand(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let ty = self.func.reg_types.get(&reg.0).cloned();
        match ty {
            Some(Ty::Pointer { .. }) => self.reg_value(ctx, reg, ScalarTy::Ptr),
            Some(ty @ Ty::Struct(..)) => {
                let Ty::Struct(name, _) = &ty else {
                    unreachable!("matched above");
                };
                let field = self.struct_decls.get(name.as_str()).and_then(|decl| {
                    decl.fields
                        .iter()
                        .find(|(_, field_ty)| matches!(field_ty, Ty::Pointer { .. }))
                        .map(|(field, _)| field.clone())
                });
                let Some(field) = field else {
                    return Err(self.unsupported_reg(
                        format!("`{ty}` is not a pointer-carrying view for external_call"),
                        dest,
                    ));
                };
                let storage = self.reg_ptr(ctx, reg)?;
                let (offset, _) = self.field_offset(&ty, &field, dest)?;
                let address = self.gep_byte(ctx, storage, offset, dest);
                let ptr_handle = ScalarTy::Ptr.handle(ctx);
                let load = LoadOp::new(ctx, address, ptr_handle);
                self.append(ctx, load.get_operation(), Some(dest));
                Ok(load.get_result(ctx))
            }
            _ => Err(self.unsupported_reg(
                "non-pointer argument at a pointer external_call parameter".into(),
                dest,
            )),
        }
    }

    /// The C integer result resized to the destination register's checked
    /// scalar width.
    fn c_integer_result(
        &mut self,
        ctx: &mut Context,
        result: Value,
        kind: CType,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let signed = matches!(kind, CType::Int | CType::SSizeT | CType::OffT);
        let from = (kind.bits(), signed);
        let to = match self.func.reg_types.get(&dest.0) {
            Some(Ty::Int | Ty::UInt) => 64,
            Some(Ty::Simd { dtype, width: 1 }) => {
                match mojito_vm::runtime::integer_dtype_bits(*dtype) {
                    Some((bits, _)) => bits,
                    None => {
                        return Err(self
                            .unsupported_reg("floating-point external_call result".into(), dest));
                    }
                }
            }
            _ => {
                return Err(self.unsupported_reg(
                    "external_call result without an integer destination".into(),
                    dest,
                ));
            }
        };
        Ok(self.resize_int(ctx, result, from, to, dest))
    }
}

/// The LLVM type of a C parameter/return kind.
pub(super) fn c_type(ctx: &Context, kind: CType) -> TypeHandle {
    match kind {
        CType::Int | CType::UInt => IntegerType::get(ctx, 32, Signedness::Signless).into(),
        CType::SizeT | CType::SSizeT | CType::OffT => {
            IntegerType::get(ctx, 64, Signedness::Signless).into()
        }
        CType::ConstCharPtr | CType::CharPtr | CType::VoidPtr | CType::IntPtr => {
            PointerType::get(ctx, 0).into()
        }
        CType::Void => VoidType::get(ctx).to_handle(),
    }
}

/// Keep the row type nameable from the module-environment declaration
/// helper without re-importing the table there.
pub(super) type LibcCallee = FfiCallee;
