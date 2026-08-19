//! Per-function lowering of scalar MIR to pliron's LLVM dialect.
//!
//! Registers map to SSA values inside their block; cross-block dataflow
//! arrives through variable slots, which lower to entry-block allocas with
//! `load`/`store` at each `UseVar`/`DefVar` (pliron's mem2reg pass rebuilds
//! SSA afterwards). Integer literals fold through their `MaterializeLiteral`
//! into single constants; `FloorDiv`/`Mod` expand to branch-free select
//! sequences matching the VM's `floor_div`/`floor_mod`, and shift amounts are
//! masked to the VM's `wrapping_shl`/`wrapping_shr` semantics. Everything
//! outside the scalar subset produces a contextual [`PlironError`].

use std::collections::{HashMap, HashSet};

use pliron::builtin::attributes::IntegerAttr;
use pliron::builtin::op_interfaces::{
    CallOpCallable, OneResultInterface, SingleBlockRegionInterface,
};
use pliron::builtin::ops::ModuleOp;
use pliron::builtin::types::{IntegerType, Signedness};
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::location::{Located, Location};
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::r#type::{TypeHandle, TypedHandle};
use pliron::utils::apint::{APInt, bw};
use pliron::value::Value;
use pliron_llvm::attributes::{ICmpPredicateAttr, IntegerOverflowFlagsAttr};
use pliron_llvm::op_interfaces::{BinArithOp, IntBinArithOpWithOverflowFlag};
use pliron_llvm::ops::{
    AddOp, AllocaOp, AndOp, BrOp, CallOp, CondBrOp, ConstantOp, FuncOp, ICmpOp, LoadOp, MulOp,
    OrOp, ReturnOp, SDivOp, SRemOp, SelectOp, ShlOp, StoreOp, SubOp, XorOp,
};
use pliron_llvm::types::{FuncType, VoidType};

use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::ast::{InfixOp, PrefixOp};
use crate::mir::{Const as MirConst, MirBlockId, MirFunction, MirInstr, MirTerm, Reg, UseMode};
use crate::token::SourceSpan;
use crate::types::Ty;

use super::{PlironError, PlironErrorKind, mangle};

/// The callable identity of one reachable function: its mangled symbol and
/// LLVM-dialect function type. Built by [`declare_function`], consumed by
/// call lowering.
pub(super) struct FnSignature {
    pub mangled: String,
    pub func_ty: TypedHandle<FuncType>,
    pub returns_value: bool,
}

/// Convert a function's checked signature and append an empty `llvm.func`
/// shell to `module`.
pub(super) fn declare_function(
    ctx: &mut Context,
    module: ModuleOp,
    name: &str,
    func: &MirFunction,
) -> Result<(FuncOp, FnSignature), PlironError> {
    let ret_ty = func.ret_ty.as_ref().ok_or_else(|| PlironError {
        function: Some(name.to_string()),
        kind: PlironErrorKind::Unsupported {
            construct: "function without a recorded return type".into(),
        },
        location: None,
    })?;
    let (result, returns_value) = match ret_ty {
        Ty::None => (VoidType::get(ctx).to_handle(), false),
        other => (scalar_type(name, other, None)?.handle(ctx), true),
    };
    let mut params = Vec::with_capacity(func.param_types.len());
    for ty in &func.param_types {
        params.push(scalar_type(name, ty, None)?.handle(ctx));
    }
    let func_ty = FuncType::get(ctx, result, params, false);
    let mangled = mangle::mangle(name);
    let identifier: Identifier = mangled
        .as_str()
        .try_into()
        .expect("mangled names are identifier-safe");
    let func_op = FuncOp::new(ctx, identifier, func_ty);
    module.append_operation(ctx, func_op.get_operation(), 0);
    Ok((
        func_op,
        FnSignature {
            mangled,
            func_ty,
            returns_value,
        },
    ))
}

/// Lower `func`'s body into the declared `func_op`.
pub(super) fn lower_body(
    ctx: &mut Context,
    name: &str,
    func: &MirFunction,
    func_op: FuncOp,
    signatures: &HashMap<String, FnSignature>,
    locator: &Locator,
) -> Result<(), PlironError> {
    let mut lowering = FnLowering {
        name,
        func,
        signatures,
        locator,
        reg_values: HashMap::new(),
        pending_literals: HashMap::new(),
        erased: HashSet::new(),
        var_slots: Vec::new(),
        blocks: Vec::new(),
        current: None,
    };
    lowering.run(ctx, func_op)
}

/// Synthesize the executable's C `main`: call each (void, zero-arg) callee in
/// order, then return `0: i32`. Callees are already-mangled native symbols.
pub(super) fn synthesize_exe_wrapper(
    ctx: &mut Context,
    module: ModuleOp,
    callees: &[String],
) -> Result<(), PlironError> {
    let void = VoidType::get(ctx).to_handle();
    let i32_ty: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
    let wrapper_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let wrapper = FuncOp::new(
        ctx,
        "main".try_into().expect("valid identifier"),
        wrapper_ty,
    );
    module.append_operation(ctx, wrapper.get_operation(), 0);
    let entry = wrapper.get_or_create_entry_block(ctx);

    for callee in callees {
        let callee_ty = FuncType::get(ctx, void, vec![], false);
        let identifier: Identifier = callee
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let call = CallOp::new(ctx, CallOpCallable::Direct(identifier), callee_ty, vec![]);
        call.get_operation().insert_at_back(entry, ctx);
    }
    let i32_handle = IntegerType::get(ctx, 32, Signedness::Signless);
    let zero_attr = IntegerAttr::new(i32_handle, APInt::from_u64(0, bw(32)));
    let zero = ConstantOp::new(ctx, Box::new(zero_attr));
    zero.get_operation().insert_at_back(entry, ctx);
    let ret = ReturnOp::new(ctx, Some(zero.get_result(ctx)));
    ret.get_operation().insert_at_back(entry, ctx);
    Ok(())
}

/// Resolves MIR source spans to pliron [`Location`]s against the compilation's
/// registered sources.
pub(super) struct Locator {
    sources: Vec<(String, pliron::location::Source, Vec<usize>)>,
}

impl Locator {
    pub(super) fn new(ctx: &mut Context, sources: &[(String, String)]) -> Locator {
        let sources = sources
            .iter()
            .map(|(name, text)| {
                let source = pliron::location::Source::new_from_file(ctx, name.clone());
                let mut line_starts = vec![0];
                for (offset, byte) in text.bytes().enumerate() {
                    if byte == b'\n' {
                        line_starts.push(offset + 1);
                    }
                }
                (name.clone(), source, line_starts)
            })
            .collect();
        Locator { sources }
    }

    fn locate(&self, span: &SourceSpan) -> Option<Location> {
        let name = span.source.as_deref()?;
        let (_, source, line_starts) = self.sources.iter().find(|(n, _, _)| n == name)?;
        let byte = span.span.0;
        let line = line_starts.partition_point(|start| *start <= byte);
        let column = byte - line_starts[line - 1] + 1;
        Some(Location::SrcPos {
            src: *source,
            pos: pliron::combine::stream::position::SourcePosition {
                line: line as i32,
                column: column as i32,
            },
        })
    }
}

/// The scalar value types the Stage 1 backend lowers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScalarTy {
    Int,
    Bool,
}

impl ScalarTy {
    fn handle(self, ctx: &mut Context) -> TypeHandle {
        match self {
            ScalarTy::Int => IntegerType::get(ctx, 64, Signedness::Signless).into(),
            ScalarTy::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
        }
    }
}

struct FnLowering<'a> {
    name: &'a str,
    func: &'a MirFunction,
    signatures: &'a HashMap<String, FnSignature>,
    locator: &'a Locator,
    /// Materialized SSA value of each register.
    reg_values: HashMap<u32, Value>,
    /// Registers holding a not-yet-materialized integer literal.
    pending_literals: HashMap<u32, BigInt>,
    /// Registers written by semantically erased instructions (analysis
    /// markers, void call results). Reading one is an internal invariant
    /// violation surfaced as a diagnostic, never a silent miscompile.
    erased: HashSet<u32>,
    /// Alloca (pointer) value of each variable slot.
    var_slots: Vec<Value>,
    /// Pliron block for each MIR block id.
    blocks: Vec<Ptr<pliron::basic_block::BasicBlock>>,
    current: Option<Ptr<pliron::basic_block::BasicBlock>>,
}

impl<'a> FnLowering<'a> {
    fn run(&mut self, ctx: &mut Context, func_op: FuncOp) -> Result<(), PlironError> {
        let entry = func_op.get_or_create_entry_block(ctx);
        let region = func_op
            .get_operation()
            .deref(ctx)
            .regions()
            .next()
            .expect("llvm.func has a body region");

        // One pliron block per MIR block (entry stays separate so MIR block 0
        // may have predecessors).
        for _ in 0..self.func.blocks.len() {
            let block = pliron::basic_block::BasicBlock::new(ctx, None, vec![]);
            block.insert_at_back(region, ctx);
            self.blocks.push(block);
        }

        // Entry: one alloca per variable slot, parameter stores, then a jump
        // to MIR block 0.
        self.current = Some(entry);
        let one = self.int_constant(ctx, 1);
        for var in 0..self.func.n_vars {
            let ty = self.var_scalar_ty(var as u32)?;
            let handle = ty.handle(ctx);
            let alloca = AllocaOp::new(ctx, handle, one);
            self.append(ctx, alloca.get_operation(), None);
            self.var_slots.push(alloca.get_result(ctx));
        }
        for param in 0..self.func.n_params {
            let value = entry.deref(ctx).get_argument(param);
            let store = StoreOp::new(ctx, value, self.var_slots[param]);
            self.append(ctx, store.get_operation(), None);
        }
        let jump = BrOp::new(ctx, self.blocks[0], vec![]);
        self.append(ctx, jump.get_operation(), None);

        for (id, block) in self.func.blocks.iter().enumerate() {
            self.current = Some(self.blocks[id]);
            for instr in &block.instrs {
                self.lower_instr(ctx, instr)?;
            }
            self.lower_term(ctx, &block.term)?;
        }
        Ok(())
    }

    fn lower_instr(&mut self, ctx: &mut Context, instr: &MirInstr) -> Result<(), PlironError> {
        match instr {
            MirInstr::Const { dest, k } => self.lower_const(ctx, *dest, k),
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => self.lower_materialize(ctx, *dest, *value, target),
            MirInstr::UnOp { op, dest, a } => self.lower_unop(ctx, *op, *dest, *a),
            MirInstr::BinOp {
                op,
                dest,
                a,
                b,
                resolved,
            } => self.lower_binop(ctx, *op, *dest, *a, *b, resolved.as_deref()),
            MirInstr::UseVar { dest, var, mode } => match mode {
                UseMode::Copy | UseMode::Move => {
                    let ty = self.var_scalar_ty(*var)?.handle(ctx);
                    let load = LoadOp::new(ctx, self.var_slots[*var as usize], ty);
                    self.define(ctx, *dest, load.get_operation(), load.get_result(ctx))
                }
                UseMode::BorrowShared | UseMode::BorrowMut => {
                    Err(self.unsupported_reg(format!("variable use mode `{mode:?}`"), *dest))
                }
            },
            MirInstr::DefVar { var, src, .. } => {
                let value = self.reg_value(ctx, *src)?;
                let store = StoreOp::new(ctx, value, self.var_slots[*var as usize]);
                self.append(ctx, store.get_operation(), None);
                Ok(())
            }
            MirInstr::Call {
                dest,
                func,
                raises,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
            } => {
                if raises.is_some() {
                    return Err(self.unsupported_reg("raising call".into(), *dest));
                }
                if !kwargs.is_empty()
                    || !kwarg_places.is_empty()
                    || arg_places.iter().any(Option::is_some)
                    || !capture_accesses.is_empty()
                    || !param_arg_regs.is_empty()
                {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                let Some(signature) = self.signatures.get(&func.0) else {
                    return Err(self.unsupported_reg(
                        format!("call to unknown or builtin function `{}`", func.0),
                        *dest,
                    ));
                };
                let mut lowered_args = Vec::with_capacity(args.len());
                for arg in args {
                    lowered_args.push(self.reg_value(ctx, *arg)?);
                }
                let callee: Identifier = signature
                    .mangled
                    .as_str()
                    .try_into()
                    .expect("mangled names are identifier-safe");
                let call = CallOp::new(
                    ctx,
                    CallOpCallable::Direct(callee),
                    signature.func_ty,
                    lowered_args,
                );
                if signature.returns_value {
                    self.define(ctx, *dest, call.get_operation(), call.get_result(ctx))
                } else {
                    self.append(ctx, call.get_operation(), Some(*dest));
                    self.erased.insert(dest.0);
                    Ok(())
                }
            }
            // Semantically erased in the scalar subset: scalar drops are
            // no-ops and interior invalidation is pure analysis metadata.
            MirInstr::DropVar { var } => {
                self.var_scalar_ty(*var)?;
                Ok(())
            }
            MirInstr::InvalidateInteriors { marker, .. } => {
                self.erased.insert(marker.0);
                Ok(())
            }
            MirInstr::EstablishLoans { marker, .. } => {
                self.erased.insert(marker.0);
                Ok(())
            }
            MirInstr::KeepAlive { .. } => Ok(()),
            // Everything below is outside the Stage 1 scalar subset. Every
            // variant is named so that new instructions force a decision here.
            MirInstr::MakeRef { dest, .. }
            | MirInstr::ReadRef { dest, .. }
            | MirInstr::CopyValue { dest, .. }
            | MirInstr::MakeClosure { dest, .. }
            | MirInstr::MovePlace { dest, .. }
            | MirInstr::CallIndirect { dest, .. }
            | MirInstr::MethodCall { dest, .. }
            | MirInstr::GetField { dest, .. }
            | MirInstr::Index { dest, .. }
            | MirInstr::Slice { dest, .. }
            | MirInstr::MultiIndex { dest, .. }
            | MirInstr::MakeTuple { dest, .. }
            | MirInstr::MakeVariant { dest, .. }
            | MirInstr::VariantIs { dest, .. }
            | MirInstr::VariantGet { dest, .. }
            | MirInstr::VariantTake { dest, .. }
            | MirInstr::VariantReplace { dest, .. }
            | MirInstr::MakeSimd { dest, .. }
            | MirInstr::SimdCast { dest, .. }
            | MirInstr::SimdShuffle { dest, .. }
            | MirInstr::PointerStorageTake { dest, .. }
            | MirInstr::UninitStorage { dest, .. }
            | MirInstr::UninitStorageTake { dest, .. }
            | MirInstr::LoadPlace { dest, .. }
            | MirInstr::HasNext { dest, .. }
            | MirInstr::Next { dest, .. }
            | MirInstr::TryNext { dest, .. } => {
                Err(self.unsupported_reg(format!("instruction `{}`", instr_name(instr)), *dest))
            }
            MirInstr::WriteRef { .. }
            | MirInstr::GetIter { .. }
            | MirInstr::VariantSet { .. }
            | MirInstr::VariantSetInitWith { .. }
            | MirInstr::VariantDeinitWith { .. }
            | MirInstr::MultiSet { .. }
            | MirInstr::Store { .. }
            | MirInstr::StoreRef { .. }
            | MirInstr::PointerStorageDestroy { .. }
            | MirInstr::UninitStorageDestroy { .. }
            | MirInstr::Raise { .. }
            | MirInstr::Try { .. }
            | MirInstr::Drop { .. }
            | MirInstr::ConsumeVar { .. }
            | MirInstr::ConsumePlace { .. } => {
                Err(self.unsupported(format!("instruction `{}`", instr_name(instr)), None))
            }
            MirInstr::Unsupported(message) => {
                Err(self.unsupported(format!("lowering-marked construct: {message}"), None))
            }
        }
    }

    fn lower_const(
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
            MirConst::Bool(value) => {
                let i1 = IntegerType::get(ctx, 1, Signedness::Signless);
                let attr = IntegerAttr::new(i1, APInt::from_u64(u64::from(*value), bw(1)));
                let op = ConstantOp::new(ctx, Box::new(attr));
                self.define(ctx, dest, op.get_operation(), op.get_result(ctx))
            }
            MirConst::IntLiteral(literal) => {
                self.pending_literals
                    .insert(dest.0, literal.as_bigint().clone());
                Ok(())
            }
            MirConst::Float(_)
            | MirConst::FloatLiteral(_)
            | MirConst::Str(_)
            | MirConst::Function(_)
            | MirConst::None => {
                Err(self.unsupported_reg(format!("constant `{}`", const_name(k)), dest))
            }
        }
    }

    fn lower_materialize(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        target: &Ty,
    ) -> Result<(), PlironError> {
        if !matches!(target, Ty::Int) {
            return Err(
                self.unsupported_reg(format!("literal materialization to `{target:?}`"), dest)
            );
        }
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
        let Some(as_i64) = literal.to_i64() else {
            return Err(PlironError {
                function: Some(self.name.to_string()),
                kind: PlironErrorKind::LiteralOutOfRange {
                    literal: literal.to_string(),
                    target: "Int (i64)",
                },
                location: self.reg_span(dest),
            });
        };
        let constant = self.int_constant(ctx, as_i64);
        self.reg_values.insert(dest.0, constant);
        Ok(())
    }

    fn lower_unop(
        &mut self,
        ctx: &mut Context,
        op: PrefixOp,
        dest: Reg,
        a: Reg,
    ) -> Result<(), PlironError> {
        // Negation of a pending literal stays a pending literal (the
        // materialization folds the sign into one constant).
        if let Some(literal) = self.pending_literals.get(&a.0).cloned() {
            if !matches!(op, PrefixOp::Neg) {
                return Err(
                    self.unsupported_reg(format!("operator `{op:?}` on an integer literal"), dest)
                );
            }
            self.pending_literals.insert(dest.0, -literal);
            return Ok(());
        }
        let value = self.reg_value(ctx, a)?;
        match op {
            PrefixOp::Neg => {
                let zero = self.int_constant(ctx, 0);
                let neg = SubOp::new_with_overflow_flag(ctx, zero, value, no_overflow_flags());
                self.define(ctx, dest, neg.get_operation(), neg.get_result(ctx))
            }
            PrefixOp::Not => {
                let i1 = IntegerType::get(ctx, 1, Signedness::Signless);
                let attr = IntegerAttr::new(i1, APInt::from_u64(1, bw(1)));
                let one = ConstantOp::new(ctx, Box::new(attr));
                self.append(ctx, one.get_operation(), Some(dest));
                let not = XorOp::new(ctx, value, one.get_result(ctx));
                self.define(ctx, dest, not.get_operation(), not.get_result(ctx))
            }
        }
    }

    fn lower_binop(
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
        let lhs = self.reg_value(ctx, a)?;
        let rhs = self.reg_value(ctx, b)?;
        let operand_ty = self.reg_scalar_ty(a)?;

        if let Some(predicate) = compare_predicate(op) {
            if matches!(operand_ty, ScalarTy::Bool) && !matches!(op, InfixOp::Eq | InfixOp::Ne) {
                return Err(
                    self.unsupported_reg(format!("operator `{op:?}` on Bool operands"), dest)
                );
            }
            let cmp = ICmpOp::new(ctx, predicate, lhs, rhs);
            return self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx));
        }

        if matches!(operand_ty, ScalarTy::Bool) {
            return match op {
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
                other => {
                    Err(self
                        .unsupported_reg(format!("operator `{other:?}` on Bool operands"), dest))
                }
            };
        }

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
                let shr = pliron_llvm::ops::AShrOp::new(ctx, lhs, masked);
                self.define(ctx, dest, shr.get_operation(), shr.get_result(ctx))
            }
            InfixOp::FloorDiv => self.lower_floor_div(ctx, dest, lhs, rhs),
            InfixOp::Mod => self.lower_floor_mod(ctx, dest, lhs, rhs),
            InfixOp::Pow
            | InfixOp::Div
            | InfixOp::MatMul
            | InfixOp::And
            | InfixOp::Or
            | InfixOp::In
            | InfixOp::NotIn => {
                Err(self.unsupported_reg(format!("operator `{op:?}` on Int operands"), dest))
            }
            InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Gt | InfixOp::Le | InfixOp::Ge => {
                unreachable!("comparisons handled above")
            }
        }
    }

    /// `floor_div`: `sdiv` rounds toward zero; subtract one when the remainder
    /// is non-zero and the operand signs differ (matches `runtime.rs`).
    fn lower_floor_div(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let quotient = SDivOp::new(ctx, lhs, rhs);
        self.append(ctx, quotient.get_operation(), Some(dest));
        let adjust = self.floor_adjust_flag(ctx, dest, lhs, rhs)?;
        let one = self.int_constant(ctx, 1);
        let minus_one =
            SubOp::new_with_overflow_flag(ctx, quotient.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, minus_one.get_operation(), Some(dest));
        let select = SelectOp::new(
            ctx,
            adjust,
            minus_one.get_result(ctx),
            quotient.get_result(ctx),
        );
        self.define(ctx, dest, select.get_operation(), select.get_result(ctx))
    }

    /// `floor_mod`: `srem` takes the dividend's sign; add the divisor when the
    /// remainder is non-zero and the operand signs differ.
    fn lower_floor_mod(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        lhs: Value,
        rhs: Value,
    ) -> Result<(), PlironError> {
        let adjust = self.floor_adjust_flag(ctx, dest, lhs, rhs)?;
        let remainder = SRemOp::new(ctx, lhs, rhs);
        self.append(ctx, remainder.get_operation(), Some(dest));
        let plus_divisor =
            AddOp::new_with_overflow_flag(ctx, remainder.get_result(ctx), rhs, no_overflow_flags());
        self.append(ctx, plus_divisor.get_operation(), Some(dest));
        let select = SelectOp::new(
            ctx,
            adjust,
            plus_divisor.get_result(ctx),
            remainder.get_result(ctx),
        );
        self.define(ctx, dest, select.get_operation(), select.get_result(ctx))
    }

    /// `(srem(lhs, rhs) != 0) & ((srem(lhs, rhs) ^ rhs) < 0)` — true exactly
    /// when truncating division must be floored.
    fn floor_adjust_flag(
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

    fn lower_term(&mut self, ctx: &mut Context, term: &MirTerm) -> Result<(), PlironError> {
        match term {
            MirTerm::Jump(target) => {
                let jump = BrOp::new(ctx, self.block(*target)?, vec![]);
                self.append(ctx, jump.get_operation(), None);
                Ok(())
            }
            MirTerm::Branch {
                cond,
                then_b,
                else_b,
            } => {
                let condition = self.reg_value(ctx, *cond)?;
                let then_block = self.block(*then_b)?;
                let else_block = self.block(*else_b)?;
                let branch = CondBrOp::new(ctx, condition, then_block, vec![], else_block, vec![]);
                self.append(ctx, branch.get_operation(), Some(*cond));
                Ok(())
            }
            MirTerm::Return(value) => {
                let lowered = match value {
                    Some(reg) => Some(self.reg_value(ctx, *reg)?),
                    None => None,
                };
                // A value-less return inside a value-returning function is
                // checker-guaranteed unreachable fall-off scaffolding.
                if lowered.is_none() && !matches!(self.func.ret_ty, Some(Ty::None)) {
                    let unreachable = pliron_llvm::ops::UnreachableOp::new(ctx);
                    self.append(ctx, unreachable.get_operation(), None);
                    return Ok(());
                }
                let ret = ReturnOp::new(ctx, lowered);
                self.append(ctx, ret.get_operation(), value.as_ref().copied());
                Ok(())
            }
            MirTerm::ReturnWithCleanup { .. } | MirTerm::FallOff | MirTerm::EscapeJump { .. } => {
                Err(self.unsupported(format!("terminator `{}`", term_name(term)), None))
            }
        }
    }

    /// Mask a shift amount with `& 63`, matching the VM's
    /// `wrapping_shl`/`wrapping_shr` modulo-width semantics.
    fn masked_shift_amount(&mut self, ctx: &mut Context, amount: Value, dest: Reg) -> Value {
        let mask = self.int_constant(ctx, 63);
        let masked = AndOp::new(ctx, amount, mask);
        self.append(ctx, masked.get_operation(), Some(dest));
        masked.get_result(ctx)
    }

    /// Emit an i64 constant in the current block and return its value.
    fn int_constant(&mut self, ctx: &mut Context, value: i64) -> Value {
        let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_ty, APInt::from_u64(value as u64, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Append `op` to the current block, stamping the span of `span_reg`
    /// (usually the instruction's dest) as its location when available.
    fn append(&mut self, ctx: &mut Context, op: Ptr<Operation>, span_reg: Option<Reg>) {
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
    fn define(
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

    fn reg_value(&mut self, ctx: &mut Context, reg: Reg) -> Result<Value, PlironError> {
        if let Some(value) = self.reg_values.get(&reg.0) {
            return Ok(*value);
        }
        // Instructions may consume `IntLiteral`-typed operands directly
        // (e.g. shift amounts); the VM reads them at i64, so materialize
        // in place with the same range contract as `MaterializeLiteral`.
        if let Some(literal) = self.pending_literals.get(&reg.0).cloned() {
            let Some(as_i64) = literal.to_i64() else {
                return Err(PlironError {
                    function: Some(self.name.to_string()),
                    kind: PlironErrorKind::LiteralOutOfRange {
                        literal: literal.to_string(),
                        target: "Int (i64)",
                    },
                    location: self.reg_span(reg),
                });
            };
            let value = self.int_constant(ctx, as_i64);
            self.reg_values.insert(reg.0, value);
            return Ok(value);
        }
        let construct = if self.erased.contains(&reg.0) {
            format!("read of erased analysis register %r{}", reg.0)
        } else {
            format!("read of undefined register %r{}", reg.0)
        };
        Err(self.unsupported(construct, self.reg_span(reg)))
    }

    fn reg_scalar_ty(&self, reg: Reg) -> Result<ScalarTy, PlironError> {
        let Some(ty) = self.func.reg_types.get(&reg.0) else {
            return Err(self.unsupported(format!("untyped register %r{}", reg.0), None));
        };
        // Literal-typed operands materialize at i64 (see `reg_value`).
        if matches!(ty, Ty::IntLiteral) {
            return Ok(ScalarTy::Int);
        }
        scalar_type(self.name, ty, self.reg_span(reg))
    }

    fn var_scalar_ty(&self, var: u32) -> Result<ScalarTy, PlironError> {
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
        scalar_type(self.name, ty, None)
    }

    fn block(&self, id: MirBlockId) -> Result<Ptr<pliron::basic_block::BasicBlock>, PlironError> {
        self.blocks
            .get(id)
            .copied()
            .ok_or_else(|| self.unsupported(format!("branch to missing block bb{id}"), None))
    }

    fn reg_span(&self, reg: Reg) -> Option<SourceSpan> {
        self.func.spans.0.get(&reg.0).map(|(span, _)| span.clone())
    }

    fn unsupported(&self, construct: String, location: Option<SourceSpan>) -> PlironError {
        PlironError {
            function: Some(self.name.to_string()),
            kind: PlironErrorKind::Unsupported { construct },
            location,
        }
    }

    fn unsupported_reg(&self, construct: String, dest: Reg) -> PlironError {
        self.unsupported(construct, self.reg_span(dest))
    }
}

/// Map a checked type to its Stage 1 scalar lowering, or reject it.
fn scalar_type(
    function: &str,
    ty: &Ty,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    match ty {
        Ty::Int => Ok(ScalarTy::Int),
        Ty::Bool => Ok(ScalarTy::Bool),
        other => Err(PlironError {
            function: Some(function.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("type `{other:?}`"),
            },
            location,
        }),
    }
}

fn compare_predicate(op: InfixOp) -> Option<ICmpPredicateAttr> {
    match op {
        InfixOp::Eq => Some(ICmpPredicateAttr::EQ),
        InfixOp::Ne => Some(ICmpPredicateAttr::NE),
        InfixOp::Lt => Some(ICmpPredicateAttr::SLT),
        InfixOp::Le => Some(ICmpPredicateAttr::SLE),
        InfixOp::Gt => Some(ICmpPredicateAttr::SGT),
        InfixOp::Ge => Some(ICmpPredicateAttr::SGE),
        _ => None,
    }
}

fn no_overflow_flags() -> IntegerOverflowFlagsAttr {
    IntegerOverflowFlagsAttr {
        nsw: false,
        nuw: false,
    }
}

fn instr_name(instr: &MirInstr) -> &'static str {
    match instr {
        MirInstr::EstablishLoans { .. } => "EstablishLoans",
        MirInstr::InvalidateInteriors { .. } => "InvalidateInteriors",
        MirInstr::MakeRef { .. } => "MakeRef",
        MirInstr::ReadRef { .. } => "ReadRef",
        MirInstr::CopyValue { .. } => "CopyValue",
        MirInstr::WriteRef { .. } => "WriteRef",
        MirInstr::MakeClosure { .. } => "MakeClosure",
        MirInstr::KeepAlive { .. } => "KeepAlive",
        MirInstr::Const { .. } => "Const",
        MirInstr::MaterializeLiteral { .. } => "MaterializeLiteral",
        MirInstr::UseVar { .. } => "UseVar",
        MirInstr::MovePlace { .. } => "MovePlace",
        MirInstr::DefVar { .. } => "DefVar",
        MirInstr::UnOp { .. } => "UnOp",
        MirInstr::BinOp { .. } => "BinOp",
        MirInstr::Call { .. } => "Call",
        MirInstr::CallIndirect { .. } => "CallIndirect",
        MirInstr::MethodCall { .. } => "MethodCall",
        MirInstr::GetField { .. } => "GetField",
        MirInstr::Index { .. } => "Index",
        MirInstr::Slice { .. } => "Slice",
        MirInstr::MultiIndex { .. } => "MultiIndex",
        MirInstr::MultiSet { .. } => "MultiSet",
        MirInstr::Store { .. } => "Store",
        MirInstr::StoreRef { .. } => "StoreRef",
        MirInstr::LoadPlace { .. } => "LoadPlace",
        MirInstr::MakeTuple { .. } => "MakeTuple",
        MirInstr::MakeVariant { .. } => "MakeVariant",
        MirInstr::VariantIs { .. } => "VariantIs",
        MirInstr::VariantGet { .. } => "VariantGet",
        MirInstr::VariantSet { .. } => "VariantSet",
        MirInstr::VariantTake { .. } => "VariantTake",
        MirInstr::VariantSetInitWith { .. } => "VariantSetInitWith",
        MirInstr::VariantDeinitWith { .. } => "VariantDeinitWith",
        MirInstr::VariantReplace { .. } => "VariantReplace",
        MirInstr::MakeSimd { .. } => "MakeSimd",
        MirInstr::SimdCast { .. } => "SimdCast",
        MirInstr::SimdShuffle { .. } => "SimdShuffle",
        MirInstr::PointerStorageTake { .. } => "PointerStorageTake",
        MirInstr::PointerStorageDestroy { .. } => "PointerStorageDestroy",
        MirInstr::UninitStorage { .. } => "UninitStorage",
        MirInstr::UninitStorageTake { .. } => "UninitStorageTake",
        MirInstr::UninitStorageDestroy { .. } => "UninitStorageDestroy",
        MirInstr::Raise { .. } => "Raise",
        MirInstr::Try { .. } => "Try",
        MirInstr::Drop { .. } => "Drop",
        MirInstr::DropVar { .. } => "DropVar",
        MirInstr::ConsumeVar { .. } => "ConsumeVar",
        MirInstr::ConsumePlace { .. } => "ConsumePlace",
        MirInstr::GetIter { .. } => "GetIter",
        MirInstr::HasNext { .. } => "HasNext",
        MirInstr::Next { .. } => "Next",
        MirInstr::TryNext { .. } => "TryNext",
        MirInstr::Unsupported(_) => "Unsupported",
    }
}

fn term_name(term: &MirTerm) -> &'static str {
    match term {
        MirTerm::Jump(_) => "Jump",
        MirTerm::Branch { .. } => "Branch",
        MirTerm::Return(_) => "Return",
        MirTerm::ReturnWithCleanup { .. } => "ReturnWithCleanup",
        MirTerm::FallOff => "FallOff",
        MirTerm::EscapeJump { .. } => "EscapeJump",
    }
}

fn const_name(k: &MirConst) -> &'static str {
    match k {
        MirConst::Int(_) => "Int",
        MirConst::Float(_) => "Float",
        MirConst::IntLiteral(_) => "IntLiteral",
        MirConst::FloatLiteral(_) => "FloatLiteral",
        MirConst::Bool(_) => "Bool",
        MirConst::Str(_) => "Str",
        MirConst::Function(_) => "Function",
        MirConst::None => "None",
    }
}
