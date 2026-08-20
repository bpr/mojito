//! Per-function lowering of scalar MIR to pliron's LLVM dialect.
//!
//! Registers map to SSA values inside their block; cross-block dataflow
//! arrives through variable slots, which lower to entry-block allocas with
//! `load`/`store` at each `UseVar`/`DefVar` (pliron's mem2reg pass rebuilds
//! SSA afterwards). Integer and float literals fold through their
//! `MaterializeLiteral` into single constants with the VM's exact wrapping
//! semantics; `FloorDiv`/`Mod` expand to branch-free select sequences matching
//! the VM's `floor_div`/`floor_mod` behind a division-by-zero trap guard, and
//! shift amounts are masked to the VM's `wrapping_shl`/`wrapping_shr`
//! semantics. Checked scalar traps branch to per-category blocks that call the
//! C `exit` with the [`TrapCategory`] exit code. Everything outside the scalar
//! subset produces a contextual [`PlironError`].

use std::collections::{HashMap, HashSet};

use pliron::basic_block::BasicBlock;
use pliron::builtin::attributes::{FPDoubleAttr, IntegerAttr, StringAttr};
use pliron::builtin::op_interfaces::{
    CallOpCallable, OneResultInterface, SingleBlockRegionInterface,
};
use pliron::builtin::ops::ModuleOp;
use pliron::builtin::types::{FP64Type, IntegerType, Signedness};
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::location::{Located, Location};
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::r#type::{TypeHandle, TypedHandle};
use pliron::utils::apint::{APInt, bw};
use pliron::value::Value;
use pliron_llvm::attributes::{
    FCmpPredicateAttr, FastmathFlagsAttr, ICmpPredicateAttr, IntegerOverflowFlagsAttr,
};
use pliron_llvm::op_interfaces::{
    BinArithOp, CastOpInterface, CastOpWithNNegInterface, FastMathFlags,
    FloatBinArithOpWithFastMathFlags, IntBinArithOpWithOverflowFlag,
};
use pliron_llvm::ops::{
    AShrOp, AddOp, AllocaOp, AndOp, BrOp, CallIntrinsicOp, CallOp, CondBrOp, ConstantOp, FAddOp,
    FCmpOp, FDivOp, FMulOp, FNegOp, FSubOp, FuncOp, ICmpOp, LShrOp, LoadOp, MulOp, OrOp, ReturnOp,
    SDivOp, SIToFPOp, SRemOp, SelectOp, ShlOp, StoreOp, SubOp, UDivOp, UIToFPOp, URemOp,
    UnreachableOp, XorOp, ZExtOp,
};
use pliron_llvm::types::{FuncType, VoidType};

use crate::ast::{InfixOp, PrefixOp};
use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::checked::CheckedConst;
use crate::literal::{FloatLiteral, IntLiteral};
use crate::mir::{
    Const as MirConst, MirBlockId, MirFunction, MirFunctionDeclaration, MirInstr, MirTerm, Reg,
    UseMode,
};
use crate::token::SourceSpan;
use crate::types::Ty;

use super::{PlironError, PlironErrorKind, RetKind, TrapCategory, mangle};

/// The callable identity of one reachable function: its mangled symbol,
/// LLVM-dialect function type, and scalar parameter/result kinds. Built by
/// [`declare_function`], consumed by call lowering.
pub(super) struct FnSignature {
    pub mangled: String,
    pub func_ty: TypedHandle<FuncType>,
    pub returns_value: bool,
    pub params: Vec<ScalarTy>,
    pub ret: RetKind,
}

/// Module-level lowering state shared by every function: the module itself
/// plus the lazily declared/emitted runtime scaffolding (`exit` for trap
/// blocks and the `mjrt_pow` helper). The `mjrt_` prefix, like `exit` and
/// `main`, is outside the injective `mj_` mangle image (see `mangle`).
pub(super) struct ModuleShared {
    module: ModuleOp,
    exit_ty: Option<TypedHandle<FuncType>>,
    pow_ty: Option<TypedHandle<FuncType>>,
}

impl ModuleShared {
    pub(super) fn new(module: ModuleOp) -> ModuleShared {
        ModuleShared {
            module,
            exit_ty: None,
            pow_ty: None,
        }
    }

    /// Declare the C `exit(i32)` once and return its call type.
    fn ensure_exit(&mut self, ctx: &mut Context) -> TypedHandle<FuncType> {
        if let Some(ty) = self.exit_ty {
            return ty;
        }
        let void = VoidType::get(ctx).to_handle();
        let i32_ty: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let exit_ty = FuncType::get(ctx, void, vec![i32_ty], false);
        let exit = FuncOp::new(ctx, "exit".try_into().expect("valid identifier"), exit_ty);
        self.module.append_operation(ctx, exit.get_operation(), 0);
        self.exit_ty = Some(exit_ty);
        exit_ty
    }

    /// Emit the wrapping square-and-multiply `mjrt_pow(base, exp) -> i64`
    /// once and return its call type. Callers guard the exponent range first;
    /// wrapping i64 multiplication is bit-identical for Int and UInt, so one
    /// helper serves both.
    fn ensure_pow(&mut self, ctx: &mut Context) -> TypedHandle<FuncType> {
        if let Some(ty) = self.pow_ty {
            return ty;
        }
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let pow_ty = FuncType::get(ctx, i64_ty, vec![i64_ty, i64_ty], false);
        let func = FuncOp::new(
            ctx,
            "mjrt_pow".try_into().expect("valid identifier"),
            pow_ty,
        );
        self.module.append_operation(ctx, func.get_operation(), 0);
        emit_pow_body(ctx, func);
        self.pow_ty = Some(pow_ty);
        pow_ty
    }
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
    let (result, returns_value, ret) = match ret_ty {
        Ty::None => (VoidType::get(ctx).to_handle(), false, RetKind::Void),
        other => {
            let scalar = scalar_type(name, other, None)?;
            (scalar.handle(ctx), true, scalar.ret_kind())
        }
    };
    let mut params = Vec::with_capacity(func.param_types.len());
    let mut param_handles = Vec::with_capacity(func.param_types.len());
    for ty in &func.param_types {
        let scalar = scalar_type(name, ty, None)?;
        param_handles.push(scalar.handle(ctx));
        params.push(scalar);
    }
    let func_ty = FuncType::get(ctx, result, param_handles, false);
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
            params,
            ret,
        },
    ))
}

/// The module-wide read-only lowering environment shared by every function
/// body: compiled signatures, call-binding declarations, and the span
/// locator.
pub(super) struct LowerEnv<'a> {
    pub signatures: &'a HashMap<String, FnSignature>,
    pub declarations: &'a HashMap<String, MirFunctionDeclaration>,
    pub locator: &'a Locator,
}

/// Lower `func`'s body into the declared `func_op`.
pub(super) fn lower_body(
    ctx: &mut Context,
    name: &str,
    func: &MirFunction,
    func_op: FuncOp,
    env: &LowerEnv<'_>,
    shared: &mut ModuleShared,
) -> Result<(), PlironError> {
    let mut lowering = FnLowering {
        name,
        func,
        signatures: env.signatures,
        declarations: env.declarations,
        locator: env.locator,
        shared,
        reg_values: HashMap::new(),
        pending_literals: HashMap::new(),
        erased: HashSet::new(),
        var_slots: Vec::new(),
        blocks: Vec::new(),
        trap_blocks: HashMap::new(),
        region: None,
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

/// The scalar value types the backend lowers. `Int` and `UInt` share the
/// signless i64 representation and differ only in operator selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScalarTy {
    Int,
    UInt,
    Float64,
    Bool,
}

impl ScalarTy {
    fn handle(self, ctx: &mut Context) -> TypeHandle {
        match self {
            ScalarTy::Int | ScalarTy::UInt => {
                IntegerType::get(ctx, 64, Signedness::Signless).into()
            }
            ScalarTy::Float64 => FP64Type::get(ctx).into(),
            ScalarTy::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
        }
    }

    fn ret_kind(self) -> RetKind {
        match self {
            ScalarTy::Int => RetKind::I64,
            ScalarTy::UInt => RetKind::U64,
            ScalarTy::Float64 => RetKind::F64,
            ScalarTy::Bool => RetKind::Bool,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ScalarTy::Int => "Int",
            ScalarTy::UInt => "UInt",
            ScalarTy::Float64 => "Float64",
            ScalarTy::Bool => "Bool",
        }
    }
}

/// A register holding a not-yet-materialized literal. Kept exact until a
/// consumer fixes the target type, mirroring the VM's literal values.
#[derive(Clone)]
enum PendingLiteral {
    Int(IntLiteral),
    Float(FloatLiteral),
}

struct FnLowering<'a> {
    name: &'a str,
    func: &'a MirFunction,
    signatures: &'a HashMap<String, FnSignature>,
    declarations: &'a HashMap<String, MirFunctionDeclaration>,
    locator: &'a Locator,
    shared: &'a mut ModuleShared,
    /// Materialized SSA value of each register.
    reg_values: HashMap<u32, Value>,
    /// Registers holding a not-yet-materialized literal.
    pending_literals: HashMap<u32, PendingLiteral>,
    /// Registers written by semantically erased instructions (analysis
    /// markers, void call results). Reading one is an internal invariant
    /// violation surfaced as a diagnostic, never a silent miscompile.
    erased: HashSet<u32>,
    /// Alloca (pointer) value of each variable slot.
    var_slots: Vec<Value>,
    /// Pliron block for each MIR block id.
    blocks: Vec<Ptr<BasicBlock>>,
    /// Lazily created per-category trap blocks of this function.
    trap_blocks: HashMap<u8, Ptr<BasicBlock>>,
    /// The function's body region (for appending guard/trap blocks).
    region: Option<Ptr<pliron::region::Region>>,
    current: Option<Ptr<BasicBlock>>,
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
        self.region = Some(region);

        // One pliron block per MIR block (entry stays separate so MIR block 0
        // may have predecessors).
        for _ in 0..self.func.blocks.len() {
            let block = BasicBlock::new(ctx, None, vec![]);
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
                let expected = self.var_scalar_ty(*var)?;
                let value = self.reg_value(ctx, *src, expected)?;
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
                if arg_places.iter().any(Option::is_some)
                    || kwarg_places.iter().any(Option::is_some)
                    || !capture_accesses.is_empty()
                    || !param_arg_regs.is_empty()
                {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                self.lower_call(ctx, *dest, &func.0, args, kwargs)
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
            // Everything below is outside the scalar subset. Every variant is
            // named so that new instructions force a decision here.
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

    /// Lower a direct call: builtin scalar conversions intercept by name
    /// exactly like the VM; everything else binds against the compiled
    /// signature, resolving keywords and constant defaults through the shared
    /// call-slot matcher.
    fn lower_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if !self.signatures.contains_key(name) {
            if matches!(name, "Int" | "UInt" | "Float64" | "Bool") {
                return self.lower_convert(ctx, dest, name, args, kwargs);
            }
            return Err(self.unsupported_reg(
                format!("call to unknown or builtin function `{name}`"),
                dest,
            ));
        }

        let lowered_args = if kwargs.is_empty() && args.len() == self.signatures[name].params.len()
        {
            let params = self.signatures[name].params.clone();
            let mut lowered = Vec::with_capacity(args.len());
            for (arg, expected) in args.iter().zip(params) {
                lowered.push(self.reg_value(ctx, *arg, expected)?);
            }
            lowered
        } else {
            self.bind_call_slots(ctx, dest, name, args, kwargs)?
        };

        let signature = &self.signatures[name];
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let (func_ty, returns_value) = (signature.func_ty, signature.returns_value);
        let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, lowered_args);
        if returns_value {
            self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
        } else {
            self.append(ctx, call.get_operation(), Some(dest));
            self.erased.insert(dest.0);
            Ok(())
        }
    }

    /// Resolve keyword arguments and constant defaults into the callee's
    /// positional parameter order via `call::match_call_slots` — the same
    /// structural binding the VM applies (`src/call.rs` owns the policy).
    fn bind_call_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<Vec<Value>, PlironError> {
        let Some(decl) = self.declarations.get(name) else {
            return Err(self.unsupported_reg(
                format!("call to `{name}` without a recorded declaration"),
                dest,
            ));
        };
        if decl.variadic.is_some() || decl.kw_variadic.is_some() {
            return Err(self.unsupported_reg(format!("variadic call to `{name}`"), dest));
        }
        let kw_names: Vec<&str> = kwargs.iter().map(|(n, _)| n.as_str()).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("call binding for `{name}` failed: {error:?}"), dest)
        })?;
        let params = self.signatures[name].params.clone();
        if matched.slots.len() != params.len() {
            return Err(self.unsupported_reg(
                format!("call binding for `{name}` disagrees with its compiled arity"),
                dest,
            ));
        }
        let defaults = decl.defaults.clone();
        let mut lowered = Vec::with_capacity(params.len());
        for (i, slot) in matched.slots.iter().enumerate() {
            let expected = params[i];
            let value = match slot {
                ArgSlot::Positional(p) => self.reg_value(ctx, args[*p], expected)?,
                ArgSlot::Keyword(k) => self.reg_value(ctx, kwargs[*k].1, expected)?,
                ArgSlot::Default => {
                    let Some(default) = defaults.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("non-constant default argument in call to `{name}`"),
                            dest,
                        ));
                    };
                    self.checked_const_value(ctx, default, expected, dest)?
                }
            };
            lowered.push(value);
        }
        Ok(lowered)
    }

    /// Materialize a constant default at the parameter's scalar type, exactly
    /// as the VM's default binding materializes the literal.
    fn checked_const_value(
        &mut self,
        ctx: &mut Context,
        value: &CheckedConst,
        expected: ScalarTy,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        match (value, expected) {
            (CheckedConst::Int(literal), _) => {
                let literal = PendingLiteral::Int(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Float(literal), _) => {
                let literal = PendingLiteral::Float(literal.clone());
                self.materialize_pending(ctx, &literal, expected, dest)
            }
            (CheckedConst::Bool(value), ScalarTy::Bool) => Ok(self.bool_constant(ctx, *value)),
            (CheckedConst::Bool(_), other) => Err(self.unsupported_reg(
                format!("Bool default argument for a `{}` parameter", other.name()),
                dest,
            )),
            (CheckedConst::String(_) | CheckedConst::None, _) => {
                Err(self.unsupported_reg("non-scalar default argument".into(), dest))
            }
        }
    }

    /// `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)` over a scalar operand,
    /// mirroring `runtime::builtin_convert`: float-to-integer saturates (NaN
    /// becomes 0), integer reinterpretations are bit-exact, and `Bool` is a
    /// non-zero test (`fcmp une` — `Bool(NaN)` is `True`).
    fn lower_convert(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(
                self.unsupported_reg(format!("conversion call contract for `{name}`"), dest)
            );
        }
        let arg = args[0];
        let target = match name {
            "Int" => ScalarTy::Int,
            "UInt" => ScalarTy::UInt,
            "Float64" => ScalarTy::Float64,
            _ => ScalarTy::Bool,
        };

        // Literal arguments fold at compile time (the VM's literal branches
        // of `builtin_convert`).
        if let Some(literal) = self.pending_literals.get(&arg.0).cloned() {
            let value = match (&literal, target) {
                (PendingLiteral::Int(_) | PendingLiteral::Float(_), ScalarTy::Bool) => {
                    let non_zero = match &literal {
                        PendingLiteral::Int(literal) => !literal.is_zero(),
                        PendingLiteral::Float(literal) => !literal.is_zero(),
                    };
                    self.bool_constant(ctx, non_zero)
                }
                (PendingLiteral::Float(literal), ScalarTy::Int | ScalarTy::UInt) => {
                    let truncated = PendingLiteral::Int(literal.trunc_to_int());
                    self.materialize_pending(ctx, &truncated, target, dest)?
                }
                _ => self.materialize_pending(ctx, &literal, target, dest)?,
            };
            self.reg_values.insert(dest.0, value);
            return Ok(());
        }

        let source = self
            .concrete_scalar_ty(arg)?
            .ok_or_else(|| self.unsupported_reg("untyped conversion operand".into(), dest))?;
        let value = self.reg_value(ctx, arg, source)?;
        match (source, target) {
            // Same-representation moves are pure aliases.
            (ScalarTy::Int | ScalarTy::UInt, ScalarTy::Int | ScalarTy::UInt)
            | (ScalarTy::Float64, ScalarTy::Float64)
            | (ScalarTy::Bool, ScalarTy::Bool) => {
                self.reg_values.insert(dest.0, value);
                Ok(())
            }
            (ScalarTy::Float64, ScalarTy::Int | ScalarTy::UInt) => {
                let intrinsic = if target == ScalarTy::Int {
                    "llvm.fptosi.sat.i64.f64"
                } else {
                    "llvm.fptoui.sat.i64.f64"
                };
                let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let fn_ty = FuncType::get(ctx, i64_ty, vec![f64_ty], false);
                let call = CallIntrinsicOp::new(
                    ctx,
                    StringAttr::new(intrinsic.to_string()),
                    fn_ty,
                    vec![value],
                );
                self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
            }
            (ScalarTy::Int, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = SIToFPOp::new(ctx, value, f64_ty);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::UInt, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Bool, ScalarTy::Int | ScalarTy::UInt) => {
                let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let cast = ZExtOp::new_with_nneg(ctx, value, i64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Bool, ScalarTy::Float64) => {
                let f64_ty: TypeHandle = FP64Type::get(ctx).into();
                let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
                self.define(ctx, dest, cast.get_operation(), cast.get_result(ctx))
            }
            (ScalarTy::Int | ScalarTy::UInt, ScalarTy::Bool) => {
                let zero = self.int_constant(ctx, 0);
                let cmp = ICmpOp::new(ctx, ICmpPredicateAttr::NE, value, zero);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
            (ScalarTy::Float64, ScalarTy::Bool) => {
                let zero = self.float_constant(ctx, 0.0);
                let cmp = self.fcmp(ctx, FCmpPredicateAttr::UNE, value, zero);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
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
            MirConst::Str(_) | MirConst::Function(_) | MirConst::None => {
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
        let target = match target {
            Ty::Int => ScalarTy::Int,
            Ty::UInt => ScalarTy::UInt,
            Ty::Float64 => ScalarTy::Float64,
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
            (op, other) => Err(self.unsupported_reg(
                format!("operator `{op:?}` on `{}` operand", other.name()),
                dest,
            )),
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
        }
    }

    fn lower_int_binop(
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
                self.lower_floor_div(ctx, dest, lhs, rhs)
            }
            InfixOp::Mod => {
                self.emit_div_zero_guard(ctx, rhs, dest)?;
                self.lower_floor_mod(ctx, dest, lhs, rhs)
            }
            InfixOp::Pow => self.lower_pow(ctx, dest, lhs, rhs),
            other => {
                Err(self.unsupported_reg(format!("operator `{other:?}` on Int operands"), dest))
            }
        }
    }

    fn lower_uint_binop(
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

    fn lower_float_binop(
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

    /// `llvm.floor.f64` over one value.
    fn float_floor(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let fn_ty = FuncType::get(ctx, f64_ty, vec![f64_ty], false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.floor.f64".to_string()),
            fn_ty,
            vec![value],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// `x ** y` on Int/UInt: guard the exponent to `pow_exp`'s accepted range
    /// (`0 ..= u32::MAX`, one unsigned compare covers negative-as-i64 too),
    /// then call the wrapping `mjrt_pow` helper.
    fn lower_pow(
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
    fn lower_true_div(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        a: Reg,
        b: Reg,
        operand_ty: ScalarTy,
    ) -> Result<(), PlironError> {
        if matches!(operand_ty, ScalarTy::Bool) {
            return Err(self.unsupported_reg("operator `Div` on Bool operands".into(), dest));
        }
        let lhs = self.reg_value(ctx, a, operand_ty)?;
        let rhs = self.reg_value(ctx, b, operand_ty)?;
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
            ScalarTy::Bool => unreachable!("rejected above"),
        };
        let div = FDivOp::new_with_fast_math_flags(ctx, lhs, rhs, FastmathFlagsAttr::default());
        self.define(ctx, dest, div.get_operation(), div.get_result(ctx))
    }

    fn int_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = SIToFPOp::new(ctx, value, f64_ty);
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    fn uint_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = UIToFPOp::new_with_nneg(ctx, value, f64_ty, false);
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    fn lower_compare(
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
            // Rust f64 comparisons: `!=` is true for NaN operands (UNE), the
            // ordered comparisons are false (`runtime::float_op`).
            ScalarTy::Float64 => {
                let cmp = self.fcmp(ctx, float_predicate(op), lhs, rhs);
                self.define(ctx, dest, cmp.get_operation(), cmp.get_result(ctx))
            }
        }
    }

    fn fcmp(
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

    /// Trap when the divisor is zero (the VM's `nonzero`/`nonzero_u` check
    /// behind "integer division or modulo by zero").
    fn emit_div_zero_guard(
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
    fn emit_trap_guard(
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

    /// The function's trap block for `category`: `exit(64 + code)` then
    /// `unreachable`, created on first use.
    fn trap_block(&mut self, ctx: &mut Context, category: TrapCategory) -> Ptr<BasicBlock> {
        if let Some(block) = self.trap_blocks.get(&category.code()) {
            return *block;
        }
        let region = self.region.expect("lowering is inside a function region");
        let exit_ty = self.shared.ensure_exit(ctx);
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        let i32_ty = IntegerType::get(ctx, 32, Signedness::Signless);
        let code_attr = IntegerAttr::new(
            i32_ty,
            APInt::from_u64(u64::from(category.exit_code()), bw(32)),
        );
        let code = ConstantOp::new(ctx, Box::new(code_attr));
        code.get_operation().insert_at_back(block, ctx);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("exit".try_into().expect("valid identifier")),
            exit_ty,
            vec![code.get_result(ctx)],
        );
        call.get_operation().insert_at_back(block, ctx);
        let unreachable = UnreachableOp::new(ctx);
        unreachable.get_operation().insert_at_back(block, ctx);
        self.trap_blocks.insert(category.code(), block);
        block
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
                let condition = self.reg_value(ctx, *cond, ScalarTy::Bool)?;
                let then_block = self.block(*then_b)?;
                let else_block = self.block(*else_b)?;
                let branch = CondBrOp::new(ctx, condition, then_block, vec![], else_block, vec![]);
                self.append(ctx, branch.get_operation(), Some(*cond));
                Ok(())
            }
            MirTerm::Return(value) => {
                // A value-less return inside a value-returning function is
                // checker-guaranteed unreachable fall-off scaffolding.
                let ret_scalar = match self.func.ret_ty.as_ref() {
                    Some(Ty::None) | None => None,
                    Some(other) => Some(scalar_type(self.name, other, None)?),
                };
                let lowered = match (value, ret_scalar) {
                    (Some(reg), Some(expected)) => Some(self.reg_value(ctx, *reg, expected)?),
                    (None, Some(_)) => {
                        let unreachable = UnreachableOp::new(ctx);
                        self.append(ctx, unreachable.get_operation(), None);
                        return Ok(());
                    }
                    (_, None) => None,
                };
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

    /// Emit an i64 constant carrying `value`'s unsigned bits.
    fn uint_constant(&mut self, ctx: &mut Context, value: u64) -> Value {
        let i64_ty = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_ty, APInt::from_u64(value, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an f64 constant in the current block and return its value.
    fn float_constant(&mut self, ctx: &mut Context, value: f64) -> Value {
        let attr = FPDoubleAttr::from(value);
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an i1 constant in the current block and return its value.
    fn bool_constant(&mut self, ctx: &mut Context, value: bool) -> Value {
        let i1 = IntegerType::get(ctx, 1, Signedness::Signless);
        let attr = IntegerAttr::new(i1, APInt::from_u64(u64::from(value), bw(1)));
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

    /// The SSA value of `reg`, materializing a pending literal at `expected`
    /// on demand (instructions may consume literal-typed operands directly,
    /// e.g. shift amounts; the VM materializes them at their consumer's kind).
    fn reg_value(
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
        } else {
            format!("read of undefined register %r{}", reg.0)
        };
        Err(self.unsupported(construct, self.reg_span(reg)))
    }

    /// Fold a pending literal into one constant of the target scalar type
    /// with the VM's exact semantics (`runtime::materialize_literal`):
    /// integers wrap modulo 2^64, floats convert exactly.
    fn materialize_pending(
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
            (PendingLiteral::Float(literal), other) => Err(self.unsupported(
                format!(
                    "float literal `{literal}` materialization to `{}`",
                    other.name()
                ),
                self.reg_span(span_reg),
            )),
            (PendingLiteral::Int(literal), ScalarTy::Bool) => Err(self.unsupported(
                format!("integer literal `{}` used as Bool", literal.as_bigint()),
                self.reg_span(span_reg),
            )),
        }
    }

    fn literal_out_of_range(
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
    fn binop_operand_ty(&self, a: Reg, b: Reg) -> Result<ScalarTy, PlironError> {
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
    fn concrete_scalar_ty(&self, reg: Reg) -> Result<Option<ScalarTy>, PlironError> {
        let Some(ty) = self.func.reg_types.get(&reg.0) else {
            return Err(self.unsupported(format!("untyped register %r{}", reg.0), None));
        };
        if matches!(ty, Ty::IntLiteral | Ty::FloatLiteral) {
            return Ok(None);
        }
        scalar_type(self.name, ty, self.reg_span(reg)).map(Some)
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

    fn block(&self, id: MirBlockId) -> Result<Ptr<BasicBlock>, PlironError> {
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

/// The wrapping square-and-multiply body of `mjrt_pow`. The exponent arrives
/// range-guarded; overflow of the accumulating multiplications wraps, in the
/// same recorded-divergence class as the plain `+`/`-`/`*` operators (the VM's
/// `i64::pow` has no defined overflow semantics).
fn emit_pow_body(ctx: &mut Context, func: FuncOp) {
    let entry = func.get_or_create_entry_block(ctx);
    let region = func
        .get_operation()
        .deref(ctx)
        .regions()
        .next()
        .expect("llvm.func has a body region");
    let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();

    let head = BasicBlock::new(ctx, None, vec![]);
    head.insert_at_back(region, ctx);
    let body = BasicBlock::new(ctx, None, vec![]);
    body.insert_at_back(region, ctx);
    let multiply = BasicBlock::new(ctx, None, vec![]);
    multiply.insert_at_back(region, ctx);
    let advance = BasicBlock::new(ctx, None, vec![]);
    advance.insert_at_back(region, ctx);
    let done = BasicBlock::new(ctx, None, vec![]);
    done.insert_at_back(region, ctx);

    let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
    fn constant(
        ctx: &mut Context,
        i64_int: TypedHandle<IntegerType>,
        value: u64,
        block: Ptr<BasicBlock>,
    ) -> Value {
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(value, bw(64)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        op.get_operation().insert_at_back(block, ctx);
        op.get_result(ctx)
    }

    // entry: allocas for acc/base/exp, seeded from the arguments.
    let one = constant(ctx, i64_int, 1, entry);
    let acc_alloca = AllocaOp::new(ctx, i64_ty, one);
    acc_alloca.get_operation().insert_at_back(entry, ctx);
    let base_alloca = AllocaOp::new(ctx, i64_ty, one);
    base_alloca.get_operation().insert_at_back(entry, ctx);
    let exp_alloca = AllocaOp::new(ctx, i64_ty, one);
    exp_alloca.get_operation().insert_at_back(entry, ctx);
    let acc_slot = acc_alloca.get_result(ctx);
    let base_slot = base_alloca.get_result(ctx);
    let exp_slot = exp_alloca.get_result(ctx);
    let seed_acc = StoreOp::new(ctx, one, acc_slot);
    seed_acc.get_operation().insert_at_back(entry, ctx);
    let base_arg = entry.deref(ctx).get_argument(0);
    let exp_arg = entry.deref(ctx).get_argument(1);
    let seed_base = StoreOp::new(ctx, base_arg, base_slot);
    seed_base.get_operation().insert_at_back(entry, ctx);
    let seed_exp = StoreOp::new(ctx, exp_arg, exp_slot);
    seed_exp.get_operation().insert_at_back(entry, ctx);
    let to_head = BrOp::new(ctx, head, vec![]);
    to_head.get_operation().insert_at_back(entry, ctx);

    // head: while exp != 0.
    let exp0 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp0.get_operation().insert_at_back(head, ctx);
    let zero = constant(ctx, i64_int, 0, head);
    let live = ICmpOp::new(ctx, ICmpPredicateAttr::NE, exp0.get_result(ctx), zero);
    live.get_operation().insert_at_back(head, ctx);
    let head_branch = CondBrOp::new(ctx, live.get_result(ctx), body, vec![], done, vec![]);
    head_branch.get_operation().insert_at_back(head, ctx);

    // body: multiply into acc when the low exponent bit is set.
    let exp1 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp1.get_operation().insert_at_back(body, ctx);
    let bit_one = constant(ctx, i64_int, 1, body);
    let bit = AndOp::new(ctx, exp1.get_result(ctx), bit_one);
    bit.get_operation().insert_at_back(body, ctx);
    let body_zero = constant(ctx, i64_int, 0, body);
    let odd = ICmpOp::new(ctx, ICmpPredicateAttr::NE, bit.get_result(ctx), body_zero);
    odd.get_operation().insert_at_back(body, ctx);
    let body_branch = CondBrOp::new(ctx, odd.get_result(ctx), multiply, vec![], advance, vec![]);
    body_branch.get_operation().insert_at_back(body, ctx);

    // multiply: acc = acc * base (wrapping).
    let acc0 = LoadOp::new(ctx, acc_slot, i64_ty);
    acc0.get_operation().insert_at_back(multiply, ctx);
    let base0 = LoadOp::new(ctx, base_slot, i64_ty);
    base0.get_operation().insert_at_back(multiply, ctx);
    let acc1 = MulOp::new_with_overflow_flag(
        ctx,
        acc0.get_result(ctx),
        base0.get_result(ctx),
        no_overflow_flags(),
    );
    acc1.get_operation().insert_at_back(multiply, ctx);
    let save_acc = StoreOp::new(ctx, acc1.get_result(ctx), acc_slot);
    save_acc.get_operation().insert_at_back(multiply, ctx);
    let multiply_join = BrOp::new(ctx, advance, vec![]);
    multiply_join.get_operation().insert_at_back(multiply, ctx);

    // advance: base = base * base (wrapping); exp >>= 1 (logical).
    let base1 = LoadOp::new(ctx, base_slot, i64_ty);
    base1.get_operation().insert_at_back(advance, ctx);
    let base2 = MulOp::new_with_overflow_flag(
        ctx,
        base1.get_result(ctx),
        base1.get_result(ctx),
        no_overflow_flags(),
    );
    base2.get_operation().insert_at_back(advance, ctx);
    let save_base = StoreOp::new(ctx, base2.get_result(ctx), base_slot);
    save_base.get_operation().insert_at_back(advance, ctx);
    let exp2 = LoadOp::new(ctx, exp_slot, i64_ty);
    exp2.get_operation().insert_at_back(advance, ctx);
    let shift_one = constant(ctx, i64_int, 1, advance);
    let exp3 = LShrOp::new(ctx, exp2.get_result(ctx), shift_one);
    exp3.get_operation().insert_at_back(advance, ctx);
    let save_exp = StoreOp::new(ctx, exp3.get_result(ctx), exp_slot);
    save_exp.get_operation().insert_at_back(advance, ctx);
    let advance_loop = BrOp::new(ctx, head, vec![]);
    advance_loop.get_operation().insert_at_back(advance, ctx);

    // done: return acc.
    let result = LoadOp::new(ctx, acc_slot, i64_ty);
    result.get_operation().insert_at_back(done, ctx);
    let ret = ReturnOp::new(ctx, Some(result.get_result(ctx)));
    ret.get_operation().insert_at_back(done, ctx);
}

/// Map a checked type to its scalar lowering, or reject it.
fn scalar_type(
    function: &str,
    ty: &Ty,
    location: Option<SourceSpan>,
) -> Result<ScalarTy, PlironError> {
    match ty {
        Ty::Int => Ok(ScalarTy::Int),
        Ty::UInt => Ok(ScalarTy::UInt),
        Ty::Float64 => Ok(ScalarTy::Float64),
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

fn is_comparison(op: InfixOp) -> bool {
    matches!(
        op,
        InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge
    )
}

fn signed_predicate(op: InfixOp) -> ICmpPredicateAttr {
    match op {
        InfixOp::Eq => ICmpPredicateAttr::EQ,
        InfixOp::Ne => ICmpPredicateAttr::NE,
        InfixOp::Lt => ICmpPredicateAttr::SLT,
        InfixOp::Le => ICmpPredicateAttr::SLE,
        InfixOp::Gt => ICmpPredicateAttr::SGT,
        InfixOp::Ge => ICmpPredicateAttr::SGE,
        other => unreachable!("`{other:?}` is not a comparison"),
    }
}

fn unsigned_predicate(op: InfixOp) -> ICmpPredicateAttr {
    match op {
        InfixOp::Eq => ICmpPredicateAttr::EQ,
        InfixOp::Ne => ICmpPredicateAttr::NE,
        InfixOp::Lt => ICmpPredicateAttr::ULT,
        InfixOp::Le => ICmpPredicateAttr::ULE,
        InfixOp::Gt => ICmpPredicateAttr::UGT,
        InfixOp::Ge => ICmpPredicateAttr::UGE,
        other => unreachable!("`{other:?}` is not a comparison"),
    }
}

fn float_predicate(op: InfixOp) -> FCmpPredicateAttr {
    match op {
        InfixOp::Eq => FCmpPredicateAttr::OEQ,
        // Rust `!=` on f64 is true for NaN operands: unordered-or-unequal.
        InfixOp::Ne => FCmpPredicateAttr::UNE,
        InfixOp::Lt => FCmpPredicateAttr::OLT,
        InfixOp::Le => FCmpPredicateAttr::OLE,
        InfixOp::Gt => FCmpPredicateAttr::OGT,
        InfixOp::Ge => FCmpPredicateAttr::OGE,
        other => unreachable!("`{other:?}` is not a comparison"),
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
