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
//! semantics. Checked scalar traps branch to per-category blocks that call
//! the runtime's `mjrt_trap` with the [`TrapCategory`] code (the runtime
//! reports on stderr and exits `64 + code`). Everything outside the scalar
//! subset produces a contextual [`PlironError`].

use std::collections::{HashMap, HashSet};

use pliron::basic_block::BasicBlock;
use pliron::builtin::attributes::{BytesAttr, FPDoubleAttr, FPSingleAttr, IntegerAttr, StringAttr};
use pliron::builtin::op_interfaces::{
    CallOpCallable, OneResultInterface, SingleBlockRegionInterface,
};
use pliron::builtin::ops::ModuleOp;
use pliron::builtin::types::{FP32Type, FP64Type, IntegerType, Signedness};
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::location::{Located, Location};
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::r#type::{TypeHandle, TypedHandle};
use pliron::utils::apint::{APInt, bw};
use pliron::value::Value;
use pliron_llvm::attributes::{
    FCmpPredicateAttr, FastmathFlagsAttr, ICmpPredicateAttr, IntegerOverflowFlagsAttr, LinkageAttr,
};
use pliron_llvm::op_interfaces::{
    AlignableOpInterface, BinArithOp, CastOpInterface, CastOpWithNNegInterface, FastMathFlags,
    FloatBinArithOpWithFastMathFlags, IntBinArithOpWithOverflowFlag,
};
use pliron_llvm::ops::{
    AShrOp, AddOp, AddressOfOp, AllocaOp, AndOp, BrOp, CallIntrinsicOp, CallOp, CondBrOp,
    ConstantOp, FAddOp, FCmpOp, FDivOp, FMulOp, FNegOp, FPExtOp, FPTruncOp, FSubOp, FuncOp,
    GepIndex, GetElementPtrOp, GlobalOp, ICmpOp, LShrOp, LoadOp, MulOp, OrOp, ReturnOp, SDivOp,
    SExtOp, SIToFPOp, SRemOp, SelectOp, ShlOp, StoreOp, SubOp, TruncOp, UDivOp, UIToFPOp, URemOp,
    UnreachableOp, XorOp, ZExtOp,
};
use pliron_llvm::types::{ArrayType, FuncType, PointerType, VoidType};

use crate::ast::{Dtype, InfixOp, PrefixOp};
use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::checked::CheckedConst;
use crate::literal::{FloatLiteral, IntLiteral};
use crate::mir::{
    Const as MirConst, MirBlock, MirBlockId, MirFunction, MirFunctionDeclaration, MirInstr,
    MirPlace, MirStructDeclaration, MirTerm, Proj, Reg, UseMode,
};
use crate::token::SourceSpan;
use crate::types::Ty;

use crate::native::layout::{Layout, LayoutCx};
use crate::native::mangle;

use super::{PlironError, PlironErrorKind, RetKind, TrapCategory};

/// The callable identity of one reachable function: its mangled symbol,
/// LLVM-dialect function type, and lowered parameter/result kinds. Built by
/// [`declare_function`], consumed by call lowering. An aggregate-returning
/// function takes a prepended sret out-pointer and returns `void`; aggregate
/// parameters pass by pointer (the shared ABI's by-reference rule).
pub(super) struct FnSignature {
    pub mangled: String,
    pub func_ty: TypedHandle<FuncType>,
    pub returns_value: bool,
    pub params: Vec<LowerTy>,
    pub ret: RetKind,
    /// The layout of the aggregate return, when the function returns one
    /// through a prepended out-pointer.
    pub sret: Option<Layout>,
    /// The tagged-outcome ABI of a `raises` function, which returns through a
    /// prepended outcome out-pointer instead of a plain sret (the ok payload
    /// lives inline in the outcome; a function never receives both).
    pub outcome: Option<OutcomeAbi>,
    /// Whether each parameter is consuming — the callee takes ownership and
    /// destroys the value, so passing an owned temporary transfers it.
    pub owned_params: Vec<bool>,
    /// Whether each parameter is a `mut`/`ref` reference: it passes as a
    /// pointer to the caller's storage and the callee's slot aliases it
    /// (write-through), so the final value is visible to the caller.
    pub ref_params: Vec<bool>,
    /// Whether the receiver (parameter 0) is a `deinit` destructor receiver —
    /// its final state writes back to the caller's receiver place, exactly
    /// like a `mut` receiver.
    pub deinit_receiver: bool,
}

/// The `{ tag: u32, ok: T, err: MjError }` outcome of a raising function,
/// laid out by the ordinary aggregate rules (`native::layout::outcome_layout`;
/// the tag sits at offset 0).
#[derive(Clone)]
pub(super) struct OutcomeAbi {
    pub layout: Layout,
    pub ok_offset: u64,
    pub err_offset: u64,
    /// The lowered ok payload (`ZeroSized` for a `None` return).
    pub ok: LowerTy,
}

/// Module-level lowering state shared by every function: the module itself
/// plus the lazily declared runtime-contract symbols (trap blocks call
/// `mjrt_trap`) and the emitted `mjrt_pow` helper. The `mjrt_` prefix, like
/// `main`, is outside the injective `mj_` mangle image (see `mangle`).
pub(super) struct ModuleShared {
    module: ModuleOp,
    rt_types: HashMap<&'static str, TypedHandle<FuncType>>,
    strings: HashMap<Vec<u8>, Identifier>,
    pow_ty: Option<TypedHandle<FuncType>>,
}

impl ModuleShared {
    pub(super) fn new(module: ModuleOp) -> ModuleShared {
        ModuleShared {
            module,
            rt_types: HashMap::new(),
            strings: HashMap::new(),
            pow_ty: None,
        }
    }

    /// Intern `bytes` as a private constant-pool global `mjstr_<n>` —
    /// deduplicated by content and numbered in first-interning order, so
    /// emission is deterministic — and return its symbol. The bytes are the
    /// whole constant: UTF-8, not NUL-terminated (`MjStrDesc` carries the
    /// length).
    fn intern_string(&mut self, ctx: &mut Context, bytes: &[u8]) -> Identifier {
        if let Some(name) = self.strings.get(bytes) {
            return name.clone();
        }
        let name: Identifier = format!("mjstr_{}", self.strings.len())
            .try_into()
            .expect("constant-pool names are identifier-safe");
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let array_ty: TypeHandle = ArrayType::get(ctx, i8_ty, bytes.len() as u64).into();
        let global = GlobalOp::new(ctx, name.clone(), array_ty);
        global.set_initializer_value(ctx, Box::new(BytesAttr::new(bytes.to_vec())));
        global.set_attr_llvm_global_linkage(ctx, LinkageAttr::PrivateLinkage);
        self.module.append_operation(ctx, global.get_operation(), 0);
        self.strings.insert(bytes.to_vec(), name.clone());
        name
    }

    /// Declare the runtime-contract symbol `symbol` (a `rt_abi::RT_SYMBOLS`
    /// row) once and return its call type. The declared LLVM signature comes
    /// mechanically from the contract table's `CAbiTy` row.
    fn ensure_rt(&mut self, ctx: &mut Context, symbol: &'static str) -> TypedHandle<FuncType> {
        if let Some(ty) = self.rt_types.get(symbol) {
            return *ty;
        }
        let sig = crate::native::rt_abi::find_symbol(symbol)
            .unwrap_or_else(|| panic!("`{symbol}` is not in the runtime contract table"));
        let ret = match sig.ret {
            Some(ty) => c_abi_type(ctx, ty),
            None => VoidType::get(ctx).to_handle(),
        };
        let mut params = Vec::with_capacity(sig.params.len());
        for (_, ty) in sig.params {
            params.push(c_abi_type(ctx, *ty));
        }
        let func_ty = FuncType::get(ctx, ret, params, false);
        let identifier: Identifier = sig
            .symbol
            .try_into()
            .expect("runtime symbols are identifier-safe");
        let func = FuncOp::new(ctx, identifier, func_ty);
        self.module.append_operation(ctx, func.get_operation(), 0);
        self.rt_types.insert(symbol, func_ty);
        func_ty
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
/// shell to `module`. Scalars pass and return by value; aggregates pass by
/// pointer and return through a prepended sret out-pointer.
pub(super) fn declare_function(
    ctx: &mut Context,
    module: ModuleOp,
    name: &str,
    func: &MirFunction,
    layout: &LayoutCx<'_>,
) -> Result<(FuncOp, FnSignature), PlironError> {
    let ret_ty = func.ret_ty.as_ref().ok_or_else(|| PlironError {
        function: Some(name.to_string()),
        kind: PlironErrorKind::Unsupported {
            construct: "function without a recorded return type".into(),
        },
        location: None,
    })?;
    let (result, returns_value, ret, sret, outcome) = if func.returns_reference {
        if func.raises {
            return Err(PlironError {
                function: Some(name.to_string()),
                kind: PlironErrorKind::Unsupported {
                    construct: "raising reference-returning function".into(),
                },
                location: None,
            });
        }
        // A reference returns as one pointer to caller-owned referent
        // storage; the checked return type names the referent.
        (
            PointerType::get(ctx, 0).into(),
            true,
            RetKind::Ptr,
            None,
            None,
        )
    } else if func.raises {
        // A raising function returns `void` through a prepended outcome
        // out-pointer; its ok payload (any kind) lives inline in the outcome.
        let ok = lower_ty(name, ret_ty, layout, None)?;
        let composed = layout.outcome_layout(ret_ty).map_err(|error| PlironError {
            function: Some(name.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("raising return of `{ret_ty}` ({error})"),
            },
            location: None,
        })?;
        let outcome = OutcomeAbi {
            layout: composed.layout,
            ok_offset: composed.offsets[1],
            err_offset: composed.offsets[2],
            ok,
        };
        (
            VoidType::get(ctx).to_handle(),
            false,
            RetKind::Void,
            None,
            Some(outcome),
        )
    } else {
        match lower_ty(name, ret_ty, layout, None)? {
            LowerTy::ZeroSized => (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                None,
                None,
            ),
            LowerTy::Scalar(scalar) => (scalar.handle(ctx), true, scalar.ret_kind(), None, None),
            LowerTy::Aggregate { layout, .. } => (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                Some(layout),
                None,
            ),
        }
    };
    let mut params = Vec::with_capacity(func.param_types.len());
    let mut param_handles = Vec::with_capacity(func.param_types.len() + 1);
    if sret.is_some() || outcome.is_some() {
        param_handles.push(PointerType::get(ctx, 0).into());
    }
    for (index, ty) in func.param_types.iter().enumerate() {
        let lowered = lower_ty(name, ty, layout, None)?;
        let by_reference = func.ref_params.get(index).copied().unwrap_or(false);
        match &lowered {
            // A `mut`/`ref` parameter passes as a pointer to the caller's
            // storage regardless of kind; the callee slot aliases it.
            _ if by_reference => param_handles.push(PointerType::get(ctx, 0).into()),
            LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
            LowerTy::Aggregate { .. } => param_handles.push(PointerType::get(ctx, 0).into()),
            LowerTy::ZeroSized => {
                return Err(PlironError {
                    function: Some(name.to_string()),
                    kind: PlironErrorKind::Unsupported {
                        construct: "zero-sized parameter".into(),
                    },
                    location: None,
                });
            }
        }
        params.push(lowered);
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
            sret,
            outcome,
            owned_params: func.owned_params.clone(),
            ref_params: func.ref_params.clone(),
            deinit_receiver: func.deinit_params.first().copied().unwrap_or(false),
        },
    ))
}

/// The module-wide read-only lowering environment shared by every function
/// body: compiled signatures, call-binding declarations, and the span
/// locator.
pub(super) struct LowerEnv<'a> {
    pub signatures: &'a HashMap<String, FnSignature>,
    pub declarations: &'a HashMap<String, MirFunctionDeclaration>,
    pub struct_decls: &'a HashMap<&'a str, &'a MirStructDeclaration>,
    pub layout: LayoutCx<'a>,
    pub locator: &'a Locator,
    /// Emit `mjrt_trace` lifecycle events (test lane only).
    pub trace_lifecycle: bool,
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
        struct_decls: env.struct_decls,
        layout: env.layout,
        locator: env.locator,
        shared,
        trace_lifecycle: env.trace_lifecycle,
        reg_values: HashMap::new(),
        pending_literals: HashMap::new(),
        str_consts: HashMap::new(),
        str_runtime: HashMap::new(),
        owned_temps: HashMap::new(),
        last_uses: HashMap::new(),
        position: (0, 0),
        erased: HashSet::new(),
        partially_moved: HashSet::new(),
        pointer_slot_refs: HashSet::new(),
        drop_flags: HashMap::new(),
        var_slots: Vec::new(),
        blocks: Vec::new(),
        function_blocks: Vec::new(),
        try_frames: Vec::new(),
        finally_states: Vec::new(),
        exit_sites: Vec::new(),
        finally_overrides: Vec::new(),
        pending_ret: None,
        falloff_target: None,
        next_region_block: 0,
        trap_blocks: HashMap::new(),
        region: None,
        entry: None,
        scratch: None,
        sret_ptr: None,
        outcome_ptr: None,
        err_slot: None,
        propagate_block: None,
        current: None,
    };
    lowering.run(ctx, func_op)
}

/// Synthesize the executable's C `main`: reference the linked runtime's
/// version entry point (`mjrt_version`, keeping the inspectable
/// `mjrt_abi_version` data symbol in every produced binary), call each
/// (void, zero-arg) callee in order, then return `0: i32`. Callees are
/// already-mangled native symbols.
pub(super) fn synthesize_exe_wrapper(
    ctx: &mut Context,
    module: ModuleOp,
    callees: &[(String, Option<(Layout, u64)>)],
) -> Result<(), PlironError> {
    let void = VoidType::get(ctx).to_handle();
    let i32_ty: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
    let i32_int = IntegerType::get(ctx, 32, Signedness::Signless);
    let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
    let i64_ty: TypeHandle = i64_int.into();
    let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
    let version_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let version = FuncOp::new(
        ctx,
        "mjrt_version".try_into().expect("valid identifier"),
        version_ty,
    );
    module.append_operation(ctx, version.get_operation(), 0);
    // A raising entry propagates its error out to the wrapper, which reports
    // it as the unhandled-error trap (stderr text and exit 69 unchanged from
    // an in-callee report).
    let unhandled_ty = FuncType::get(ctx, void, vec![ptr_ty, i64_ty], false);
    if callees.iter().any(|(_, outcome)| outcome.is_some()) {
        let unhandled = FuncOp::new(
            ctx,
            "mjrt_unhandled_error".try_into().expect("valid identifier"),
            unhandled_ty,
        );
        module.append_operation(ctx, unhandled.get_operation(), 0);
    }
    let wrapper_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let wrapper = FuncOp::new(
        ctx,
        "main".try_into().expect("valid identifier"),
        wrapper_ty,
    );
    module.append_operation(ctx, wrapper.get_operation(), 0);
    let entry = wrapper.get_or_create_entry_block(ctx);
    let region = wrapper
        .get_operation()
        .deref(ctx)
        .regions()
        .next()
        .expect("llvm.func has a body region");
    let mut current = entry;

    let version_call = CallOp::new(
        ctx,
        CallOpCallable::Direct("mjrt_version".try_into().expect("valid identifier")),
        version_ty,
        vec![],
    );
    version_call.get_operation().insert_at_back(current, ctx);
    for (callee, outcome) in callees {
        let identifier: Identifier = callee
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let Some((layout, err_offset)) = outcome else {
            let callee_ty = FuncType::get(ctx, void, vec![], false);
            let call = CallOp::new(ctx, CallOpCallable::Direct(identifier), callee_ty, vec![]);
            call.get_operation().insert_at_back(current, ctx);
            continue;
        };
        let count_attr = IntegerAttr::new(i64_int, APInt::from_u64(layout.size.max(1), bw(64)));
        let count = ConstantOp::new(ctx, Box::new(count_attr));
        count.get_operation().insert_at_back(current, ctx);
        let storage = AllocaOp::new(ctx, i8_ty, count.get_result(ctx));
        storage.set_alignment(ctx, layout.align as u32);
        storage.get_operation().insert_at_back(current, ctx);
        let callee_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(identifier),
            callee_ty,
            vec![storage.get_result(ctx)],
        );
        call.get_operation().insert_at_back(current, ctx);
        let tag = LoadOp::new(ctx, storage.get_result(ctx), i32_ty);
        tag.get_operation().insert_at_back(current, ctx);
        let err_attr = IntegerAttr::new(
            i32_int,
            APInt::from_u64(u64::from(crate::native::rt_abi::MJ_TAG_ERR), bw(32)),
        );
        let err_tag = ConstantOp::new(ctx, Box::new(err_attr));
        err_tag.get_operation().insert_at_back(current, ctx);
        let is_err = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            tag.get_result(ctx),
            err_tag.get_result(ctx),
        );
        is_err.get_operation().insert_at_back(current, ctx);
        let err_block = BasicBlock::new(ctx, None, vec![]);
        err_block.insert_at_back(region, ctx);
        let cont_block = BasicBlock::new(ctx, None, vec![]);
        cont_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_err.get_result(ctx),
            err_block,
            vec![],
            cont_block,
            vec![],
        );
        branch.get_operation().insert_at_back(current, ctx);
        // The error's message is the MjString at the error offset:
        // `{ data, size, cap }`.
        let data_address = if *err_offset == 0 {
            storage.get_result(ctx)
        } else {
            let index = u32::try_from(*err_offset).expect("outcome offsets fit u32");
            let gep = GetElementPtrOp::new(
                ctx,
                storage.get_result(ctx),
                vec![GepIndex::Constant(index)],
                i8_ty,
            );
            gep.get_operation().insert_at_back(err_block, ctx);
            gep.get_result(ctx)
        };
        let data = LoadOp::new(ctx, data_address, ptr_ty);
        data.get_operation().insert_at_back(err_block, ctx);
        let size_index = u32::try_from(*err_offset + 8).expect("outcome offsets fit u32");
        let size_gep = GetElementPtrOp::new(
            ctx,
            storage.get_result(ctx),
            vec![GepIndex::Constant(size_index)],
            i8_ty,
        );
        size_gep.get_operation().insert_at_back(err_block, ctx);
        let size = LoadOp::new(ctx, size_gep.get_result(ctx), i64_ty);
        size.get_operation().insert_at_back(err_block, ctx);
        let report = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_unhandled_error".try_into().expect("valid identifier")),
            unhandled_ty,
            vec![data.get_result(ctx), size.get_result(ctx)],
        );
        report.get_operation().insert_at_back(err_block, ctx);
        let unreachable = UnreachableOp::new(ctx);
        unreachable.get_operation().insert_at_back(err_block, ctx);
        current = cont_block;
    }
    let zero_attr = IntegerAttr::new(i32_int, APInt::from_u64(0, bw(32)));
    let zero = ConstantOp::new(ctx, Box::new(zero_attr));
    zero.get_operation().insert_at_back(current, ctx);
    let ret = ReturnOp::new(ctx, Some(zero.get_result(ctx)));
    ret.get_operation().insert_at_back(current, ctx);
    Ok(())
}

/// Calls the backend lowers as runtime-ABI intrinsics instead of compiling
/// their stdlib bodies: the `std.memory` allocation entry points allocate
/// through element-erased builtins (the VM's slot arena) whose element sizes
/// only exist at the specialized call sites the backend intercepts.
/// `reachable_set` skips these edges so the erased bodies are never declared.
pub(super) fn intercepted_call(name: &str) -> bool {
    name == "__module$std$memory$unsafe_alloc"
        || name.starts_with("__module$std$memory$unsafe_alloc$")
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
/// signless i64 representation and differ only in operator selection; `Ptr`
/// is one opaque target pointer (checked `Pointer` values, origins erased);
/// `Sized` is a width-1 SIMD scalar alias at its lane width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScalarTy {
    Int,
    UInt,
    Float64,
    Bool,
    Ptr,
    /// A sized scalar alias (`Int8`…`Int64`, `UInt8`…`UInt64`, `Float32`).
    /// The `int`, `float64`, and `bool` dtypes canonicalize to the variants
    /// above through [`ScalarTy::of_dtype`] and never appear here.
    Sized(Dtype),
}

impl ScalarTy {
    /// The scalar lowering of a width-1 SIMD dtype.
    fn of_dtype(dtype: Dtype) -> ScalarTy {
        match dtype {
            Dtype::Int => ScalarTy::Int,
            Dtype::Float64 => ScalarTy::Float64,
            Dtype::Bool => ScalarTy::Bool,
            sized => ScalarTy::Sized(sized),
        }
    }

    /// An integer scalar's `(bits, signed)` lane shape — `Int`/`UInt` are
    /// 64-bit signed/unsigned, sized integers use the VM's lane table — or
    /// `None` for floats, `Bool`, and `Ptr`.
    fn int_shape(self) -> Option<(u32, bool)> {
        match self {
            ScalarTy::Int => Some((64, true)),
            ScalarTy::UInt => Some((64, false)),
            ScalarTy::Sized(dtype) => crate::runtime::integer_dtype_bits(dtype),
            _ => None,
        }
    }

    fn handle(self, ctx: &mut Context) -> TypeHandle {
        match self {
            ScalarTy::Int | ScalarTy::UInt => {
                IntegerType::get(ctx, 64, Signedness::Signless).into()
            }
            ScalarTy::Float64 => FP64Type::get(ctx).into(),
            ScalarTy::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
            ScalarTy::Ptr => PointerType::get(ctx, 0).into(),
            ScalarTy::Sized(Dtype::Float32) => FP32Type::get(ctx).into(),
            ScalarTy::Sized(dtype) => {
                let (bits, _) = crate::runtime::integer_dtype_bits(dtype)
                    .expect("of_dtype leaves only sized integers and Float32 in Sized");
                IntegerType::get(ctx, bits, Signedness::Signless).into()
            }
        }
    }

    fn ret_kind(self) -> RetKind {
        match self {
            ScalarTy::Int => RetKind::I64,
            ScalarTy::UInt => RetKind::U64,
            ScalarTy::Float64 => RetKind::F64,
            ScalarTy::Bool => RetKind::Bool,
            ScalarTy::Ptr => RetKind::Ptr,
            ScalarTy::Sized(dtype) => RetKind::Sized(dtype),
        }
    }

    fn name(self) -> &'static str {
        match self {
            ScalarTy::Int => "Int",
            ScalarTy::UInt => "UInt",
            ScalarTy::Float64 => "Float64",
            ScalarTy::Bool => "Bool",
            ScalarTy::Ptr => "Pointer",
            ScalarTy::Sized(dtype) => dtype.scalar_alias().unwrap_or_else(|| dtype.name()),
        }
    }
}

/// The backend lowering of one checked type: scalars stay SSA values,
/// aggregates are memory-resident — registers hold a pointer to storage laid
/// out by the shared `native::layout` engine — and zero-sized values have no
/// runtime representation. The aggregate's checked type is boxed to keep the
/// enum scalar-cheap.
#[derive(Clone)]
pub(super) enum LowerTy {
    Scalar(ScalarTy),
    Aggregate { ty: Box<Ty>, layout: Layout },
    ZeroSized,
}

/// Classify a checked type for lowering: scalars (including width-1 SIMD
/// aliases and the i64/f64 storage of `IntLiteral`/`FloatLiteral`-typed
/// registers) stay SSA; `None` is zero-sized; struct, tuple, and
/// `StringLiteral`-descriptor aggregates take their shared-engine layout;
/// everything else (multi-lane SIMD, packs, callables) stays outside the
/// supported subset with a contextual rejection.
fn lower_ty(
    function: &str,
    ty: &Ty,
    layout: &LayoutCx<'_>,
    location: Option<SourceSpan>,
) -> Result<LowerTy, PlironError> {
    match ty {
        Ty::Int => Ok(LowerTy::Scalar(ScalarTy::Int)),
        Ty::UInt => Ok(LowerTy::Scalar(ScalarTy::UInt)),
        Ty::Float64 => Ok(LowerTy::Scalar(ScalarTy::Float64)),
        Ty::Bool => Ok(LowerTy::Scalar(ScalarTy::Bool)),
        Ty::Simd { dtype, width: 1 } => Ok(LowerTy::Scalar(ScalarTy::of_dtype(*dtype))),
        // Literal-typed storage holds the default materialized value; a
        // constant that exceeds it rejects at the storage boundary rather
        // than wrapping (the VM keeps arbitrary precision).
        Ty::IntLiteral => Ok(LowerTy::Scalar(ScalarTy::Int)),
        Ty::FloatLiteral => Ok(LowerTy::Scalar(ScalarTy::Float64)),
        // Origins and ownership facts erase after validation; a pointer is
        // one opaque target pointer regardless of its element type.
        Ty::Pointer { .. } | Ty::Ref(_) => Ok(LowerTy::Scalar(ScalarTy::Ptr)),
        Ty::None => Ok(LowerTy::ZeroSized),
        // The built-in error value is `MjError { message: MjString }` storage;
        // its message buffer frees invisibly on drop (no user destructor).
        // `StringLiteral` storage is the borrowed `MjStrDesc` descriptor.
        Ty::Error | Ty::Struct(..) | Ty::Tuple(_) | Ty::RuntimePack(_) | Ty::StringLiteral => {
            match layout.layout_of(ty) {
                Ok(computed) => Ok(LowerTy::Aggregate {
                    ty: Box::new(ty.clone()),
                    layout: computed,
                }),
                Err(error) => Err(PlironError {
                    function: Some(function.to_string()),
                    kind: PlironErrorKind::Unsupported {
                        construct: format!("type `{ty:?}` ({error})"),
                    },
                    location,
                }),
            }
        }
        other => Err(PlironError {
            function: Some(function.to_string()),
            kind: PlironErrorKind::Unsupported {
                construct: format!("type `{other:?}`"),
            },
            location,
        }),
    }
}

/// A register holding a not-yet-materialized literal. Kept exact until a
/// consumer fixes the target type, mirroring the VM's literal values.
#[derive(Clone)]
enum PendingLiteral {
    Int(IntLiteral),
    Float(FloatLiteral),
}

// Registers holding a compile-time string literal carry their exact bytes
// (`str_consts`); consumers intern into the constant pool only on actual use,
// so fold-only literals never emit a global.

/// A register holding a runtime string as a `(data, len)` SSA pair — the
/// in-flight form of `MjStrDesc`. An `owned` descriptor carries a dedicated
/// `mjrt_alloc` allocation the consuming String constructor steals.
#[derive(Clone, Copy)]
struct RuntimeStr {
    data: Value,
    len: Value,
    owned: bool,
}

/// One enclosing `try` during lowering: the landing block its raise edge
/// jumps to, the body-local cleanup variables the VM drops whenever the
/// body region is left (raise, normal completion, return, or escape), and
/// the pending-outcome state when the try has a `finally` region. Handler
/// and `else` regions lower under a pseudo-frame (empty cleanup — it
/// already ran on the body edge) so their raises and returns still route
/// through the `finally`.
struct TryFrame {
    landing: Ptr<BasicBlock>,
    cleanup: Vec<u32>,
    /// Index into `FnLowering::finally_states` when this try has `finally`.
    finally: Option<usize>,
    /// Whether a raise landing on this frame pends an error on its
    /// finalbody (a handler-less `try`/`finally` body, or a handler/orelse
    /// pseudo-frame) — recorded on demand so a raise-free body emits no
    /// error dispatch.
    pends_error: bool,
}

/// The pending-outcome machinery of one `finally`-bearing try. The finalbody
/// lowers once; every incoming edge records what was pending in `kind_slot`
/// (0 normal, 1 error, `2 + site` for an exit site) and the post-finalbody
/// dispatch forwards the outcome — a finalbody-internal raise, return, or
/// escape simply never reaches the dispatch, which is the VM's "finally
/// outcome wins".
struct FinallyState {
    /// Forwarding block every pending edge jumps to (branches to the lowered
    /// finalbody entry).
    entry: Ptr<BasicBlock>,
    /// The post-finalbody dispatch block (the finalbody's `FallOff` target).
    dispatch: Ptr<BasicBlock>,
    /// Stages a raise into this finally: copies the staged error into
    /// `pending_err`, tags `kind_slot` 1, and enters the finalbody.
    error_entry: Ptr<BasicBlock>,
    /// `i32` pending-kind alloca.
    kind_slot: Value,
    /// Per-try pending `MjError` storage (the shared staging slot may be
    /// clobbered by a raise handled inside the finalbody itself).
    pending_err: Value,
    /// Exit-site codes that can pend on this frame (registered as sites
    /// cross it; complete before the finalbody lowers, since every setter
    /// sits in the body/handler/orelse regions lowered first).
    codes: Vec<u32>,
    /// Whether any edge can pend an error here (a handler-less raise edge,
    /// or a raise inside the handler/orelse). When false the dispatch and
    /// resolutions skip the error case — nonraising functions have no
    /// propagation path to re-raise into.
    error_possible: bool,
    /// The try's continuation for the dispatch's normal case.
    after: Ptr<BasicBlock>,
}

/// One function-exit site that may cross `finally` regions: a return
/// terminator (its value staged at the site, its carried cleanup run at the
/// terminal after every pending finalbody) or a `break`/`continue` escape to
/// a function-level block.
struct ExitSiteInfo {
    action: ExitAction,
    /// `finally_states` indices whose pending outcome this site overrides
    /// (the site sits inside those finalbodies); resolved at the terminal
    /// for returns — the VM merges the overridden return's cleanup roots
    /// into the overriding return's frame-end cleanup.
    overrides: Vec<usize>,
    /// Lazily created terminal block (dispatch chains target it).
    terminal: Option<Ptr<BasicBlock>>,
}

enum ExitAction {
    Return { cleanup: Vec<u32> },
    Escape { target: usize },
}

struct FnLowering<'a> {
    name: &'a str,
    func: &'a MirFunction,
    signatures: &'a HashMap<String, FnSignature>,
    declarations: &'a HashMap<String, MirFunctionDeclaration>,
    struct_decls: &'a HashMap<&'a str, &'a MirStructDeclaration>,
    layout: LayoutCx<'a>,
    locator: &'a Locator,
    shared: &'a mut ModuleShared,
    /// Emit `mjrt_trace` lifecycle events (test lane only).
    trace_lifecycle: bool,
    /// Materialized SSA value of each register.
    reg_values: HashMap<u32, Value>,
    /// Registers holding a not-yet-materialized literal.
    pending_literals: HashMap<u32, PendingLiteral>,
    /// Registers holding a compile-time string literal (its exact bytes).
    str_consts: HashMap<u32, Vec<u8>>,
    /// Registers holding a runtime string as a `(data, len)` SSA pair.
    str_runtime: HashMap<u32, RuntimeStr>,
    /// Registers holding an owned heap-carrying temporary (a `clone_value`
    /// copy, a constructed String, or an owned runtime string) that no
    /// variable owns. The VM never destroys register temporaries — its arena
    /// makes that free — so the native release is invisible bookkeeping:
    /// heap buffers free directly after the temporary's last use, and no
    /// user destructor runs.
    owned_temps: HashMap<u32, Ty>,
    /// `(block, instruction)` of each register's final operand appearance;
    /// terminator uses record `usize::MAX`.
    last_uses: HashMap<u32, (usize, usize)>,
    /// The `(block, instruction)` currently being lowered.
    position: (usize, usize),
    /// Registers written by semantically erased instructions (analysis
    /// markers, void call results). Reading one is an internal invariant
    /// violation surfaced as a diagnostic, never a silent miscompile.
    erased: HashSet<u32>,
    /// Variables a `MovePlace` moved a projection out of. Dropping such a
    /// variable must skip the moved parts (the VM tombstones them); when the
    /// drop would emit destructor work, lowering rejects instead of guessing.
    partially_moved: HashSet<u32>,
    /// `MakeRef` results that address pointer-typed storage: a value access
    /// through such a handle first loads the stored pointer (the VM's
    /// reference-pointer boundary). A plain pointer value (a loaded pointer
    /// field) needs no extra dereference, so the distinction is per
    /// register, not per type.
    pointer_slot_refs: HashSet<u32>,
    /// The `i1` initialization flag of each droppable variable. Drop
    /// elaboration legitimately drops not-yet-initialized slots (ahead of
    /// `try` regions) and lists variables on cleanup edges they already died
    /// before; the VM's empty-slot drop is a silent no-op, so native drops
    /// guard on the flag to be no-op-safe the same way. Set on `DefVar` and
    /// whole-variable stores, cleared on drop/consume/move-out; parameters
    /// start initialized.
    drop_flags: HashMap<u32, Value>,
    /// Alloca (pointer) value of each variable slot; aggregate parameter
    /// slots alias the incoming pointer (write-through).
    var_slots: Vec<Value>,
    /// Pliron block for each MIR block id — the enclosing function's blocks
    /// outside `try` regions, a region's local mini-CFG blocks while one
    /// lowers (regions share the register/variable space but have local
    /// block ids).
    blocks: Vec<Ptr<BasicBlock>>,
    /// The function-level block map regardless of region nesting —
    /// `EscapeJump` targets enclosing-function blocks.
    function_blocks: Vec<Ptr<BasicBlock>>,
    /// Enclosing `try` frames, innermost last: a raise edge jumps to the
    /// innermost landing block, and a return or escape crossing a body
    /// region runs each enclosing frame's cleanup drops (inner to outer —
    /// the VM runs `Try.cleanup` whenever the body region is left).
    try_frames: Vec<TryFrame>,
    /// Pending-outcome state of each `finally`-bearing try (arena — frames
    /// and sites refer by index).
    finally_states: Vec<FinallyState>,
    /// Function-exit sites that cross `finally` regions.
    exit_sites: Vec<ExitSiteInfo>,
    /// `finally_states` indices whose finalbody is lexically being lowered,
    /// innermost last: a raise/return/escape inside a finalbody overrides
    /// that pending outcome and must resolve it (drop the pending return's
    /// carried roots, free a pending error).
    finally_overrides: Vec<usize>,
    /// Staging slot for a scalar return value crossing `finally` regions.
    pending_ret: Option<Value>,
    /// Where a region's `FallOff` terminator jumps.
    falloff_target: Option<Ptr<BasicBlock>>,
    /// The next synthetic block id in the last-use position space; region
    /// blocks take ids past the function's own, assigned in exactly the
    /// order `record_last_uses` assigns them.
    next_region_block: usize,
    /// Lazily created per-category trap blocks of this function.
    trap_blocks: HashMap<u8, Ptr<BasicBlock>>,
    /// The function's body region (for appending guard/trap blocks).
    region: Option<Ptr<pliron::region::Region>>,
    /// The function's entry block (for prepending lazily created storage).
    entry: Option<Ptr<BasicBlock>>,
    /// The lazily created 32-byte `mjrt_fmt_*` scratch buffer.
    scratch: Option<Value>,
    /// The aggregate-return out-pointer (argument 0), when the function has
    /// one.
    sret_ptr: Option<Value>,
    /// The tagged-outcome out-pointer (argument 0) of a raising function.
    outcome_ptr: Option<Value>,
    /// The entry-block MjError staging slot: raise sites and propagating
    /// call edges write the in-flight error here before jumping to the
    /// current raise-edge target. Created lazily.
    err_slot: Option<Value>,
    /// The per-function propagate block of a raising function: copies the
    /// staged error into the outcome's error slot, tags it, releases the
    /// heap buffers of still-initialized releasable locals (no user
    /// destructor — the VM abandons raising frames), and returns. Created
    /// lazily.
    propagate_block: Option<Ptr<BasicBlock>>,
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
        self.entry = Some(entry);

        // One pliron block per MIR block (entry stays separate so MIR block 0
        // may have predecessors).
        for _ in 0..self.func.blocks.len() {
            let block = BasicBlock::new(ctx, None, vec![]);
            block.insert_at_back(region, ctx);
            self.blocks.push(block);
        }
        self.function_blocks = self.blocks.clone();
        self.next_region_block = self.func.blocks.len();

        // Entry: one alloca per variable slot, parameter stores, then a jump
        // to MIR block 0. An aggregate-returning function receives its sret
        // out-pointer as argument 0, shifting every parameter right by one;
        // aggregate parameter slots alias the incoming pointer directly
        // (write-through — `out`/`mut` receivers mutate caller storage), so
        // they allocate nothing.
        self.current = Some(entry);
        let signature = &self.signatures[self.name];
        let arg_offset = usize::from(signature.sret.is_some() || signature.outcome.is_some());
        if signature.outcome.is_some() {
            self.outcome_ptr = Some(entry.deref(ctx).get_argument(0));
        } else if signature.sret.is_some() {
            self.sret_ptr = Some(entry.deref(ctx).get_argument(0));
        }
        let param_tys: Vec<Option<LowerTy>> = (0..self.func.n_vars)
            .map(|var| {
                (var < self.func.n_params).then(|| self.signatures[self.name].params[var].clone())
            })
            .collect();
        let ref_params = self.signatures[self.name].ref_params.clone();
        let param_by_pointer = |var: usize, param_ty: &Option<LowerTy>| {
            matches!(param_ty, Some(LowerTy::Aggregate { .. }))
                || (param_ty.is_some() && ref_params.get(var).copied().unwrap_or(false))
        };
        let one = self.int_constant(ctx, 1);
        for (var, param_ty) in param_tys.iter().enumerate() {
            match param_ty {
                // Aggregate and `mut`/`ref` parameter slots alias the
                // incoming pointer (write-through).
                _ if param_by_pointer(var, param_ty) => {
                    let incoming = entry.deref(ctx).get_argument(arg_offset + var);
                    self.var_slots.push(incoming);
                }
                _ => {
                    let slot = match self.var_lower_ty(var as u32)? {
                        LowerTy::Scalar(scalar) => {
                            let handle = scalar.handle(ctx);
                            let alloca = AllocaOp::new(ctx, handle, one);
                            self.append(ctx, alloca.get_operation(), None);
                            alloca.get_result(ctx)
                        }
                        LowerTy::Aggregate { layout, .. } => {
                            self.entry_alloca(ctx, layout.size, layout.align)
                        }
                        LowerTy::ZeroSized => {
                            return Err(self.unsupported(
                                format!(
                                    "zero-sized variable `{}`",
                                    self.func
                                        .var_names
                                        .get(var)
                                        .map(String::as_str)
                                        .unwrap_or("?")
                                ),
                                None,
                            ));
                        }
                    };
                    self.var_slots.push(slot);
                }
            }
        }
        for (param, param_ty) in param_tys.iter().take(self.func.n_params).enumerate() {
            if param_by_pointer(param, param_ty) {
                continue;
            }
            let value = entry.deref(ctx).get_argument(arg_offset + param);
            let store = StoreOp::new(ctx, value, self.var_slots[param]);
            self.append(ctx, store.get_operation(), None);
        }
        // Every droppable variable gets an initialization flag (parameters
        // arrive bound, everything else starts empty). See `drop_flags`.
        let i1: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        for var in 0..self.func.n_vars {
            let LowerTy::Aggregate { ty, layout } = self.var_lower_ty(var as u32)? else {
                continue;
            };
            if !self.needs_drop(&ty) {
                continue;
            }
            // Local droppable storage zeroes at entry (parameters alias
            // caller storage and arrive initialized).
            if var >= self.func.n_params {
                let slot = self.var_slots[var];
                self.mem_zero(ctx, slot, layout.size);
            }
            let alloca = AllocaOp::new(ctx, i1, one);
            self.append(ctx, alloca.get_operation(), None);
            let init = self.bool_constant(ctx, var < self.func.n_params);
            let store = StoreOp::new(ctx, init, alloca.get_result(ctx));
            self.append(ctx, store.get_operation(), None);
            self.drop_flags.insert(var as u32, alloca.get_result(ctx));
        }
        let jump = BrOp::new(ctx, self.blocks[0], vec![]);
        self.append(ctx, jump.get_operation(), None);

        // Final operand appearances drive the owned-temporary releases. The
        // walk recurses into `try` regions with synthetic block ids assigned
        // in exactly the order region lowering assigns them.
        let function_ids: Vec<usize> = (0..self.func.blocks.len()).collect();
        let mut next_id = self.func.blocks.len();
        record_last_uses(
            &mut self.last_uses,
            self.func.blocks.as_slice(),
            &function_ids,
            &mut next_id,
        );

        for (id, block) in self.func.blocks.iter().enumerate() {
            self.current = Some(self.blocks[id]);
            for (index, instr) in block.instrs.iter().enumerate() {
                self.position = (id, index);
                self.lower_instr(ctx, instr)?;
                self.flush_owned_temps(ctx)?;
            }
            self.position = (id, usize::MAX);
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
            MirInstr::UseVar { dest, var, mode } => self.lower_use_var(ctx, *dest, *var, *mode),
            MirInstr::DefVar { var, src, .. } => self.lower_def_var(ctx, *var, *src),
            MirInstr::LoadPlace { dest, place } => {
                let (address, ty) = self.place_address(ctx, place, *dest)?;
                self.load_from(ctx, address, &ty, *dest)
            }
            MirInstr::MovePlace { dest, place } => {
                // Moving out of a projection leaves the variable partially
                // initialized; record it so a later whole-variable drop can
                // refuse destructor work instead of double-freeing.
                if !place.proj.is_empty() {
                    self.partially_moved.insert(place.root);
                }
                let (address, ty) = self.place_address(ctx, place, *dest)?;
                self.load_from(ctx, address, &ty, *dest)
            }
            MirInstr::Store { place, src } => {
                // The VM overwrites the designated storage without dropping
                // the old value (drop elaboration emits explicit drops), so a
                // plain store/copy is exact.
                let (address, ty) = self.place_address(ctx, place, *src)?;
                self.store_to(ctx, address, &ty, *src)?;
                // A whole-variable store (re)initializes the slot.
                if place.proj.is_empty() && place.through.is_none() {
                    self.set_drop_flag(ctx, place.root, true);
                }
                Ok(())
            }
            MirInstr::GetField { dest, base, field } => {
                self.lower_get_field(ctx, *dest, *base, field)
            }
            MirInstr::MakeTuple {
                dest,
                elems,
                element_types,
            } => self.lower_make_tuple(ctx, *dest, elems, element_types.as_deref()),
            MirInstr::MethodCall {
                dest,
                recv,
                method,
                resolved,
                raises,
                reference_result,
                result_adapter,
                args,
                kwargs,
                recv_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                ..
            } => {
                // The callee's compiled signature is authoritative for the
                // raising and reference-result ABI.
                let _ = (raises, reference_result);
                if result_adapter.is_some() {
                    return Err(
                        self.unsupported_reg("reference-result method adapter".into(), *dest)
                    );
                }
                // Erased type-parameter slots (`value: None`) carry no
                // runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                if kwarg_places.iter().any(Option::is_some)
                    || !capture_accesses.is_empty()
                    || param_arg_regs.iter().any(|arg| arg.value.is_some())
                {
                    return Err(self.unsupported_reg(
                        format!("non-positional method contract for `{method}`"),
                        *dest,
                    ));
                }
                self.lower_method_call(
                    ctx,
                    *dest,
                    *recv,
                    method,
                    resolved.as_deref(),
                    args,
                    kwargs,
                    arg_places,
                    recv_place.as_ref(),
                )
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
                // The callee's compiled signature is authoritative for the
                // raising ABI.
                let _ = raises;
                // Erased type-parameter slots (`value: None`) carry no
                // runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                if kwarg_places.iter().any(Option::is_some)
                    || !capture_accesses.is_empty()
                    || param_arg_regs.iter().any(|arg| arg.value.is_some())
                {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                self.lower_call(ctx, *dest, &func.0, args, kwargs, arg_places)
            }
            MirInstr::DropVar { var } => self.lower_drop_var(ctx, *var),
            MirInstr::ConsumeVar { var } => self.lower_consume_var(ctx, *var),
            MirInstr::ConsumePlace { place, marker } => {
                // Consumption skips the whole-value destructor and destroys
                // only residual fields — a no-op unless fields carry their
                // own destructor work.
                let ty = place
                    .ty
                    .clone()
                    .or_else(|| place.root_ty.clone())
                    .ok_or_else(|| self.unsupported("untyped consumed place".into(), None))?;
                if self.fields_need_drop(&ty) {
                    return Err(
                        self.unsupported("place consumption with droppable fields".into(), None)
                    );
                }
                self.erased.insert(marker.0);
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
            MirInstr::CopyValue { dest, value } => self.lower_copy_value(ctx, *dest, *value),
            // Pointer subscripts are runtime intrinsics; every other
            // subscript form routes through nominal `__getitem__` calls the
            // subset does not lower yet.
            MirInstr::Index {
                dest,
                base,
                index,
                intrinsic: Some(crate::mir::MirIntrinsicSubscript::Pointer),
                ..
            } => self.lower_pointer_index(ctx, *dest, *base, *index),
            // References are place addresses: ownership verified the
            // discipline; the backend materializes plain pointers.
            MirInstr::MakeRef { dest, place } => self.lower_make_ref(ctx, *dest, place),
            MirInstr::ReadRef { dest, reference } => self.lower_read_ref(ctx, *dest, *reference),
            MirInstr::WriteRef { reference, value } => {
                self.lower_write_ref(ctx, *reference, *value)
            }
            MirInstr::StoreRef { place, reference } => {
                let place = place.clone();
                let (address, _) = self.place_address(ctx, &place, *reference)?;
                let handle = self.reg_value(ctx, *reference, ScalarTy::Ptr)?;
                let store = StoreOp::new(ctx, handle, address);
                self.append(ctx, store.get_operation(), Some(*reference));
                Ok(())
            }
            // Everything below is outside the supported subset. Every variant
            // is named so that new instructions force a decision here.
            MirInstr::HasNext { dest, iter, method } => {
                self.lower_has_next(ctx, *dest, *iter, method.as_deref())
            }
            MirInstr::Next { dest, iter, call } => {
                self.lower_next(ctx, *dest, *iter, call.as_ref())
            }
            MirInstr::TryNext {
                dest,
                yielded,
                iter,
                call,
                exhaustion: _,
            } => self.lower_try_next(ctx, *dest, *yielded, *iter, call),
            MirInstr::MakeClosure { dest, .. }
            | MirInstr::CallIndirect { dest, .. }
            | MirInstr::Index { dest, .. }
            | MirInstr::Slice { dest, .. }
            | MirInstr::MultiIndex { dest, .. }
            | MirInstr::MakeVariant { dest, .. }
            | MirInstr::VariantIs { dest, .. }
            | MirInstr::VariantGet { dest, .. }
            | MirInstr::VariantTake { dest, .. }
            | MirInstr::VariantReplace { dest, .. }
            | MirInstr::SimdShuffle { dest, .. }
            | MirInstr::PointerStorageTake { dest, .. }
            | MirInstr::UninitStorage { dest, .. }
            | MirInstr::UninitStorageTake { dest, .. } => {
                Err(self.unsupported_reg(format!("instruction `{}`", instr_name(instr)), *dest))
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => self.lower_make_simd(ctx, *dest, *dtype, *width, elems),
            MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
            } => self.lower_simd_cast(ctx, *dest, *value, *dtype, *width),
            MirInstr::Raise { src } => self.lower_raise(ctx, *src),
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                cleanup,
            } => self.lower_try(
                ctx,
                body,
                handler.as_ref(),
                orelse.as_deref(),
                finalbody.as_deref(),
                cleanup,
            ),
            MirInstr::GetIter {
                source,
                dest,
                mode: _,
                prepare,
            } => self.lower_get_iter(ctx, *source, *dest, prepare),
            MirInstr::VariantSet { .. }
            | MirInstr::VariantSetInitWith { .. }
            | MirInstr::VariantDeinitWith { .. }
            | MirInstr::MultiSet { .. }
            | MirInstr::PointerStorageDestroy { .. }
            | MirInstr::UninitStorageDestroy { .. }
            | MirInstr::Drop { .. } => {
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
        arg_places: &[Option<MirPlace>],
    ) -> Result<(), PlironError> {
        if intercepted_call(name) {
            return self.lower_unsafe_alloc(ctx, dest, args, kwargs);
        }
        if !self.signatures.contains_key(name) {
            if matches!(name, "Int" | "UInt" | "Float64" | "Bool") {
                return self.lower_convert(ctx, dest, name, args, kwargs);
            }
            if name == "print" {
                if !kwargs.is_empty() {
                    return Err(
                        self.unsupported_reg("print call with keyword arguments".into(), dest)
                    );
                }
                return self.lower_print(ctx, dest, args);
            }
            if name == "String" {
                return self.lower_string_builtin(ctx, dest, args, kwargs);
            }
            if name == "Error" {
                return self.lower_error_builtin(ctx, dest, args, kwargs);
            }
            if self.struct_decls.contains_key(name) {
                return self.lower_constructor(ctx, dest, name, args, kwargs);
            }
            return Err(self.unsupported_reg(
                format!("call to unknown or builtin function `{name}`"),
                dest,
            ));
        }

        let params = self.signatures[name].params.clone();
        let owned = self.signatures[name].owned_params.clone();
        let by_reference = self.signatures[name].ref_params.clone();
        let lowered_args = if kwargs.is_empty() && args.len() == params.len() {
            let mut lowered = Vec::with_capacity(args.len());
            for (i, (arg, expected)) in args.iter().zip(&params).enumerate() {
                let owned = owned.get(i).copied().unwrap_or(false);
                let value = if by_reference.get(i).copied().unwrap_or(false) {
                    // A `mut`/`ref` argument passes the address of the
                    // caller's designated storage (write-through).
                    let Some(place) = arg_places.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("`mut`/`ref` argument of `{name}` without a place"),
                            dest,
                        ));
                    };
                    let place = place.clone();
                    self.place_address(ctx, &place, dest)?.0
                } else {
                    self.arg_value(ctx, *arg, expected, owned, dest)?
                };
                lowered.push(value);
            }
            lowered
        } else {
            if by_reference.iter().any(|&by_ref| by_ref) {
                return Err(self.unsupported_reg(
                    format!("keyword-bound `mut`/`ref` argument of `{name}`"),
                    dest,
                ));
            }
            self.bind_call_slots(ctx, dest, name, &params, &owned, args, kwargs)?
        };
        self.emit_bound_call(ctx, dest, name, lowered_args)
    }

    /// Resolve keyword arguments and constant defaults into the callee's
    /// positional parameter order via `call::match_call_slots` — the same
    /// structural binding the VM applies (`src/call.rs` owns the policy).
    /// `params` is the expected slice of value parameters: a method or
    /// constructor caller passes its signature minus the receiver.
    #[allow(clippy::too_many_arguments)]
    fn bind_call_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        params: &[LowerTy],
        owned: &[bool],
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
        if matched.slots.len() != params.len() {
            return Err(self.unsupported_reg(
                format!("call binding for `{name}` disagrees with its compiled arity"),
                dest,
            ));
        }
        let defaults = decl.defaults.clone();
        let mut lowered = Vec::with_capacity(params.len());
        for (i, slot) in matched.slots.iter().enumerate() {
            let expected = params[i].clone();
            let owned = owned.get(i).copied().unwrap_or(false);
            let value = match slot {
                ArgSlot::Positional(p) => self.arg_value(ctx, args[*p], &expected, owned, dest)?,
                ArgSlot::Keyword(k) => self.arg_value(ctx, kwargs[*k].1, &expected, owned, dest)?,
                ArgSlot::Default => {
                    let Some(default) = defaults.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("non-constant default argument in call to `{name}`"),
                            dest,
                        ));
                    };
                    let LowerTy::Scalar(scalar) = expected else {
                        return Err(self.unsupported_reg(
                            format!("non-scalar default argument in call to `{name}`"),
                            dest,
                        ));
                    };
                    self.checked_const_value(ctx, default, scalar, dest)?
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

        let source = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A runtime literal-typed value converts at its storage kind
            // (its constant was range-checked when it entered storage).
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
                Some(Ty::IntLiteral) => ScalarTy::Int,
                _ => return Err(self.unsupported_reg("untyped conversion operand".into(), dest)),
            },
        };
        let value = self.reg_value(ctx, arg, source)?;
        // A sized operand converts through its mathematical lane value (the
        // VM's `builtin_convert` width-1 arm): integers sign/zero-extend to
        // i64, a `Float32` converts through its f64 view. The normalized
        // kind then takes the ordinary scalar conversion arms.
        let (source, value) = match source {
            ScalarTy::Sized(Dtype::Float32) => {
                (ScalarTy::Float64, self.f32_to_f64(ctx, value, dest))
            }
            ScalarTy::Sized(dtype) => {
                let (_, signed) =
                    crate::runtime::integer_dtype_bits(dtype).expect("Float32 is matched above");
                let wide = self.sized_to_i64(ctx, value, dtype, dest);
                (
                    if signed {
                        ScalarTy::Int
                    } else {
                        ScalarTy::UInt
                    },
                    wide,
                )
            }
            other => (other, value),
        };
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
            (ScalarTy::Ptr, _) | (_, ScalarTy::Ptr) => {
                Err(self
                    .unsupported_reg(format!("conversion `{name}` over a Pointer operand"), dest))
            }
            (ScalarTy::Sized(_), _) | (_, ScalarTy::Sized(_)) => {
                unreachable!("sized sources normalize above; conversion targets are builtins")
            }
        }
    }

    /// `MakeSimd` at width 1 — scalar-alias construction (`Int8(x)`,
    /// `Float32(x)`, `SIMD[DType.<dt>, 1](x)`): convert the single element
    /// with the VM's lane builders (`runtime::value_to_int_lane`/
    /// `value_to_float_lane`). Multi-lane construction stays out of the
    /// subset until the SIMD slice.
    fn lower_make_simd(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        dtype: Dtype,
        width: usize,
        elems: &[Reg],
    ) -> Result<(), PlironError> {
        if width != 1 || elems.len() != 1 {
            return Err(self.unsupported_reg(
                format!("multi-lane SIMD construction (width {width})"),
                dest,
            ));
        }
        let elem = elems[0];
        let target = ScalarTy::of_dtype(dtype);
        // A literal element folds with the exact conversions (integers wrap
        // at the lane width, `Float32` rounds from the exact rational).
        if let Some(literal) = self.pending_literals.get(&elem.0).cloned() {
            let constant = self.materialize_pending(ctx, &literal, target, dest)?;
            self.reg_values.insert(dest.0, constant);
            return Ok(());
        }
        let source = match self.concrete_scalar_ty(elem)? {
            Some(ty) => ty,
            None => match self.func.reg_types.get(&elem.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, elem, source)?;
        let converted = self.convert_lane(ctx, source, target, value, dest)?;
        self.reg_values.insert(dest.0, converted);
        Ok(())
    }

    /// `SimdCast` at width 1 (`x.cast[DType.<dt>]()`) — the VM's
    /// `runtime::simd_cast`: int→int rewraps at the new width, int→float
    /// converts through f64 (`Float32` rounds), float→float widens or
    /// rounds, and float→int truncates toward zero saturating at the
    /// 128-bit intermediate before wrapping — saturation must happen at
    /// i128, not the target width, or large magnitudes wrap differently
    /// than the VM. Bool casts reject (VM parity); multi-lane casts stay
    /// out of the subset.
    fn lower_simd_cast(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    ) -> Result<(), PlironError> {
        if width != 1 {
            return Err(self.unsupported_reg(format!("multi-lane SIMD cast (width {width})"), dest));
        }
        if dtype == Dtype::Bool {
            return Err(self.unsupported_reg("bool SIMD dtype cast".into(), dest));
        }
        let source = self.concrete_scalar_ty(value)?.ok_or_else(|| {
            self.unsupported_reg("SIMD cast of an unmaterialized literal".into(), dest)
        })?;
        if matches!(source, ScalarTy::Bool | ScalarTy::Ptr) {
            return Err(
                self.unsupported_reg(format!("SIMD cast of a {} operand", source.name()), dest)
            );
        }
        let target = ScalarTy::of_dtype(dtype);
        let lane = self.reg_value(ctx, value, source)?;
        let converted = match target {
            ScalarTy::Float64 => self.lane_to_f64(ctx, source, lane, dest)?,
            ScalarTy::Sized(Dtype::Float32) => {
                let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                self.f64_to_f32(ctx, wide, dest)
            }
            integer => {
                let (to_bits, _) = integer
                    .int_shape()
                    .expect("bool targets are rejected above");
                match source.int_shape() {
                    Some(from) => self.resize_int(ctx, lane, from, to_bits, dest),
                    // Float source: truncate toward zero, saturating at the
                    // 128-bit intermediate (Rust `as i128`, NaN → 0), then
                    // wrap to the lane width.
                    None => {
                        let wide = self.lane_to_f64(ctx, source, lane, dest)?;
                        let saturated = self.fptosi_sat_i128(ctx, wide, dest);
                        self.resize_int(ctx, saturated, (128, true), to_bits, dest)
                    }
                }
            }
        };
        self.reg_values.insert(dest.0, converted);
        Ok(())
    }

    /// One scalar value as a `target` SIMD lane — the VM's lane builders:
    /// integer lanes wrap the source's mathematical value at the lane width
    /// (`value_to_int_lane`; Bool reads as 0/1), float lanes convert through
    /// f64 with `Float32` rounding (`value_to_float_lane`), bool lanes only
    /// accept Bool. Sources the VM cannot read as the lane's kind reject.
    fn convert_lane(
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
    fn lane_to_f64(
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
                let (_, signed) =
                    crate::runtime::integer_dtype_bits(dtype).expect("Float32 is matched above");
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
    fn fptosi_sat_i128(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
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

    /// `UseVar`: scalars load from their slot; aggregate copies run the VM's
    /// `clone_value` semantics and aggregate moves transfer the bytes (the VM
    /// tombstones the source, and ownership analysis rejects later uses
    /// statically, so no runtime tombstone is needed).
    fn lower_use_var(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        mode: UseMode,
    ) -> Result<(), PlironError> {
        if matches!(mode, UseMode::BorrowShared | UseMode::BorrowMut) {
            // A borrow is the address of the variable's storage (the VM's
            // `Value::Ref` handle); ownership already verified the
            // discipline.
            self.reg_values.insert(dest.0, self.var_slots[var as usize]);
            return Ok(());
        }
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, self.var_slots[var as usize], handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, layout } => {
                let src = self.var_slots[var as usize];
                if matches!(mode, UseMode::Copy) {
                    self.copy_aggregate(ctx, dest, &ty, layout, src)
                } else {
                    // The nominal String's `__moveinit__` is an identity
                    // field move — the byte copy below is exactly it.
                    let stdlib_string = matches!(ty.as_ref(), Ty::Struct(name, _)
                        if crate::symbol::is_stdlib_string_struct(name));
                    if !stdlib_string && self.has_lifecycle_method(&ty, "__moveinit__") {
                        return Err(self.unsupported_reg(
                            format!("move of `{ty}` with a user `__moveinit__`"),
                            dest,
                        ));
                    }
                    let storage = self.entry_alloca(ctx, layout.size, layout.align);
                    self.mem_copy(ctx, storage, src, layout.size, dest);
                    self.reg_values.insert(dest.0, storage);
                    // The move vacates the slot (the VM tombstones it); a
                    // later cleanup-edge drop must find it empty.
                    self.set_drop_flag(ctx, var, false);
                    Ok(())
                }
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// `DefVar`: the VM clones the register into the variable slot — for
    /// aggregates a byte copy of the register's storage.
    fn lower_def_var(&mut self, ctx: &mut Context, var: u32, src: Reg) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(expected) => {
                let value = self.literal_slot_value(ctx, var, src, expected)?;
                let store = StoreOp::new(ctx, value, self.var_slots[var as usize]);
                self.append(ctx, store.get_operation(), None);
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let ptr = self.reg_ptr(ctx, src)?;
                let slot = self.var_slots[var as usize];
                self.mem_copy(ctx, slot, ptr, layout.size, src);
                // The variable owns the value now; the temporary transfers.
                self.owned_temps.remove(&src.0);
                self.set_drop_flag(ctx, var, true);
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    /// The SSA value of `src` for a store into variable `var`'s scalar slot:
    /// a pending literal entering `IntLiteral`/`FloatLiteral`-typed storage
    /// converts exactly (rejecting what the storage cannot hold) instead of
    /// wrapping at the consumer's kind.
    fn literal_slot_value(
        &mut self,
        ctx: &mut Context,
        var: u32,
        src: Reg,
        expected: ScalarTy,
    ) -> Result<Value, PlironError> {
        if let Some(ty @ (Ty::IntLiteral | Ty::FloatLiteral)) = self.func.var_tys.get(&var).cloned()
            && let Some(literal) = self.pending_literals.get(&src.0).cloned()
        {
            let constant = self.exact_literal_storage(ctx, &literal, &ty, src)?;
            self.reg_values.insert(src.0, constant);
            return Ok(constant);
        }
        self.reg_value(ctx, src, expected)
    }

    /// `CopyValue` — materialize an owned copy of a register: scalars and
    /// compile-time literals alias (their SSA values are already owned);
    /// aggregates run the VM's `clone_value` copy.
    fn lower_copy_value(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        value: Reg,
    ) -> Result<(), PlironError> {
        if self.pointer_slot_refs.contains(&value.0) {
            self.pointer_slot_refs.insert(dest.0);
        }
        if let Some(literal) = self.pending_literals.get(&value.0).cloned() {
            self.pending_literals.insert(dest.0, literal);
            return Ok(());
        }
        if let Some(bytes) = self.str_consts.get(&value.0).cloned() {
            self.str_consts.insert(dest.0, bytes);
            return Ok(());
        }
        let Some(ty) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg(format!("untyped copy source %r{}", value.0), dest));
        };
        match lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let copied = self.reg_value(ctx, value, scalar)?;
                self.reg_values.insert(dest.0, copied);
                Ok(())
            }
            LowerTy::Aggregate { ty, layout } => {
                let src = self.reg_ptr(ctx, value)?;
                self.copy_aggregate(ctx, dest, &ty, layout, src)
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// The intercepted `std.memory` allocation entry point,
    /// `unsafe_alloc[T](count, *, alignment = 0)`: `mjrt_alloc(count *
    /// sizeof(T), align)` with the element type taken from the call site's
    /// concrete `Pointer[T]` destination. An excessive count traps with the
    /// allocation-failure category (a recorded divergence — the VM raises a
    /// `TypeError` for a negative count).
    fn lower_unsafe_alloc(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(
                self.unsupported_reg("allocation without a concrete pointer result".into(), dest)
            );
        };
        let element_layout = self.layout.layout_of(&element).map_err(|error| {
            self.unsupported_reg(format!("allocation element layout ({error})"), dest)
        })?;
        if args.len() != 1 {
            return Err(self.unsupported_reg("allocation call contract".into(), dest));
        }
        let alignment = match kwargs {
            [] => None,
            [(name, reg)] if name == "alignment" => Some(*reg),
            _ => {
                return Err(self.unsupported_reg("allocation call contract".into(), dest));
            }
        };
        let count = self.reg_value(ctx, args[0], ScalarTy::Int)?;
        // Guard the byte-size multiplication: any count above the safe bound
        // (negative counts arrive as huge unsigned values) traps.
        let element_size = element_layout.size.max(1);
        let limit = self.uint_constant(ctx, u64::MAX / element_size);
        let excessive = ICmpOp::new(ctx, ICmpPredicateAttr::UGT, count, limit);
        self.append(ctx, excessive.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            excessive.get_result(ctx),
            TrapCategory::AllocFailure,
            dest,
        )?;
        let size_const = self.uint_constant(ctx, element_layout.size);
        let bytes = MulOp::new_with_overflow_flag(ctx, count, size_const, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let natural_align = self.uint_constant(ctx, element_layout.align);
        let align = match alignment {
            None => natural_align,
            Some(reg) => {
                // `alignment = 0` selects the element's natural alignment.
                let requested = self.reg_value(ctx, reg, ScalarTy::Int)?;
                let zero = self.int_constant(ctx, 0);
                let is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, requested, zero);
                self.append(ctx, is_zero.get_operation(), Some(dest));
                let select = SelectOp::new(ctx, is_zero.get_result(ctx), natural_align, requested);
                self.append(ctx, select.get_operation(), Some(dest));
                select.get_result(ctx)
            }
        };
        let alloc_ty = self.shared.ensure_rt(ctx, "mjrt_alloc");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_alloc".try_into().expect("valid identifier")),
            alloc_ty,
            vec![bytes.get_result(ctx), align],
        );
        self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
    }

    /// Pointer-receiver method intrinsics — the VM's `Value::Pointer` method
    /// dispatch: `free`/`unsafe_free` release through the runtime's size-less
    /// free. Everything else stays unsupported.
    fn lower_pointer_method(
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

    /// `p[i]` over the pointer subscript intrinsic: load the element at
    /// `p + i * sizeof(element)` — the VM's unchecked heap read.
    fn lower_pointer_index(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
    ) -> Result<(), PlironError> {
        let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&base.0).cloned() else {
            return Err(
                self.unsupported_reg("pointer subscript on a non-pointer base".into(), dest)
            );
        };
        let ptr = self.reg_value(ctx, base, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, &element, dest)?;
        self.load_from(ctx, address, &element, dest)
    }

    /// `pointer + index * sizeof(element)` as an opaque address.
    fn pointer_element_address(
        &mut self,
        ctx: &mut Context,
        pointer: Value,
        index: Reg,
        element: &Ty,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let element_layout = self.layout.layout_of(element).map_err(|error| {
            self.unsupported_reg(format!("pointer element layout ({error})"), dest)
        })?;
        let index_value = self.reg_value(ctx, index, ScalarTy::Int)?;
        let size = self.uint_constant(ctx, element_layout.size);
        let bytes = MulOp::new_with_overflow_flag(ctx, index_value, size, no_overflow_flags());
        self.append(ctx, bytes.get_operation(), Some(dest));
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let gep = GetElementPtrOp::new(
            ctx,
            pointer,
            vec![GepIndex::Value(bytes.get_result(ctx))],
            i8_ty,
        );
        self.append(ctx, gep.get_operation(), Some(dest));
        Ok(gep.get_result(ctx))
    }

    /// An owned copy of an aggregate — the VM's `clone_value`: the nominal
    /// String copies through the native bridge (the stdlib byte loop needs
    /// machinery outside this stage), a struct's compiled `__copyinit__` runs
    /// when it defines one, and otherwise a byte copy applies (exact for
    /// every type whose transitive fields carry no user copy constructor; a
    /// nested-only `__copyinit__` rejects rather than diverge from the VM's
    /// recursive clone).
    fn copy_aggregate(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        ty: &Ty,
        layout: Layout,
        src_ptr: Value,
    ) -> Result<(), PlironError> {
        if let Ty::Struct(name, _) = ty
            && crate::symbol::is_stdlib_string_struct(name)
        {
            // The stdlib copy constructor: a fresh `cap`-byte allocation with
            // `size` bytes copied and `size`/`cap` preserved.
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, dest);
            let src_cap = self.string_cap(ctx, src_ptr, dest);
            let new_data = self.emit_alloc(ctx, src_cap, 1, dest);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, dest);
            self.store_string_fields(ctx, storage, new_data, src_size, src_cap, dest);
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        }
        if matches!(ty, Ty::Error) {
            // The VM's clone of an error duplicates its message, so the copy
            // outlives the original's drop.
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, dest);
            let new_data = self.emit_alloc(ctx, src_size, 1, dest);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, dest);
            self.store_string_fields(ctx, storage, new_data, src_size, src_size, dest);
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        }
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        if let Ty::Struct(name, _) = ty
            && self
                .declarations
                .contains_key(&format!("{name}.__copyinit__"))
        {
            let copyinit = format!("{name}.__copyinit__");
            let Some(signature) = self.signatures.get(&copyinit) else {
                return Err(self.unsupported_reg(format!("copy via uncompiled `{copyinit}`"), dest));
            };
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported_reg(format!("raising copy constructor `{copyinit}`"), dest)
                );
            }
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            // `__copyinit__(out self, copy: Self)`: dest storage, then source.
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                func_ty,
                vec![storage, src_ptr],
            );
            self.append(ctx, call.get_operation(), Some(dest));
            // A user copy constructor may have allocated; release only what
            // the invisible-release rule understands (String buffers).
            if self.releasable(ty) {
                self.mark_owned_temp(dest, ty.clone())?;
            }
        } else if self.has_nested_lifecycle(ty, "__copyinit__") {
            return Err(self.unsupported_reg(
                format!("copy of `{ty}` with a nested user `__copyinit__`"),
                dest,
            ));
        } else {
            // A byte copy shares any heap its fields point at (the VM's plain
            // clone does too); the owning variable's drop releases it, so the
            // temporary must not.
            self.mem_copy(ctx, storage, src_ptr, layout.size, dest);
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Whether the invisible-release rule can free every heap buffer a value
    /// of `ty` owns without running user code: the nominal String (one
    /// buffer), and byte-copied aggregates over such fields.
    fn releasable(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Error => true,
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => true,
            Ty::Struct(name, _) => {
                !self
                    .declarations
                    .contains_key(&format!("{name}.__copyinit__"))
                    && self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                        decl.fields
                            .iter()
                            .all(|(_, field)| !self.owns_heap(field) || self.releasable(field))
                    })
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .all(|element| !self.owns_heap(element) || self.releasable(element)),
            _ => !self.owns_heap(ty),
        }
    }

    /// Whether a value of `ty` semantically owns heap memory (the nominal
    /// String's buffer; raw pointers are not owned).
    fn owns_heap(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Error => true,
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => true,
            Ty::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.owns_heap(field))),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().any(|element| self.owns_heap(element))
            }
            _ => false,
        }
    }

    /// Record `dest` as an owned heap-carrying temporary, released after its
    /// final use in this block. A temporary whose final use sits in another
    /// block would need liveness analysis — reject instead of leaking.
    fn mark_owned_temp(&mut self, dest: Reg, ty: Ty) -> Result<(), PlironError> {
        if !self.owns_heap(&ty) && !matches!(ty, Ty::StringLiteral) {
            return Ok(());
        }
        if let Some((block, _)) = self.last_uses.get(&dest.0)
            && *block != self.position.0
        {
            return Err(self.unsupported_reg(
                "owned heap-carrying temporary used across blocks".into(),
                dest,
            ));
        }
        self.owned_temps.insert(dest.0, ty);
        Ok(())
    }

    /// Release every owned temporary whose final use was the instruction just
    /// lowered (or that is never used at all).
    fn flush_owned_temps(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        let due: Vec<(u32, Ty)> = self
            .owned_temps
            .iter()
            .filter(|(reg, _)| match self.last_uses.get(reg) {
                None => true,
                Some(last) => *last == self.position,
            })
            .map(|(reg, ty)| (*reg, ty.clone()))
            .collect();
        for (reg, ty) in due {
            self.owned_temps.remove(&reg);
            self.emit_release_reg(ctx, reg, &ty)?;
        }
        Ok(())
    }

    /// Free the heap buffers register `reg` (an owned temporary) carries,
    /// without running any user destructor — mirroring the VM, which never
    /// destroys register temporaries.
    fn emit_release_reg(
        &mut self,
        ctx: &mut Context,
        reg: u32,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        if matches!(ty, Ty::StringLiteral) {
            let Some(descriptor) = self.str_runtime.get(&reg).copied() else {
                return Ok(());
            };
            self.emit_free(ctx, descriptor.data);
            return Ok(());
        }
        let Some(storage) = self.reg_values.get(&reg).copied() else {
            return Ok(());
        };
        self.emit_release_storage(ctx, storage, ty)
    }

    /// Recursively free the owned heap buffers inside `ty`-typed storage.
    fn emit_release_storage(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
    ) -> Result<(), PlironError> {
        match ty {
            // A String's buffer and an error's message buffer both sit at
            // offset 0 (MjString/MjError agree on the data-first layout).
            Ty::Error => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            Ty::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let fields = decl.fields.clone();
                let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
                let composed = self
                    .layout
                    .struct_layout(&field_tys)
                    .map_err(|error| self.unsupported(format!("release layout ({error})"), None))?;
                for (position, field_ty) in field_tys.iter().enumerate() {
                    if !self.owns_heap(field_ty) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_release_storage(ctx, address, field_ty)?;
                }
                Ok(())
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self
                    .layout
                    .struct_layout(&elements)
                    .map_err(|error| self.unsupported(format!("release layout ({error})"), None))?;
                for (position, element) in elements.iter().enumerate() {
                    if !self.owns_heap(element) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_release_storage(ctx, address, element)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `mjrt_free(ptr)`.
    fn emit_free(&mut self, ctx: &mut Context, ptr: Value) {
        let free_ty = self.shared.ensure_rt(ctx, "mjrt_free");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_free".try_into().expect("valid identifier")),
            free_ty,
            vec![ptr],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// Load the `(data, size)` fields of nominal-String storage.
    fn string_parts(&mut self, ctx: &mut Context, ptr: Value, dest: Reg) -> (Value, Value) {
        let ptr_handle = ScalarTy::Ptr.handle(ctx);
        let data = LoadOp::new(ctx, ptr, ptr_handle);
        self.append(ctx, data.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, ptr, 8, dest);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let size = LoadOp::new(ctx, size_address, i64_handle);
        self.append(ctx, size.get_operation(), Some(dest));
        (data.get_result(ctx), size.get_result(ctx))
    }

    /// Load the `cap` field of nominal-String storage.
    fn string_cap(&mut self, ctx: &mut Context, ptr: Value, dest: Reg) -> Value {
        let cap_address = self.gep_byte(ctx, ptr, 16, dest);
        let i64_handle = ScalarTy::Int.handle(ctx);
        let cap = LoadOp::new(ctx, cap_address, i64_handle);
        self.append(ctx, cap.get_operation(), Some(dest));
        cap.get_result(ctx)
    }

    /// Store `{data, size, cap}` into nominal-String storage.
    fn store_string_fields(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        data: Value,
        size: Value,
        cap: Value,
        dest: Reg,
    ) {
        let data_store = StoreOp::new(ctx, data, storage);
        self.append(ctx, data_store.get_operation(), Some(dest));
        let size_address = self.gep_byte(ctx, storage, 8, dest);
        let size_store = StoreOp::new(ctx, size, size_address);
        self.append(ctx, size_store.get_operation(), Some(dest));
        let cap_address = self.gep_byte(ctx, storage, 16, dest);
        let cap_store = StoreOp::new(ctx, cap, cap_address);
        self.append(ctx, cap_store.get_operation(), Some(dest));
    }

    /// `mjrt_alloc(size, align)` with a runtime byte count.
    fn emit_alloc(&mut self, ctx: &mut Context, size: Value, align: u64, dest: Reg) -> Value {
        let alloc_ty = self.shared.ensure_rt(ctx, "mjrt_alloc");
        let align_value = self.uint_constant(ctx, align);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_alloc".try_into().expect("valid identifier")),
            alloc_ty,
            vec![size, align_value],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        call.get_result(ctx)
    }

    /// `llvm.memset.p0.i64(dest, 0, len, volatile=false)`: zero storage.
    /// Droppable variable slots zero at entry so a flag-guarded drop or
    /// release path never reads undefined bytes, and the intrinsic use keeps
    /// mem2reg from promoting a slot whose stores sit in since-pruned blocks.
    fn mem_zero(&mut self, ctx: &mut Context, dest: Value, len: u64) {
        if len == 0 {
            return;
        }
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, i8_ty, i64_ty, i1_ty], false);
        let i8_int = IntegerType::get(ctx, 8, Signedness::Signless);
        let zero_attr = IntegerAttr::new(i8_int, APInt::from_u64(0, bw(8)));
        let zero = ConstantOp::new(ctx, Box::new(zero_attr));
        self.append(ctx, zero.get_operation(), None);
        let len_value = self.uint_constant(ctx, len);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memset.p0.i64".to_string()),
            fn_ty,
            vec![dest, zero.get_result(ctx), len_value, volatile],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// `llvm.memcpy` with a runtime byte count.
    fn mem_copy_dynamic(
        &mut self,
        ctx: &mut Context,
        dest_ptr: Value,
        src: Value,
        len: Value,
        span_reg: Reg,
    ) {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, ptr_ty, i64_ty, i1_ty], false);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memcpy.p0.p0.i64".to_string()),
            fn_ty,
            vec![dest_ptr, src, len, volatile],
        );
        self.append(ctx, call.get_operation(), Some(span_reg));
    }

    /// Resolve a place to the address and checked type of its designated
    /// storage: the root variable slot plus statically composed field and
    /// tuple-element offsets from the shared layout engine. A pointer
    /// subscript projection loads the pointer value and continues at
    /// `pointer + index * sizeof(element)` — the VM's unchecked heap
    /// addressing.
    fn place_address(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        dest: Reg,
    ) -> Result<(Value, Ty), PlironError> {
        let root_ty = self
            .func
            .var_tys
            .get(&place.root)
            .cloned()
            .or_else(|| place.root_ty.clone())
            .ok_or_else(|| {
                self.unsupported_reg(format!("untyped place root ${}", place.root), dest)
            })?;
        let root_slot = self
            .var_slots
            .get(place.root as usize)
            .copied()
            .ok_or_else(|| {
                self.unsupported_reg(format!("place root ${} out of range", place.root), dest)
            })?;
        // A place through a local reference designates the referent behind
        // the handle stored in the root's slot: load the pointer, then
        // project relative to the referent type.
        let ref_param_root = (place.root as usize) < self.func.n_params
            && self
                .func
                .ref_params
                .get(place.root as usize)
                .copied()
                .unwrap_or(false);
        let (mut ty, mut address) = if place.through.is_some() {
            match &root_ty {
                Ty::Ref(ref_ty) => {
                    let referent = (*ref_ty.referent).clone();
                    let handle = ScalarTy::Ptr.handle(ctx);
                    let load = LoadOp::new(ctx, root_slot, handle);
                    self.append(ctx, load.get_operation(), Some(dest));
                    (referent, load.get_result(ctx))
                }
                // A `mut`/`ref` parameter is typed as its referent and its
                // aliased slot already IS the referent address.
                _ if ref_param_root => (root_ty, root_slot),
                _ => {
                    return Err(self.unsupported_reg(
                        format!("place through non-reference root `{root_ty}`"),
                        dest,
                    ));
                }
            }
        } else {
            (root_ty, root_slot)
        };
        let mut offset: u64 = 0;
        for proj in &place.proj {
            match proj {
                Proj::Field(field) => {
                    let (field_offset, field_ty) = self.field_offset(&ty, field, dest)?;
                    offset += field_offset;
                    ty = field_ty;
                }
                Proj::ConstIndex(index) => {
                    let (Ty::Tuple(elements) | Ty::RuntimePack(elements)) = &ty else {
                        return Err(self
                            .unsupported_reg(format!("tuple-element projection on `{ty}`"), dest));
                    };
                    let elements = elements.clone();
                    let composed = self.struct_layout_of(&elements, dest)?;
                    let Some(element_offset) = composed.offsets.get(*index).copied() else {
                        return Err(self.unsupported_reg(
                            format!("tuple-element projection index {index} out of range"),
                            dest,
                        ));
                    };
                    offset += element_offset;
                    ty = elements[*index].clone();
                }
                Proj::Index(index) => {
                    let Ty::Pointer { element, .. } = &ty else {
                        return Err(
                            self.unsupported_reg(format!("subscript projection on `{ty}`"), dest)
                        );
                    };
                    let element = (**element).clone();
                    // The address so far designates pointer storage; load the
                    // pointer value and address its element.
                    if offset != 0 {
                        address = self.gep_byte(ctx, address, offset, dest);
                        offset = 0;
                    }
                    let ptr_handle = ScalarTy::Ptr.handle(ctx);
                    let load = LoadOp::new(ctx, address, ptr_handle);
                    self.append(ctx, load.get_operation(), Some(dest));
                    address = self.pointer_element_address(
                        ctx,
                        load.get_result(ctx),
                        *index,
                        &element,
                        dest,
                    )?;
                    ty = element;
                }
                other => {
                    return Err(self.unsupported_reg(format!("place projection `{other:?}`"), dest));
                }
            }
        }
        let address = if offset == 0 {
            address
        } else {
            self.gep_byte(ctx, address, offset, dest)
        };
        Ok((address, ty))
    }

    /// The byte offset and checked type of `field` within struct type `ty`.
    fn field_offset(&self, ty: &Ty, field: &str, dest: Reg) -> Result<(u64, Ty), PlironError> {
        let Ty::Struct(name, _) = ty else {
            return Err(self.unsupported_reg(format!("field access on `{ty}`"), dest));
        };
        let Some(decl) = self.struct_decls.get(name.as_str()) else {
            return Err(
                self.unsupported_reg(format!("struct `{name}` without a declaration"), dest)
            );
        };
        let Some(position) = decl.fields.iter().position(|(n, _)| n == field) else {
            return Err(
                self.unsupported_reg(format!("struct `{name}` has no field `{field}`"), dest)
            );
        };
        let field_tys: Vec<Ty> = decl.fields.iter().map(|(_, t)| t.clone()).collect();
        let composed = self.struct_layout_of(&field_tys, dest)?;
        Ok((composed.offsets[position], field_tys[position].clone()))
    }

    /// The composed layout of `fields`, or a contextual rejection.
    fn struct_layout_of(
        &self,
        fields: &[Ty],
        dest: Reg,
    ) -> Result<crate::native::layout::StructLayout, PlironError> {
        self.layout
            .struct_layout(fields)
            .map_err(|error| self.unsupported_reg(format!("aggregate layout ({error})"), dest))
    }

    /// `GetField` on an aggregate-valued register (field reads through places
    /// use `LoadPlace`; this covers direct register bases).
    fn lower_get_field(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        field: &str,
    ) -> Result<(), PlironError> {
        let Some(base_ty) = self.func.reg_types.get(&base.0).cloned() else {
            return Err(self.unsupported_reg(format!("untyped field base %r{}", base.0), dest));
        };
        let (offset, field_ty) = self.field_offset(&base_ty, field, dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let address = if offset == 0 {
            base_ptr
        } else {
            self.gep_byte(ctx, base_ptr, offset, dest)
        };
        self.load_from(ctx, address, &field_ty, dest)
    }

    /// `MakeTuple`: fresh storage with each element stored at its composed
    /// offset (compiler-private heterogeneous pack storage).
    fn lower_make_tuple(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        elems: &[Reg],
        element_types: Option<&[Ty]>,
    ) -> Result<(), PlironError> {
        let elements: Vec<Ty> = match (self.func.reg_types.get(&dest.0), element_types) {
            (Some(Ty::Tuple(es) | Ty::RuntimePack(es)), _) => es.clone(),
            (_, Some(es)) => es.to_vec(),
            _ => {
                return Err(self.unsupported_reg("untyped tuple construction".into(), dest));
            }
        };
        let composed = self.struct_layout_of(&elements, dest)?;
        let storage = self.entry_alloca(ctx, composed.layout.size, composed.layout.align);
        for ((elem, elem_ty), offset) in elems.iter().zip(&elements).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, elem_ty, *elem)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// A resolved method call: the receiver and aggregate arguments pass by
    /// pointer; a `mut self` (or `deinit self`) receiver's final state copies
    /// back to the caller's receiver place afterwards — the VM's
    /// `store_at_call_place` write-back.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn lower_method_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        method: &str,
        resolved: Option<&str>,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        // Pointer receivers dispatch to runtime intrinsics, never to compiled
        // stdlib bodies.
        if matches!(self.func.reg_types.get(&recv.0), Some(Ty::Pointer { .. })) {
            return self.lower_pointer_method(ctx, dest, recv, method, args);
        }
        let Some(resolved) = resolved else {
            return Err(self.unsupported_reg(format!("unresolved method call `{method}`"), dest));
        };
        let Some(signature) = self.signatures.get(resolved) else {
            return Err(
                self.unsupported_reg(format!("method call to uncompiled `{resolved}`"), dest)
            );
        };
        let params = signature.params.clone();
        let owned = signature.owned_params.clone();
        let by_reference = signature.ref_params.clone();
        let deinit_receiver = signature.deinit_receiver;
        if params.is_empty() {
            return Err(self.unsupported_reg(
                format!("method `{resolved}` without a receiver parameter"),
                dest,
            ));
        }
        let recv_owned = owned.first().copied().unwrap_or(false);
        // A `mut`/`ref` receiver with a known place passes the caller's
        // storage address directly (write-through) — copy-in/copy-out would
        // point an escaping interior pointer at the copy. A `read`/`deinit`
        // receiver (or a placeless temporary) keeps the VM's clone-on-read
        // copy.
        let receiver_alias = recv_place.is_some()
            && matches!(
                self.declarations
                    .get(resolved)
                    .and_then(|decl| decl.receiver_convention.as_ref()),
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
        let recv_value = if receiver_alias {
            let place = recv_place.expect("aliased receivers have a place").clone();
            self.place_address(ctx, &place, dest)?.0
        } else {
            self.arg_value(ctx, recv, &params[0], recv_owned, dest)?
        };
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let rest_by_reference = if by_reference.len() > 1 {
            &by_reference[1..]
        } else {
            &[]
        };
        let mut lowered = vec![recv_value];
        if kwargs.is_empty() && args.len() == rest.len() {
            for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                let owned = rest_owned.get(i).copied().unwrap_or(false);
                let value = if rest_by_reference.get(i).copied().unwrap_or(false) {
                    // A `mut`/`ref` argument passes the address of the
                    // caller's designated storage (write-through).
                    let Some(place) = arg_places.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("`mut`/`ref` argument of `{resolved}` without a place"),
                            dest,
                        ));
                    };
                    let place = place.clone();
                    self.place_address(ctx, &place, dest)?.0
                } else {
                    self.arg_value(ctx, *arg, expected, owned, dest)?
                };
                lowered.push(value);
            }
        } else {
            if rest_by_reference.iter().any(|&by_ref| by_ref) {
                return Err(self.unsupported_reg(
                    format!("keyword-bound `mut`/`ref` argument of `{resolved}`"),
                    dest,
                ));
            }
            lowered
                .extend(self.bind_call_slots(ctx, dest, resolved, rest, rest_owned, args, kwargs)?);
        }
        self.emit_bound_call(ctx, dest, resolved, lowered)?;
        // `mut self` (the struct's mut_self_methods set — keyed by either the
        // overload-qualified or the source method name) and named destructors
        // write the receiver back; a missing place means a discarded
        // temporary receiver.
        let write_back = !receiver_alias
            && match self.func.reg_types.get(&recv.0) {
                Some(Ty::Struct(struct_name, _)) => {
                    let is_mut = self
                        .struct_decls
                        .get(struct_name.as_str())
                        .is_some_and(|d| {
                            d.mut_self_methods.contains(resolved)
                                || d.mut_self_methods.contains(method)
                        });
                    is_mut || deinit_receiver
                }
                _ => false,
            };
        if write_back && let Some(place) = recv_place {
            let LowerTy::Aggregate { layout, .. } = &params[0] else {
                return Err(
                    self.unsupported_reg("mutating method on a scalar receiver".into(), dest)
                );
            };
            let size = layout.size;
            let recv_ptr = self.reg_ptr(ctx, recv)?;
            let (address, _) = self.place_address(ctx, place, dest)?;
            self.mem_copy(ctx, address, recv_ptr, size, dest);
        }
        Ok(())
    }

    /// `GetIter`: normalize the iterable variable through its checker-selected
    /// (and mono-retargeted) `__iter__` chain into the iterator variable.
    /// Receiver conventions mirror the VM: a borrowed (`ref`/`mut`) step
    /// aliases the current storage — for step 0 that is the source slot, the
    /// VM's reference-handle seam, so a borrowing iterator roots at the loop
    /// frame — a `read` step passes a plain byte copy (the VM's
    /// `current.clone()`, no lifecycle copy), and an owned (`var`) step
    /// consumes the current storage in place.
    fn lower_get_iter(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        prepare: &[String],
    ) -> Result<(), PlironError> {
        if prepare.is_empty() && source == dest {
            // Identity normalization: the slot already holds the iterator.
            return Ok(());
        }
        let LowerTy::Aggregate {
            layout: dest_layout,
            ..
        } = self.var_lower_ty(dest)?
        else {
            return Err(self.unsupported("non-aggregate iterator variable".into(), None));
        };
        // A borrowed named source binds its slot to a reference handle; load
        // it to reach the iterable's storage, as the VM dereferences for
        // method resolution.
        let source_ty = self.func.var_tys.get(&source).cloned().ok_or_else(|| {
            self.unsupported(
                format!(
                    "untyped variable `{}`",
                    self.func
                        .var_names
                        .get(source as usize)
                        .map(String::as_str)
                        .unwrap_or("?")
                ),
                None,
            )
        })?;
        let (mut current, mut current_ty) = if let Ty::Ref(reference) = &source_ty {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, self.var_slots[source as usize], handle);
            self.append(ctx, load.get_operation(), None);
            (load.get_result(ctx), (*reference.referent).clone())
        } else {
            (self.var_slots[source as usize], source_ty)
        };
        // Whether `current` is a chain temporary this instruction owns (the
        // source variable owns its own storage).
        let mut owns_current = false;
        for selected in prepare {
            let Some(signature) = self.signatures.get(selected) else {
                return Err(self.unsupported(
                    format!("iterator preparation via uncompiled `{selected}`"),
                    None,
                ));
            };
            if signature.outcome.is_some() {
                return Err(
                    self.unsupported(format!("raising iterator preparation `{selected}`"), None)
                );
            }
            let Some(receiver_param) = signature.params.first().cloned() else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without a receiver"),
                    None,
                ));
            };
            let Some(result_layout) = signature.sret else {
                return Err(self.unsupported(
                    format!("iterator preparation `{selected}` without an aggregate result"),
                    None,
                ));
            };
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            let borrowed = signature.ref_params.first().copied().unwrap_or(false)
                || matches!(
                    self.declarations
                        .get(selected)
                        .and_then(|decl| decl.receiver_convention.as_ref()),
                    Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                );
            let owned = signature.owned_params.first().copied().unwrap_or(false);
            let (receiver, release_current) = if borrowed || owned {
                // Aliased or consumed in place; a consumed chain temporary
                // needs no release (the callee destroyed it).
                (current, false)
            } else {
                let LowerTy::Aggregate { layout, .. } = receiver_param else {
                    return Err(self.unsupported(
                        format!("iterator preparation `{selected}` on a scalar receiver"),
                        None,
                    ));
                };
                let copy = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, copy, current, layout.size, Reg(u32::MAX));
                (copy, owns_current)
            };
            let result = self.entry_alloca(ctx, result_layout.size, result_layout.align);
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee),
                func_ty,
                vec![result, receiver],
            );
            self.append(ctx, call.get_operation(), None);
            if release_current {
                // The VM's superseded intermediate drops silently (no user
                // destructor); free its heap invisibly or reject.
                if self.owns_heap(&current_ty) {
                    if self.releasable(&current_ty) {
                        self.emit_release_storage(ctx, current, &current_ty)?;
                    } else {
                        return Err(self.unsupported(
                            format!(
                                "iterator preparation abandoning `{current_ty}` with destructor work"
                            ),
                            None,
                        ));
                    }
                }
            }
            current = result;
            current_ty = self
                .declarations
                .get(selected)
                .map(|decl| decl.ret_ty.clone())
                .ok_or_else(|| {
                    self.unsupported(
                        format!("iterator preparation `{selected}` without declaration facts"),
                        None,
                    )
                })?;
            owns_current = true;
        }
        if prepare.is_empty() && self.owns_heap(&current_ty) {
            // A stepless split binds a plain clone of the source; a byte copy
            // of heap-owning storage would double-release at the two drops.
            return Err(self.unsupported(
                format!("borrowed iteration of `{current_ty}` without a preparation step"),
                None,
            ));
        }
        self.mem_copy(
            ctx,
            self.var_slots[dest as usize],
            current,
            dest_layout.size,
            Reg(u32::MAX),
        );
        self.set_drop_flag(ctx, dest, true);
        Ok(())
    }

    /// `HasNext`: the bounded protocol's pure length read — call the
    /// iterator's `__len__` and compare greater-than-zero. The receiver
    /// passes as a plain byte copy (the VM clones its value for the call).
    fn lower_has_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
        method: Option<&str>,
    ) -> Result<(), PlironError> {
        let Some(method) = method else {
            // Compiler-private pack/comptime storage never reaches the
            // native path with a nominal method.
            return Err(self.unsupported_reg("method-free iterator length read".into(), dest));
        };
        let Some(signature) = self.signatures.get(method) else {
            return Err(
                self.unsupported_reg(format!("iterator length via uncompiled `{method}`"), dest)
            );
        };
        if signature.outcome.is_some() || signature.sret.is_some() {
            return Err(
                self.unsupported_reg(format!("iterator length contract of `{method}`"), dest)
            );
        }
        if signature.ret != RetKind::I64 {
            return Err(self.unsupported_reg(format!("iterator length result of `{method}`"), dest));
        }
        let Some(LowerTy::Aggregate { layout, .. }) = signature.params.first().cloned() else {
            return Err(
                self.unsupported_reg(format!("iterator length receiver of `{method}`"), dest)
            );
        };
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let receiver = self.entry_alloca(ctx, layout.size, layout.align);
        self.mem_copy(
            ctx,
            receiver,
            self.var_slots[iter as usize],
            layout.size,
            dest,
        );
        let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![receiver]);
        self.append(ctx, call.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let has_next = ICmpOp::new(ctx, ICmpPredicateAttr::SGT, call.get_result(ctx), zero);
        self.define(
            ctx,
            dest,
            has_next.get_operation(),
            has_next.get_result(ctx),
        )
    }

    /// `Next`: advance the iterator in place through its non-raising
    /// `__next__(mut self)`. The receiver operand is the iterator variable's
    /// own storage, so the mutation is the write-back; a reference result
    /// binds the returned place pointer, and the `CopyIteratorReference`
    /// adapter reads through it with the VM's lifecycle copy.
    fn lower_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
        call: Option<&crate::checked::CheckedIteratorCall>,
    ) -> Result<(), PlironError> {
        let Some(call) = call else {
            return Err(self.unsupported_reg("method-free iterator advance".into(), dest));
        };
        let signature = self.iterator_next_signature(&call.target, dest)?;
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(
                format!("raising bounded `__next__` `{}`", call.target),
                dest,
            ));
        }
        let receiver = self.var_slots[iter as usize];
        if call.result_adapter.is_some() && signature.ret == RetKind::Ptr {
            // The abstract call promised a value; the concrete target returns
            // a reference — read through it and lifecycle-copy the element.
            let callee: Identifier = signature
                .mangled
                .as_str()
                .try_into()
                .expect("mangled names are identifier-safe");
            let func_ty = signature.func_ty;
            let call_op = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![receiver]);
            self.append(ctx, call_op.get_operation(), Some(dest));
            let element = call_op.get_result(ctx);
            return match lower_ty(
                self.name,
                &call.result_ty,
                &self.layout,
                self.reg_span(dest),
            )? {
                LowerTy::Scalar(scalar) => {
                    let handle = scalar.handle(ctx);
                    let load = LoadOp::new(ctx, element, handle);
                    self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
                }
                LowerTy::Aggregate { ty, layout } => {
                    self.copy_aggregate(ctx, dest, &ty, layout, element)
                }
                LowerTy::ZeroSized => {
                    self.erased.insert(dest.0);
                    Ok(())
                }
            };
        }
        self.emit_bound_call(ctx, dest, &call.target, vec![receiver])
    }

    /// `TryNext`: advance through the raising `__next__` over the tagged
    /// outcome. The error edge is statically the exhaustion edge — the
    /// checker pins `call.raises == Some(exhaustion)`, so any raise out of
    /// the callee is exactly the caught `StopIteration` — it releases the
    /// caught error's message and zeroes the ok payload, leaving `dest`
    /// inert. `yielded` is the ok-tag comparison.
    fn lower_try_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        yielded: Reg,
        iter: u32,
        call: &crate::checked::CheckedIteratorCall,
    ) -> Result<(), PlironError> {
        let signature = self.iterator_next_signature(&call.target, dest)?;
        let Some(outcome) = signature.outcome.clone() else {
            return Err(self.unsupported_reg(
                format!(
                    "non-raising `__next__` `{}` on the raising path",
                    call.target
                ),
                dest,
            ));
        };
        if signature.ret == RetKind::Ptr || call.reference_result.is_some() {
            // Unreachable in practice: a raising reference-returning callee
            // already rejected at declaration.
            return Err(self.unsupported_reg(
                format!("reference-yielding raising `__next__` `{}`", call.target),
                dest,
            ));
        }
        // The exhausted edge leaves zeroed element bytes in `dest`; releasing
        // zeroed heap fields is a null-free no-op, but a user destructor
        // observing zeroed fields would diverge from the VM's inert `None`.
        if self.has_nested_lifecycle(&call.result_ty, "__deinit__") {
            return Err(self.unsupported_reg(
                format!(
                    "iterator element `{}` with a user destructor",
                    call.result_ty
                ),
                dest,
            ));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        let receiver = self.var_slots[iter as usize];
        let call_op = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![storage, receiver],
        );
        self.append(ctx, call_op.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let ok_tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_OK);
        let is_ok = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), ok_tag);
        self.define(ctx, yielded, is_ok.get_operation(), is_ok.get_result(ctx))?;
        let region = self.region.expect("lowering is inside a function");
        let exhausted_block = BasicBlock::new(ctx, None, vec![]);
        exhausted_block.insert_at_back(region, ctx);
        let join_block = BasicBlock::new(ctx, None, vec![]);
        join_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_ok.get_result(ctx),
            join_block,
            vec![],
            exhausted_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        self.current = Some(exhausted_block);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.emit_release_storage(ctx, err_address, &Ty::Error)?;
        let ok_size = match &outcome.ok {
            LowerTy::ZeroSized => 0,
            _ => {
                self.layout
                    .layout_of(&call.result_ty)
                    .map_err(|error| {
                        self.unsupported_reg(format!("iterator element layout ({error})"), dest)
                    })?
                    .size
            }
        };
        if ok_size > 0 {
            let ok_address = self.offset_address(ctx, storage, outcome.ok_offset);
            self.mem_zero(ctx, ok_address, ok_size);
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join_block);
        match outcome.ok {
            LowerTy::Scalar(scalar) => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { .. } => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                // Deliberately not an owned temporary: the following
                // `DefVar` copies the element out, and the zeroed exhausted
                // bytes must never release.
                self.reg_values.insert(dest.0, address);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// The compiled signature of an iterator `__next__` target, requiring the
    /// VM's `mut self` receiver contract.
    fn iterator_next_signature(
        &self,
        target: &str,
        dest: Reg,
    ) -> Result<&FnSignature, PlironError> {
        if !matches!(
            self.declarations
                .get(target)
                .and_then(|decl| decl.receiver_convention.as_ref()),
            Some(crate::ast::ArgConvention::Mut)
        ) {
            return Err(self.unsupported_reg(
                format!("iterator `__next__` `{target}` without a `mut self` receiver"),
                dest,
            ));
        }
        self.signatures
            .get(target)
            .ok_or_else(|| self.unsupported_reg(format!("call to uncompiled `{target}`"), dest))
    }

    /// A constructor call to declared struct `name`: the fieldwise copy form
    /// (`Type(copy=value)`), the compiled `__init__` overload with fresh
    /// storage as its `out self`, or fieldwise per-field stores — the VM's
    /// dispatch order.
    fn lower_constructor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if crate::symbol::is_stdlib_string_struct(name) {
            return self.lower_string_ctor(ctx, dest, args, kwargs);
        }
        let struct_ty = match self.func.reg_types.get(&dest.0) {
            Some(ty @ Ty::Struct(..)) => ty.clone(),
            _ => Ty::Struct(name.to_string(), Vec::new()),
        };
        let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
        let LowerTy::Aggregate { ty, layout } = lowered else {
            return Err(self.unsupported_reg(format!("constructor for `{name}`"), dest));
        };
        if args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy" {
            let src = self.reg_ptr(ctx, kwargs[0].1)?;
            return self.copy_aggregate(ctx, dest, &ty, layout, src);
        }
        if let Some(init) = self.constructor_init(name, args.len()) {
            let params = self.signatures[&init].params.clone();
            let owned = self.signatures[&init].owned_params.clone();
            if params.is_empty() {
                return Err(
                    self.unsupported_reg(format!("`{init}` without an `out self` parameter"), dest)
                );
            }
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let rest = &params[1..];
            let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
            let mut lowered = vec![storage];
            if kwargs.is_empty() && args.len() == rest.len() {
                for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                    let owned = rest_owned.get(i).copied().unwrap_or(false);
                    lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
                }
            } else {
                lowered.extend(
                    self.bind_call_slots(ctx, dest, &init, rest, rest_owned, args, kwargs)?,
                );
            }
            self.emit_bound_call(ctx, dest, &init, lowered)?;
            // `__init__` returns nothing; the constructed value is the
            // storage its `out self` wrote through.
            self.erased.remove(&dest.0);
            self.reg_values.insert(dest.0, storage);
            return Ok(());
        }
        let decl = self.struct_decls[name];
        if !decl.fieldwise_init {
            return Err(self.unsupported_reg(
                format!("constructor for `{name}` without a compiled `__init__`"),
                dest,
            ));
        }
        if !kwargs.is_empty() {
            return Err(self.unsupported_reg(
                format!("keyword arguments in the fieldwise constructor of `{name}`"),
                dest,
            ));
        }
        if args.len() != decl.fields.len() {
            return Err(self.unsupported_reg(
                format!(
                    "fieldwise constructor of `{name}` expects {} arguments, got {}",
                    decl.fields.len(),
                    args.len()
                ),
                dest,
            ));
        }
        let fields: Vec<(String, Ty)> = decl.fields.clone();
        let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
        let composed = self.struct_layout_of(&field_tys, dest)?;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        for ((arg, field_ty), offset) in args.iter().zip(&field_tys).zip(&composed.offsets) {
            let address = if *offset == 0 {
                storage
            } else {
                self.gep_byte(ctx, storage, *offset, dest)
            };
            self.store_to(ctx, address, field_ty, *arg)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// The nominal String constructor — the VM's literal-to-struct bridge
    /// (`materialize_string_struct`): the stdlib body never executes; the
    /// byte buffer fills from the string source instead. A compile-time
    /// literal copies out of the constant pool; an owned runtime string's
    /// allocation is stolen; a borrowed runtime string is copied.
    fn lower_string_ctor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("String constructor contract".into(), dest));
        }
        let source = args[0];
        let storage = self.entry_alloca(ctx, 24, 8);
        if let Some(bytes) = self.str_consts.get(&source.0).cloned() {
            let len = bytes.len() as u64;
            let global = self.shared.intern_string(ctx, &bytes);
            let len_value = self.uint_constant(ctx, len);
            let data = self.emit_alloc(ctx, len_value, 1, dest);
            if len > 0 {
                let literal = self.global_address(ctx, &global, dest);
                self.mem_copy(ctx, data, literal, len, dest);
            }
            self.store_string_fields(ctx, storage, data, len_value, len_value, dest);
        } else if let Some(descriptor) = self.str_runtime.get(&source.0).copied() {
            let data = if descriptor.owned && self.owned_temps.remove(&source.0).is_some() {
                // Steal the dedicated allocation — the temporary transfers
                // into the String.
                descriptor.data
            } else {
                let data = self.emit_alloc(ctx, descriptor.len, 1, dest);
                self.mem_copy_dynamic(ctx, data, descriptor.data, descriptor.len, dest);
                data
            };
            self.store_string_fields(ctx, storage, data, descriptor.len, descriptor.len, dest);
        } else if matches!(self.func.reg_types.get(&source.0), Some(Ty::StringLiteral)) {
            // A runtime StringLiteral value (typed storage): copy the bytes
            // its borrowed descriptor points at.
            let ptr = self.reg_ptr(ctx, source)?;
            let (src_data, len) = self.string_parts(ctx, ptr, dest);
            let data = self.emit_alloc(ctx, len, 1, dest);
            self.mem_copy_dynamic(ctx, data, src_data, len, dest);
            self.store_string_fields(ctx, storage, data, len, len, dest);
        } else {
            return Err(
                self.unsupported_reg("String constructor over an unsupported source".into(), dest)
            );
        }
        self.reg_values.insert(dest.0, storage);
        let ty = self
            .func
            .reg_types
            .get(&dest.0)
            .cloned()
            .unwrap_or_else(|| Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), vec![]));
        self.mark_owned_temp(dest, ty)?;
        Ok(())
    }

    /// The `String(x)` builtin — the VM's `format_value` over one argument:
    /// a literal stays compile-time; scalars format through `mjrt_fmt_*`
    /// into a dedicated allocation (an owned runtime string); a nominal
    /// String reads back as a borrowed runtime string.
    fn lower_string_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("String conversion contract".into(), dest));
        }
        let arg = args[0];
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.str_consts.insert(dest.0, bytes);
            return Ok(());
        }
        if let Some(ty) = self.func.reg_types.get(&arg.0)
            && let Ty::Struct(name, _) = ty
            && crate::symbol::is_stdlib_string_struct(name)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: false,
                },
            );
            return Ok(());
        }
        // A runtime StringLiteral value reads back as a borrowed string.
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::StringLiteral))
            && !self.pending_literals.contains_key(&arg.0)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: false,
                },
            );
            return Ok(());
        }
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A runtime FloatLiteral value rejects — the VM formats its
            // exact rational, which f64 storage cannot reproduce.
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => {
                    if !self.pending_literals.contains_key(&arg.0) {
                        return Err(self.unsupported_reg(
                            "String conversion of a runtime FloatLiteral value".into(),
                            dest,
                        ));
                    }
                    ScalarTy::Float64
                }
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, arg, ty)?;
        let (text, len) = self.format_scalar(ctx, ty, value, dest)?;
        let data = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, data, text, len, dest);
        self.str_runtime.insert(
            dest.0,
            RuntimeStr {
                data,
                len,
                owned: true,
            },
        );
        self.mark_owned_temp(dest, Ty::StringLiteral)?;
        Ok(())
    }

    /// The `Error(x)` builtin. Before Stage 4's tagged outcomes the only
    /// consumer is `Raise`, which reads the message bytes and exits — so an
    /// error value lowers as its message string pair.
    /// The lowered function-return value kind: the reference pointer for a
    /// reference-returning function, else the checked return type's lowering.
    fn return_value_lower(&self) -> Result<Option<LowerTy>, PlironError> {
        if self.func.returns_reference {
            return Ok(Some(LowerTy::Scalar(ScalarTy::Ptr)));
        }
        match self.func.ret_ty.as_ref() {
            Some(Ty::None) | None => Ok(None),
            Some(other) => Ok(Some(lower_ty(self.name, other, &self.layout, None)?)),
        }
    }

    /// `MakeRef`: materialize a reference to a verified place — its address.
    /// A place through a local reference forwards (and extends) the stored
    /// handle.
    fn lower_make_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        place: &MirPlace,
    ) -> Result<(), PlironError> {
        let place = place.clone();
        let (address, ty) = self.place_address(ctx, &place, dest)?;
        if matches!(ty, Ty::Pointer { .. }) {
            self.pointer_slot_refs.insert(dest.0);
        }
        self.reg_values.insert(dest.0, address);
        Ok(())
    }

    /// `ReadRef`: read the referent behind a handle — a scalar load or an
    /// aggregate copy-out (the VM's clone-on-read).
    fn lower_read_ref(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        reference: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // A handle addressing pointer-typed storage dereferences through the
        // stored pointer (the VM's reference-pointer boundary).
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference read".into(), dest));
        };
        self.load_from(ctx, pointer, &ty, dest)
    }

    /// `WriteRef`: write a value through a handle into the referent storage.
    fn lower_write_ref(
        &mut self,
        ctx: &mut Context,
        reference: Reg,
        value: Reg,
    ) -> Result<(), PlironError> {
        let mut pointer = self.reg_value(ctx, reference, ScalarTy::Ptr)?;
        // See `lower_read_ref`: a handle addressing pointer-typed storage
        // dereferences through the stored pointer.
        if self.pointer_slot_refs.contains(&reference.0) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, pointer, handle);
            self.append(ctx, load.get_operation(), Some(value));
            pointer = load.get_result(ctx);
        }
        let Some(ty) = self.func.reg_types.get(&value.0).cloned() else {
            return Err(self.unsupported_reg("untyped reference write".into(), value));
        };
        self.store_to(ctx, pointer, &ty, value)
    }

    /// `mjrt_trace(kind, data, len)` — one ordered lifecycle event (test
    /// lane only; callers guard on `trace_lifecycle`).
    fn emit_trace(&mut self, ctx: &mut Context, kind: u32, data: Value, len: Value) {
        let trace_ty = self.shared.ensure_rt(ctx, "mjrt_trace");
        let kind_value = self.tag_constant(ctx, kind);
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_trace".try_into().expect("valid identifier")),
            trace_ty,
            vec![kind_value, data, len],
        );
        self.append(ctx, call.get_operation(), None);
    }

    /// A lifecycle event with a compile-time payload (a type name).
    fn emit_trace_text(&mut self, ctx: &mut Context, kind: u32, text: &str) {
        let global = self.shared.intern_string(ctx, text.as_bytes());
        let data = self.global_address(ctx, &global, Reg(u32::MAX));
        let len = self.uint_constant(ctx, text.len() as u64);
        self.emit_trace(ctx, kind, data, len);
    }

    /// A lifecycle event carrying the staged error's message.
    fn emit_trace_err_slot(&mut self, ctx: &mut Context, kind: u32) {
        let err_slot = self.ensure_err_slot(ctx);
        let (data, size) = self.string_parts(ctx, err_slot, Reg(u32::MAX));
        self.emit_trace(ctx, kind, data, size);
    }

    /// Free the buffers of still-initialized error-typed locals on a normal
    /// return. Drop elaboration never drops a bound-but-unused handler
    /// error (the VM abandons it to its arena at frame end), so the frame
    /// exit releases it invisibly — error values are never buffer-shared
    /// (copies are deep), and borrowed parameters are excluded (their value
    /// belongs to the caller).
    fn emit_frame_exit_error_releases(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        let mut vars: Vec<u32> = self.drop_flags.keys().copied().collect();
        vars.sort_unstable();
        for var in vars {
            if !matches!(self.func.var_tys.get(&var), Some(Ty::Error)) {
                continue;
            }
            let borrowed_param = (var as usize) < self.func.n_params
                && !self
                    .func
                    .owned_params
                    .get(var as usize)
                    .copied()
                    .unwrap_or(false);
            if borrowed_param {
                continue;
            }
            let flag = self.drop_flags[&var];
            let cont = self.begin_flag_guard(ctx, flag);
            let slot = self.var_slots[var as usize];
            self.emit_release_storage(ctx, slot, &Ty::Error)?;
            self.end_flag_guard(ctx, cont);
        }
        Ok(())
    }

    /// The entry-block MjError staging slot for in-flight errors.
    fn ensure_err_slot(&mut self, ctx: &mut Context) -> Value {
        if let Some(slot) = self.err_slot {
            return slot;
        }
        let slot = self.entry_alloca(ctx, 24, 8);
        self.err_slot = Some(slot);
        slot
    }

    /// The innermost raise-edge target: the innermost enclosing `try`'s
    /// landing block, else the raising function's propagate block. The staged
    /// error must already sit in the err slot when jumping here.
    fn raise_edge_target(&mut self, ctx: &mut Context) -> Result<Ptr<BasicBlock>, PlironError> {
        if let Some(frame) = self.try_frames.last() {
            // A raise landing on a handler-less `try`/`finally` body or a
            // handler/orelse pseudo-frame pends an error on the finalbody.
            if let Some(idx) = frame.finally
                && frame.pends_error
            {
                self.finally_states[idx].error_possible = true;
            }
            return Ok(frame.landing);
        }
        self.ensure_propagate_block(ctx)
    }

    /// The per-function propagate block of a raising function: free the heap
    /// buffers of still-initialized releasable locals (no user destructor
    /// runs — the VM abandons raising frames and its arena reclaims the
    /// memory invisibly; other droppable locals are a recorded leak residue),
    /// move the staged error into the outcome's error slot, tag the outcome,
    /// and return.
    fn ensure_propagate_block(
        &mut self,
        ctx: &mut Context,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        if let Some(block) = self.propagate_block {
            return Ok(block);
        }
        let Some(outcome) = self.signatures[self.name].outcome.clone() else {
            return Err(self.unsupported(
                "raise propagation out of a nonraising function".into(),
                None,
            ));
        };
        let outcome_ptr = self
            .outcome_ptr
            .expect("raising functions receive an outcome pointer");
        let err_slot = self.ensure_err_slot(ctx);
        let region = self.region.expect("lowering is inside a function");
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        let saved = self.current;
        self.current = Some(block);
        let mut vars: Vec<u32> = self.drop_flags.keys().copied().collect();
        vars.sort_unstable();
        for var in vars {
            let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(var)? else {
                continue;
            };
            if !self.owns_heap(&ty) || !self.releasable(&ty) {
                continue;
            }
            let flag = self.drop_flags[&var];
            let cont = self.begin_flag_guard(ctx, flag);
            let slot = self.var_slots[var as usize];
            self.emit_release_storage(ctx, slot, &ty)?;
            self.end_flag_guard(ctx, cont);
        }
        let err_address = self.offset_address(ctx, outcome_ptr, outcome.err_offset);
        self.mem_copy(ctx, err_address, err_slot, 24, Reg(u32::MAX));
        let tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_ERR);
        let store = StoreOp::new(ctx, tag, outcome_ptr);
        self.append(ctx, store.get_operation(), None);
        let ret = ReturnOp::new(ctx, None);
        self.append(ctx, ret.get_operation(), None);
        self.current = saved;
        self.propagate_block = Some(block);
        Ok(block)
    }

    /// Materialize the register a `raise` names as an owned `MjError` in
    /// `storage`: a compile-time or borrowed message copies into a fresh
    /// allocation, an owned runtime string or String temporary transfers its
    /// allocation, a live nominal String copies its bytes (the VM clones the
    /// message and drops the String normally), and an error value moves.
    fn store_error_into(
        &mut self,
        ctx: &mut Context,
        storage: Value,
        src: Reg,
    ) -> Result<(), PlironError> {
        if let Some(descriptor) = self.str_runtime.get(&src.0).copied() {
            let data = if descriptor.owned && self.owned_temps.remove(&src.0).is_some() {
                descriptor.data
            } else {
                let data = self.emit_alloc(ctx, descriptor.len, 1, src);
                self.mem_copy_dynamic(ctx, data, descriptor.data, descriptor.len, src);
                data
            };
            self.store_string_fields(ctx, storage, data, descriptor.len, descriptor.len, src);
            return Ok(());
        }
        if let Some(bytes) = self.str_consts.get(&src.0).cloned() {
            let len = bytes.len() as u64;
            let len_value = self.uint_constant(ctx, len);
            let data = self.emit_alloc(ctx, len_value, 1, src);
            if len > 0 {
                let global = self.shared.intern_string(ctx, &bytes);
                let literal = self.global_address(ctx, &global, src);
                self.mem_copy(ctx, data, literal, len, src);
            }
            self.store_string_fields(ctx, storage, data, len_value, len_value, src);
            return Ok(());
        }
        match self.func.reg_types.get(&src.0) {
            Some(Ty::Struct(name, _)) if crate::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, src)?;
                if self.owned_temps.remove(&src.0).is_some() {
                    // The temporary transfers its whole allocation.
                    let (data, size) = self.string_parts(ctx, ptr, src);
                    let cap = self.string_cap(ctx, ptr, src);
                    self.store_string_fields(ctx, storage, data, size, cap, src);
                } else {
                    let (data, size) = self.string_parts(ctx, ptr, src);
                    let copy = self.emit_alloc(ctx, size, 1, src);
                    self.mem_copy_dynamic(ctx, copy, data, size, src);
                    self.store_string_fields(ctx, storage, copy, size, size, src);
                }
                Ok(())
            }
            Some(Ty::Error) => {
                let ptr = self.reg_ptr(ctx, src)?;
                self.mem_copy(ctx, storage, ptr, 24, src);
                self.owned_temps.remove(&src.0);
                Ok(())
            }
            // A nullary error struct (`raise StopIteration()`) carries no
            // runtime payload; its owned message is the VM's `Display` of the
            // value, `Name()`. Structs with fields keep rejecting: their
            // display embeds runtime field values.
            Some(ty @ Ty::Struct(name, _))
                if self
                    .layout
                    .layout_of(ty)
                    .is_ok_and(|layout| layout.size == 0) =>
            {
                let message = format!("{name}()").into_bytes();
                let len = message.len() as u64;
                let len_value = self.uint_constant(ctx, len);
                let data = self.emit_alloc(ctx, len_value, 1, src);
                let global = self.shared.intern_string(ctx, &message);
                let literal = self.global_address(ctx, &global, src);
                self.mem_copy(ctx, data, literal, len, src);
                self.store_string_fields(ctx, storage, data, len_value, len_value, src);
                Ok(())
            }
            _ => Err(self.unsupported_reg(format!("raised value in register %r{}", src.0), src)),
        }
    }

    /// `base + offset`, skipping the GEP for offset 0.
    fn offset_address(&mut self, ctx: &mut Context, base: Value, offset: u64) -> Value {
        if offset == 0 {
            base
        } else {
            self.gep_byte_unspanned(ctx, base, offset)
        }
    }

    /// Emit the i32 tag constant of a tagged outcome.
    fn tag_constant(&mut self, ctx: &mut Context, tag: u32) -> Value {
        let i32_int = IntegerType::get(ctx, 32, Signedness::Signless);
        let attr = IntegerAttr::new(i32_int, APInt::from_u64(u64::from(tag), bw(32)));
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    fn lower_error_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
    ) -> Result<(), PlironError> {
        if args.len() != 1 || !kwargs.is_empty() {
            return Err(self.unsupported_reg("Error construction contract".into(), dest));
        }
        let (data, len) = self.string_bytes(ctx, args[0], dest)?;
        self.str_runtime.insert(
            dest.0,
            RuntimeStr {
                data,
                len,
                owned: false,
            },
        );
        Ok(())
    }

    /// `Raise`: materialize the raised value as an owned error in the staging
    /// slot and jump to the innermost raise-edge target (a `try` landing
    /// block once regions lower; the raising function's propagate block
    /// otherwise). Lowering continues into a fresh unreachable block for the
    /// dead remainder of the MIR block.
    fn lower_raise(&mut self, ctx: &mut Context, src: Reg) -> Result<(), PlironError> {
        let err_slot = self.ensure_err_slot(ctx);
        self.store_error_into(ctx, err_slot, src)?;
        // The VM's lifecycle log records only `Value::Error` raises; a raised
        // error struct (`raise StopIteration()`) stays silent there.
        let struct_raise = matches!(self.func.reg_types.get(&src.0), Some(Ty::Struct(name, _))
            if !crate::symbol::is_stdlib_string_struct(name));
        if self.trace_lifecycle && !struct_raise {
            self.emit_trace_err_slot(ctx, crate::native::rt_abi::TRACE_RAISE);
        }
        // A raise inside a finalbody overrides the pending outcome; the VM
        // runs a pending return's roots before propagating.
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let target = self.raise_edge_target(ctx)?;
        let jump = BrOp::new(ctx, target, vec![]);
        self.append(ctx, jump.get_operation(), Some(src));
        let region = self.region.expect("lowering is inside a function region");
        let dead = BasicBlock::new(ctx, None, vec![]);
        dead.insert_at_back(region, ctx);
        self.current = Some(dead);
        Ok(())
    }

    /// A structurally lowered `try`: flatten the region mini-CFGs into this
    /// function's flat block list with explicit edges. The body lowers with a
    /// fresh landing block as its raise-edge target; every body exit (normal
    /// completion, raise, return, escape) runs the flag-guarded `cleanup`
    /// drops exactly like the VM's `exec_try`; the handler binds the staged
    /// error; `orelse` runs only on normal completion.
    fn lower_try(
        &mut self,
        ctx: &mut Context,
        body: &[MirBlock],
        handler: Option<&(Option<u32>, Vec<MirBlock>)>,
        orelse: Option<&[MirBlock]>,
        finalbody: Option<&[MirBlock]>,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let after = BasicBlock::new(ctx, None, vec![]);
        after.insert_at_back(region, ctx);
        let landing = BasicBlock::new(ctx, None, vec![]);
        landing.insert_at_back(region, ctx);
        let normal_exit = BasicBlock::new(ctx, None, vec![]);
        normal_exit.insert_at_back(region, ctx);

        // A `finally`-bearing try gets its pending-outcome machinery up
        // front: kind/error staging, the forwarding entry every pending edge
        // jumps to, the post-finalbody dispatch, a normal-entry (kind 0)
        // block, and the error-entry (kind 1) block raises route through.
        let finally_idx = match finalbody {
            Some(_) => {
                let entry = BasicBlock::new(ctx, None, vec![]);
                entry.insert_at_back(region, ctx);
                let dispatch = BasicBlock::new(ctx, None, vec![]);
                dispatch.insert_at_back(region, ctx);
                let error_entry = BasicBlock::new(ctx, None, vec![]);
                error_entry.insert_at_back(region, ctx);
                let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
                let kind_slot = self.entry_typed_alloca(ctx, i32_handle);
                let pending_err = self.entry_alloca(ctx, 24, 8);
                self.finally_states.push(FinallyState {
                    entry,
                    dispatch,
                    error_entry,
                    kind_slot,
                    pending_err,
                    codes: Vec::new(),
                    error_possible: false,
                    after,
                });
                Some(self.finally_states.len() - 1)
            }
            None => None,
        };

        // Body: raises inside it land on this try's landing block.
        self.try_frames.push(TryFrame {
            landing,
            cleanup: cleanup.to_vec(),
            finally: finally_idx,
            pends_error: handler.is_none() && finally_idx.is_some(),
        });
        let body_entry = self.lower_region(ctx, body, normal_exit)?;
        self.try_frames.pop();
        // Enter the try from the enclosing block.
        let enter = BrOp::new(ctx, body_entry, vec![]);
        self.append(ctx, enter.get_operation(), None);

        // Handler and `else` regions lower under a pseudo-frame when a
        // `finally` exists: their raises stage the pending error and their
        // returns/escapes still pend on this finalbody, but the body-edge
        // cleanup does not run twice.
        let pseudo_frame = finally_idx.map(|idx| TryFrame {
            landing: self.finally_states[idx].error_entry,
            cleanup: Vec::new(),
            finally: Some(idx),
            pends_error: true,
        });
        // Where a completed handler/orelse continues: into the finalbody
        // with a normal pending outcome, or straight to the continuation.
        let normal_entry = match finally_idx {
            Some(idx) => {
                let block = BasicBlock::new(ctx, None, vec![]);
                block.insert_at_back(region, ctx);
                let saved = self.current;
                self.current = Some(block);
                let tag = self.tag_constant(ctx, 0);
                let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
                self.append(ctx, store.get_operation(), None);
                let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
                block
            }
            None => after,
        };

        // Raise edge: cleanup drops, then the handler (binding the staged
        // error) or propagation. The handler region lowers before `orelse` —
        // position-space region ids must follow the body → handler → orelse
        // → finalbody order `record_last_uses` used.
        self.current = Some(landing);
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        match handler {
            Some((error_var, handler_blocks)) => {
                if self.trace_lifecycle {
                    self.emit_trace_err_slot(ctx, crate::native::rt_abi::TRACE_CATCH);
                }
                let err_slot = self.ensure_err_slot(ctx);
                match error_var {
                    Some(var) => {
                        // A still-initialized previous binding (a loop
                        // rebinding the same handler var) frees first — the
                        // VM abandons the overwritten value to its arena.
                        if let Some(&flag) = self.drop_flags.get(var) {
                            let cont = self.begin_flag_guard(ctx, flag);
                            let slot = self.var_slots[*var as usize];
                            self.emit_release_storage(ctx, slot, &Ty::Error)?;
                            self.end_flag_guard(ctx, cont);
                        }
                        // The staged error moves into the bound slot; its
                        // ordinary drop frees the message.
                        let slot = self.var_slots[*var as usize];
                        self.mem_copy(ctx, slot, err_slot, 24, Reg(u32::MAX));
                        self.set_drop_flag(ctx, *var, true);
                    }
                    None => {
                        // No binder: the caught error is dropped on entry.
                        self.emit_release_storage(ctx, err_slot, &Ty::Error)?;
                    }
                }
                if let Some(frame) = pseudo_frame.as_ref() {
                    self.try_frames.push(TryFrame {
                        landing: frame.landing,
                        cleanup: Vec::new(),
                        finally: frame.finally,
                        pends_error: true,
                    });
                }
                let handler_entry = self.lower_region(ctx, handler_blocks, normal_entry)?;
                if pseudo_frame.is_some() {
                    self.try_frames.pop();
                }
                let jump = BrOp::new(ctx, handler_entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
            }
            None => match finally_idx {
                // No handler: the raise pends on the finalbody, or
                // propagates to the enclosing observer.
                Some(idx) => {
                    let jump = BrOp::new(ctx, self.finally_states[idx].error_entry, vec![]);
                    self.append(ctx, jump.get_operation(), None);
                }
                None => {
                    let target = self.raise_edge_target(ctx)?;
                    let jump = BrOp::new(ctx, target, vec![]);
                    self.append(ctx, jump.get_operation(), None);
                }
            },
        }

        // Normal completion: cleanup drops, then `orelse` (only here), then
        // the finalbody or the continuation.
        self.current = Some(normal_exit);
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        let normal_target = match orelse {
            Some(orelse_blocks) => {
                if let Some(frame) = pseudo_frame.as_ref() {
                    self.try_frames.push(TryFrame {
                        landing: frame.landing,
                        cleanup: Vec::new(),
                        finally: frame.finally,
                        pends_error: true,
                    });
                }
                let entry = self.lower_region(ctx, orelse_blocks, normal_entry)?;
                if pseudo_frame.is_some() {
                    self.try_frames.pop();
                }
                entry
            }
            None => normal_entry,
        };
        let jump = BrOp::new(ctx, normal_target, vec![]);
        self.append(ctx, jump.get_operation(), None);

        // The finalbody itself, then the pending-outcome dispatch.
        if let (Some(final_blocks), Some(idx)) = (finalbody, finally_idx) {
            // Error entry: stage the raise as this try's pending outcome.
            let saved = self.current;
            self.current = Some(self.finally_states[idx].error_entry);
            let err_slot = self.ensure_err_slot(ctx);
            let pending_err = self.finally_states[idx].pending_err;
            self.mem_copy(ctx, pending_err, err_slot, 24, Reg(u32::MAX));
            let tag = self.tag_constant(ctx, 1);
            let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
            self.append(ctx, store.get_operation(), None);
            let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
            self.append(ctx, jump.get_operation(), None);
            self.current = saved;

            // The finalbody lowers once, outside this try's raise protection
            // (its own raise/return/escape overrides the pending outcome and
            // resolves it on the way out).
            self.finally_overrides.push(idx);
            let fin_entry = self.lower_region(ctx, final_blocks, self.finally_states[idx].dispatch);
            self.finally_overrides.pop();
            let fin_entry = fin_entry?;
            let saved = self.current;
            self.current = Some(self.finally_states[idx].entry);
            let jump = BrOp::new(ctx, fin_entry, vec![]);
            self.append(ctx, jump.get_operation(), None);

            // Dispatch: forward the pending outcome now that the finalbody
            // completed normally.
            self.current = Some(self.finally_states[idx].dispatch);
            self.emit_finally_dispatch(ctx, idx)?;
            self.current = saved;
        }

        self.current = Some(after);
        Ok(())
    }

    /// The post-finalbody dispatch of `finally_states[idx]`: switch on the
    /// pending kind — normal continues to the try's continuation, a pending
    /// error re-raises toward the enclosing observer, and a pending exit
    /// site crosses outward (running enclosing cleanups, pending on the next
    /// finalbody, or reaching its terminal).
    fn emit_finally_dispatch(&mut self, ctx: &mut Context, idx: usize) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let kind_slot = self.finally_states[idx].kind_slot;
        let after = self.finally_states[idx].after;
        let pending_err = self.finally_states[idx].pending_err;
        let codes: Vec<u32> = self.finally_states[idx].codes.clone();
        let error_possible = self.finally_states[idx].error_possible;
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let kind = LoadOp::new(ctx, kind_slot, i32_handle);
        self.append(ctx, kind.get_operation(), None);
        let kind = kind.get_result(ctx);

        // Pending error case (kind 1): restage and re-raise outward.
        let mut next = self.current.expect("dispatch emission is inside a block");
        if error_possible {
            let error_case = BasicBlock::new(ctx, None, vec![]);
            error_case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(error_case);
                let err_slot = self.ensure_err_slot(ctx);
                self.mem_copy(ctx, err_slot, pending_err, 24, Reg(u32::MAX));
                let target = self.raise_edge_target(ctx)?;
                let jump = BrOp::new(ctx, target, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let one = self.tag_constant(ctx, 1);
            let is_error = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, one);
            self.append(ctx, is_error.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(
                ctx,
                is_error.get_result(ctx),
                error_case,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        // One case per pending exit-site code.
        for code in codes {
            self.current = Some(next);
            let case = BasicBlock::new(ctx, None, vec![]);
            case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(case);
                self.emit_exit_crossing(ctx, code)?;
                self.current = saved;
            }
            let expected = self.tag_constant(ctx, code);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, expected);
            self.append(ctx, matches.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(ctx, matches.get_result(ctx), case, vec![], rest, vec![]);
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        // Everything else is the normal completion.
        self.current = Some(next);
        let jump = BrOp::new(ctx, after, vec![]);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// Route exit-site `code` outward from the current frame context: run
    /// each enclosing frame's cleanup drops inner to outer; the first
    /// `finally`-bearing frame records the code as pending and enters its
    /// finalbody; with none left, the site's terminal runs.
    fn emit_exit_crossing(&mut self, ctx: &mut Context, code: u32) -> Result<(), PlironError> {
        let frames: Vec<(Vec<u32>, Option<usize>)> = self
            .try_frames
            .iter()
            .rev()
            .map(|frame| (frame.cleanup.clone(), frame.finally))
            .collect();
        for (cleanup, finally) in frames {
            for var in cleanup {
                self.lower_drop_var(ctx, var)?;
            }
            if let Some(idx) = finally {
                if !self.finally_states[idx].codes.contains(&code) {
                    self.finally_states[idx].codes.push(code);
                }
                let tag = self.tag_constant(ctx, code);
                let store = StoreOp::new(ctx, tag, self.finally_states[idx].kind_slot);
                self.append(ctx, store.get_operation(), None);
                let jump = BrOp::new(ctx, self.finally_states[idx].entry, vec![]);
                self.append(ctx, jump.get_operation(), None);
                return Ok(());
            }
        }
        let terminal = self.site_terminal(ctx, code)?;
        let jump = BrOp::new(ctx, terminal, vec![]);
        self.append(ctx, jump.get_operation(), None);
        Ok(())
    }

    /// The terminal block of exit site `code - 2`: a return runs the site's
    /// carried cleanup roots, resolves any pending outcomes the site
    /// overrode, and returns the staged value; an escape jumps to its
    /// function-level target.
    fn site_terminal(
        &mut self,
        ctx: &mut Context,
        code: u32,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        let site = (code - 2) as usize;
        if let Some(block) = self.exit_sites[site].terminal {
            return Ok(block);
        }
        let region = self.region.expect("lowering is inside a function");
        let block = BasicBlock::new(ctx, None, vec![]);
        block.insert_at_back(region, ctx);
        self.exit_sites[site].terminal = Some(block);
        let saved = self.current;
        self.current = Some(block);
        match &self.exit_sites[site].action {
            ExitAction::Return { cleanup } => {
                let cleanup = cleanup.clone();
                let overrides = self.exit_sites[site].overrides.clone();
                for var in cleanup {
                    self.lower_drop_var(ctx, var)?;
                }
                // The VM merges an overridden return's cleanup roots after
                // the overriding return's own (distinct-union; flags make
                // re-listed roots no-ops), innermost override first.
                for idx in overrides.into_iter().rev() {
                    self.emit_pending_resolution(ctx, idx)?;
                }
                self.emit_staged_return(ctx)?;
            }
            ExitAction::Escape { target } => {
                let target = *target;
                let Some(&target_block) = self.function_blocks.get(target) else {
                    return Err(
                        self.unsupported(format!("escape to missing block bb{target}"), None)
                    );
                };
                let jump = BrOp::new(ctx, target_block, vec![]);
                self.append(ctx, jump.get_operation(), None);
            }
        }
        self.current = saved;
        Ok(block)
    }

    /// Resolve the pending outcome of `finally_states[idx]` after an
    /// override: a pending return's carried roots still leave scope, a
    /// pending error's message frees (no user destructor — the VM's
    /// discarded error is arena-reclaimed), a pending normal is nothing.
    fn emit_pending_resolution(
        &mut self,
        ctx: &mut Context,
        idx: usize,
    ) -> Result<(), PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let kind_slot = self.finally_states[idx].kind_slot;
        let pending_err = self.finally_states[idx].pending_err;
        let codes: Vec<u32> = self.finally_states[idx].codes.clone();
        let error_possible = self.finally_states[idx].error_possible;
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let kind = LoadOp::new(ctx, kind_slot, i32_handle);
        self.append(ctx, kind.get_operation(), None);
        let kind = kind.get_result(ctx);
        let join = BasicBlock::new(ctx, None, vec![]);
        join.insert_at_back(region, ctx);

        // kind 1: free the discarded pending error's message.
        let mut next = self.current.expect("resolution emission is inside a block");
        if error_possible {
            let error_case = BasicBlock::new(ctx, None, vec![]);
            error_case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(error_case);
                self.emit_release_storage(ctx, pending_err, &Ty::Error)?;
                let jump = BrOp::new(ctx, join, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let one = self.tag_constant(ctx, 1);
            let is_error = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, one);
            self.append(ctx, is_error.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(
                ctx,
                is_error.get_result(ctx),
                error_case,
                vec![],
                rest,
                vec![],
            );
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }

        for code in codes {
            // Only pending returns carry roots to resolve; a pending escape
            // resolved its overrides at its own site.
            let site = (code - 2) as usize;
            let ExitAction::Return { cleanup } = &self.exit_sites[site].action else {
                continue;
            };
            let cleanup = cleanup.clone();
            let inner_overrides = self.exit_sites[site].overrides.clone();
            self.current = Some(next);
            let case = BasicBlock::new(ctx, None, vec![]);
            case.insert_at_back(region, ctx);
            {
                let saved = self.current;
                self.current = Some(case);
                for var in cleanup {
                    self.lower_drop_var(ctx, var)?;
                }
                for inner in inner_overrides.into_iter().rev() {
                    self.emit_pending_resolution(ctx, inner)?;
                }
                let jump = BrOp::new(ctx, join, vec![]);
                self.append(ctx, jump.get_operation(), None);
                self.current = saved;
            }
            let expected = self.tag_constant(ctx, code);
            let matches = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, kind, expected);
            self.append(ctx, matches.get_operation(), None);
            let rest = BasicBlock::new(ctx, None, vec![]);
            rest.insert_at_back(region, ctx);
            let branch = CondBrOp::new(ctx, matches.get_result(ctx), case, vec![], rest, vec![]);
            self.append(ctx, branch.get_operation(), None);
            next = rest;
        }
        self.current = Some(next);
        let jump = BrOp::new(ctx, join, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join);
        Ok(())
    }

    /// Stage a return's value at its site, before any finalbody runs: the ok
    /// payload of a raising function writes into the outcome, an aggregate
    /// writes through the sret pointer, a scalar parks in the per-function
    /// staging slot.
    fn stage_return_value(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
    ) -> Result<(), PlironError> {
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            let outcome_ptr = self
                .outcome_ptr
                .expect("raising functions receive an outcome pointer");
            match (&outcome.ok, value) {
                (LowerTy::ZeroSized, _) | (_, None) => {}
                (LowerTy::Scalar(expected), Some(reg)) => {
                    let staged = self.reg_value(ctx, reg, *expected)?;
                    let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                    let store = StoreOp::new(ctx, staged, address);
                    self.append(ctx, store.get_operation(), Some(reg));
                }
                (LowerTy::Aggregate { layout, .. }, Some(reg)) => {
                    let size = layout.size;
                    let ptr = self.reg_ptr(ctx, reg)?;
                    let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                    self.mem_copy(ctx, address, ptr, size, reg);
                    self.owned_temps.remove(&reg.0);
                }
            }
            return Ok(());
        }
        let ret_lower = self.return_value_lower()?;
        match (ret_lower, value) {
            (Some(LowerTy::Aggregate { layout, .. }), Some(reg)) => {
                let sret = self
                    .sret_ptr
                    .expect("aggregate-returning functions receive an sret pointer");
                let ptr = self.reg_ptr(ctx, reg)?;
                self.mem_copy(ctx, sret, ptr, layout.size, reg);
                self.owned_temps.remove(&reg.0);
            }
            (Some(LowerTy::Scalar(expected)), Some(reg)) => {
                let staged = self.reg_value(ctx, reg, expected)?;
                let slot = match self.pending_ret {
                    Some(slot) => slot,
                    None => {
                        let handle = expected.handle(ctx);
                        let slot = self.entry_typed_alloca(ctx, handle);
                        self.pending_ret = Some(slot);
                        slot
                    }
                };
                let store = StoreOp::new(ctx, staged, slot);
                self.append(ctx, store.get_operation(), Some(reg));
            }
            _ => {}
        }
        Ok(())
    }

    /// The function-exit half of a staged return: read the staged value per
    /// the return ABI and return.
    fn emit_staged_return(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        self.emit_frame_exit_error_releases(ctx)?;
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            let outcome_ptr = self
                .outcome_ptr
                .expect("raising functions receive an outcome pointer");
            let _ = outcome;
            let tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_OK);
            let store = StoreOp::new(ctx, tag, outcome_ptr);
            self.append(ctx, store.get_operation(), None);
            let ret = ReturnOp::new(ctx, None);
            self.append(ctx, ret.get_operation(), None);
            return Ok(());
        }
        let ret_lower = self.return_value_lower()?;
        let value = match ret_lower {
            Some(LowerTy::Scalar(scalar)) => {
                let slot = self
                    .pending_ret
                    .expect("scalar returns crossing a finalbody stage their value");
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, slot, handle);
                self.append(ctx, load.get_operation(), None);
                Some(load.get_result(ctx))
            }
            _ => None,
        };
        let ret = ReturnOp::new(ctx, value);
        self.append(ctx, ret.get_operation(), None);
        Ok(())
    }

    /// Lower one `try` sub-region mini-CFG into fresh pliron blocks (the
    /// region's local block ids swap in as `self.blocks`; its position-space
    /// ids continue `next_region_block` exactly as `record_last_uses`
    /// assigned them). `FallOff` jumps to `falloff`. Returns the region's
    /// entry block; `self.current` is restored.
    fn lower_region(
        &mut self,
        ctx: &mut Context,
        blocks: &[MirBlock],
        falloff: Ptr<BasicBlock>,
    ) -> Result<Ptr<BasicBlock>, PlironError> {
        let region = self.region.expect("lowering is inside a function");
        let ids_start = self.next_region_block;
        self.next_region_block += blocks.len();
        let mut locals = Vec::with_capacity(blocks.len());
        for _ in blocks {
            let block = BasicBlock::new(ctx, None, vec![]);
            block.insert_at_back(region, ctx);
            locals.push(block);
        }
        let entry = locals[0];
        let saved_blocks = std::mem::replace(&mut self.blocks, locals);
        let saved_falloff = self.falloff_target.replace(falloff);
        let saved_current = self.current;
        let saved_position = self.position;
        let mut result = Ok(());
        'blocks: for (i, block) in blocks.iter().enumerate() {
            self.current = Some(self.blocks[i]);
            for (index, instr) in block.instrs.iter().enumerate() {
                self.position = (ids_start + i, index);
                if let Err(error) = self
                    .lower_instr(ctx, instr)
                    .and_then(|()| self.flush_owned_temps(ctx))
                {
                    result = Err(error);
                    break 'blocks;
                }
            }
            self.position = (ids_start + i, usize::MAX);
            if let Err(error) = self.lower_term(ctx, &block.term) {
                result = Err(error);
                break 'blocks;
            }
        }
        self.blocks = saved_blocks;
        self.falloff_target = saved_falloff;
        self.current = saved_current;
        self.position = saved_position;
        result.map(|()| entry)
    }

    /// Drops crossing a return or escape edge: each enclosing `try`'s
    /// cleanup list (inner to outer — the VM runs `Try.cleanup` whenever a
    /// body region is left), then the edge's own carried cleanup roots. All
    /// drops are flag-guarded, so listings that already died are no-ops.
    fn emit_scope_exit_cleanups(
        &mut self,
        ctx: &mut Context,
        edge_cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let frames: Vec<Vec<u32>> = self
            .try_frames
            .iter()
            .rev()
            .map(|frame| frame.cleanup.clone())
            .collect();
        for cleanup in frames {
            for var in cleanup {
                self.lower_drop_var(ctx, var)?;
            }
        }
        for &var in edge_cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        Ok(())
    }

    /// The `(data, len)` byte pair of a string-carrying register: a
    /// compile-time literal, a runtime string (an `Error` message included),
    /// or a nominal String value.
    fn string_bytes(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        if let Some(bytes) = self.str_consts.get(&reg.0).cloned() {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&reg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&reg.0) {
            Some(Ty::Struct(name, _)) if crate::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            // An error value displays as its bare message (the VM's
            // `format_value` over `Value::Error`).
            Some(Ty::Error) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            _ => Err(self.unsupported_reg(format!("string value in register %r{}", reg.0), dest)),
        }
    }

    /// The compiled `__init__` a constructor call executes: the exact name,
    /// else the unique overload taking `argc + 1` parameters (counting
    /// `out self`) — the VM's `overload_name` policy over the compiled set.
    fn constructor_init(&self, name: &str, argc: usize) -> Option<String> {
        let init = format!("{name}.__init__");
        if self.signatures.contains_key(&init) {
            return Some(init);
        }
        let mut matches = self.signatures.iter().filter(|(fname, signature)| {
            crate::symbol::is_overload_of(fname, &init) && signature.params.len() == argc + 1
        });
        let first = matches.next()?.0.clone();
        matches.next().is_none().then_some(first)
    }

    /// `DropVar` — the VM's `drop_value`: nothing for scalars; for aggregates
    /// run the compiled `__deinit__` when defined, then destroy fields in
    /// reverse declaration order. Combinations whose residual state is only
    /// dynamically knowable (a destructor plus independently droppable
    /// fields, or a partially-moved variable) reject instead of guessing.
    fn lower_drop_var(&mut self, ctx: &mut Context, var: u32) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                if !self.needs_drop(&ty) {
                    return Ok(());
                }
                if self.partially_moved.contains(&var) {
                    return Err(self.unsupported(
                        format!(
                            "drop of partially-moved variable `{}`",
                            self.func
                                .var_names
                                .get(var as usize)
                                .map(String::as_str)
                                .unwrap_or("?")
                        ),
                        None,
                    ));
                }
                let ptr = self.var_slots[var as usize];
                let flag = self.drop_flags.get(&var).copied();
                match flag {
                    Some(flag) => {
                        let cont = self.begin_flag_guard(ctx, flag);
                        self.emit_drop_value(ctx, ptr, &ty, false)?;
                        self.set_drop_flag(ctx, var, false);
                        self.end_flag_guard(ctx, cont);
                        Ok(())
                    }
                    None => self.emit_drop_value(ctx, ptr, &ty, false),
                }
            }
        }
    }

    /// `ConsumeVar`: skip the whole-value destructor but destroy residual
    /// fields — a no-op unless fields carry their own destructor work. The
    /// consumed slot is empty afterwards either way.
    fn lower_consume_var(&mut self, ctx: &mut Context, var: u32) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                if self.fields_need_drop(&ty) {
                    return Err(
                        self.unsupported("variable consumption with droppable fields".into(), None)
                    );
                }
                if self.trace_lifecycle
                    && let Ty::Struct(name, _) = ty.as_ref()
                {
                    let name = name.clone();
                    self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_CONSUME, &name);
                }
                self.set_drop_flag(ctx, var, false);
                Ok(())
            }
        }
    }

    /// Branch on `flag` into a fresh guarded-work block, returning the
    /// continuation block. The caller emits the guarded work into the current
    /// block, then closes with [`Self::end_flag_guard`].
    fn begin_flag_guard(&mut self, ctx: &mut Context, flag: Value) -> Ptr<BasicBlock> {
        let i1: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let load = LoadOp::new(ctx, flag, i1);
        self.append(ctx, load.get_operation(), None);
        let region = self.region.expect("lowering is inside a function");
        let work = BasicBlock::new(ctx, None, vec![]);
        work.insert_at_back(region, ctx);
        let cont = BasicBlock::new(ctx, None, vec![]);
        cont.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, load.get_result(ctx), work, vec![], cont, vec![]);
        self.append(ctx, branch.get_operation(), None);
        self.current = Some(work);
        cont
    }

    /// Close a [`Self::begin_flag_guard`] region: jump to and continue in the
    /// continuation block.
    fn end_flag_guard(&mut self, ctx: &mut Context, cont: Ptr<BasicBlock>) {
        let jump = BrOp::new(ctx, cont, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(cont);
    }

    /// Store `value` into `var`'s initialization flag; a no-op for variables
    /// without one (nothing droppable to guard).
    fn set_drop_flag(&mut self, ctx: &mut Context, var: u32, value: bool) {
        let Some(&flag) = self.drop_flags.get(&var) else {
            return;
        };
        let constant = self.bool_constant(ctx, value);
        let store = StoreOp::new(ctx, constant, flag);
        self.append(ctx, store.get_operation(), None);
    }

    /// Emit the VM's `drop_value` over storage: the struct's compiled
    /// `__deinit__` (its own body consumes the receiver's residual fields),
    /// else recurse into fields in reverse declaration order.
    fn emit_drop_value(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        skip_whole_deinit: bool,
    ) -> Result<(), PlironError> {
        match ty {
            // The built-in error has no user destructor; dropping it frees
            // the message buffer invisibly (the VM's error drop is a no-op —
            // its message is arena-owned).
            Ty::Error => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let data = LoadOp::new(ctx, ptr, handle);
                self.append(ctx, data.get_operation(), None);
                self.emit_free(ctx, data.get_result(ctx));
                Ok(())
            }
            Ty::Struct(name, _) => {
                let deinit = format!("{name}.__deinit__");
                if !skip_whole_deinit && self.declarations.contains_key(&deinit) {
                    if self.fields_need_drop(ty) {
                        // The VM destroys only the residual fields the
                        // destructor body left initialized — dynamic state
                        // this backend does not track.
                        return Err(self.unsupported(
                            format!("destructor of `{name}` with droppable fields"),
                            None,
                        ));
                    }
                    let Some(signature) = self.signatures.get(&deinit) else {
                        return Err(
                            self.unsupported(format!("drop via uncompiled `{deinit}`"), None)
                        );
                    };
                    if signature.outcome.is_some() {
                        return Err(
                            self.unsupported(format!("raising destructor `{deinit}`"), None)
                        );
                    }
                    if self.trace_lifecycle {
                        let name = name.clone();
                        self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_DROP, &name);
                    }
                    let callee: Identifier = signature
                        .mangled
                        .as_str()
                        .try_into()
                        .expect("mangled names are identifier-safe");
                    let func_ty = signature.func_ty;
                    let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, vec![ptr]);
                    self.append(ctx, call.get_operation(), None);
                    return Ok(());
                }
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let fields = decl.fields.clone();
                let field_tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
                let composed = self
                    .layout
                    .struct_layout(&field_tys)
                    .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
                for (position, (_, field_ty)) in fields.iter().enumerate().rev() {
                    if !self.needs_drop(field_ty) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, field_ty, false)?;
                }
                Ok(())
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self
                    .layout
                    .struct_layout(&elements)
                    .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
                for (position, element) in elements.iter().enumerate().rev() {
                    if !self.needs_drop(element) {
                        continue;
                    }
                    let offset = composed.offsets[position];
                    let address = if offset == 0 {
                        ptr
                    } else {
                        self.gep_byte_unspanned(ctx, ptr, offset)
                    };
                    self.emit_drop_value(ctx, address, element, false)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Whether dropping a value of `ty` performs any work: a struct with a
    /// `__deinit__`, any transitive field/element that does, or the built-in
    /// error (its message buffer frees on drop).
    fn needs_drop(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Error)
            || self.has_lifecycle_method(ty, "__deinit__")
            || self.fields_need_drop(ty)
    }

    /// Whether any transitive field/element of `ty` performs drop work
    /// (excluding `ty`'s own whole-value destructor).
    fn fields_need_drop(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(name, _) => self
                .struct_decls
                .get(name.as_str())
                .is_some_and(|decl| decl.fields.iter().any(|(_, field)| self.needs_drop(field))),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                elements.iter().any(|element| self.needs_drop(element))
            }
            _ => false,
        }
    }

    /// Whether `ty` is a struct whose program declares `method` for it.
    fn has_lifecycle_method(&self, ty: &Ty, method: &str) -> bool {
        matches!(ty, Ty::Struct(name, _) if self
            .declarations
            .contains_key(&format!("{name}.{method}")))
    }

    /// Whether `ty` or any transitive field declares `method`.
    fn has_nested_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        if self.has_lifecycle_method(ty, method) {
            return true;
        }
        match ty {
            Ty::Struct(name, _) => self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                decl.fields
                    .iter()
                    .any(|(_, field)| self.has_nested_lifecycle(field, method))
            }),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.has_nested_lifecycle(element, method)),
            _ => false,
        }
    }

    /// The bound operand value of one argument at its expected lowered type.
    /// A consuming (`owned`) parameter takes ownership — an owned temporary
    /// passed there transfers to the callee, which destroys it.
    fn arg_value(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        expected: &LowerTy,
        owned: bool,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        if owned {
            self.owned_temps.remove(&reg.0);
        }
        match expected {
            LowerTy::Scalar(scalar) => self.reg_value(ctx, reg, *scalar),
            LowerTy::Aggregate { .. } => self.reg_ptr(ctx, reg),
            LowerTy::ZeroSized => Err(self.unsupported_reg("zero-sized argument".into(), dest)),
        }
    }

    /// Emit the call to compiled `name` with fully bound operands, prepending
    /// fresh sret storage for an aggregate return and defining or erasing
    /// `dest` by the callee's result kind.
    fn emit_bound_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        let signature = &self.signatures[name];
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let (func_ty, returns_value, sret) =
            (signature.func_ty, signature.returns_value, signature.sret);
        if let Some(outcome) = signature.outcome.clone() {
            return self.emit_raising_call(ctx, dest, callee, func_ty, outcome, operands);
        }
        if let Some(layout) = sret {
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            operands.insert(0, storage);
            let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, operands);
            self.append(ctx, call.get_operation(), Some(dest));
            self.reg_values.insert(dest.0, storage);
            // The callee's return transferred ownership here; a discarded or
            // borrowed-only aggregate result is an owned temporary.
            if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
                && self.owns_heap(&ty)
                && self.releasable(&ty)
            {
                self.mark_owned_temp(dest, ty)?;
            }
            Ok(())
        } else {
            let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, operands);
            if returns_value {
                self.define(ctx, dest, call.get_operation(), call.get_result(ctx))
            } else {
                self.append(ctx, call.get_operation(), Some(dest));
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Call a raising function through its prepended outcome out-pointer and
    /// branch on the tag: the error edge stages the callee's error and jumps
    /// to the innermost raise-edge target; lowering continues in the ok
    /// block with the payload bound (so post-call effects like receiver
    /// write-back run only on success, matching the VM).
    fn emit_raising_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callee: Identifier,
        func_ty: TypedHandle<FuncType>,
        outcome: OutcomeAbi,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        operands.insert(0, storage);
        let call = CallOp::new(ctx, CallOpCallable::Direct(callee), func_ty, operands);
        self.append(ctx, call.get_operation(), Some(dest));
        let i32_handle: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
        let tag = LoadOp::new(ctx, storage, i32_handle);
        self.append(ctx, tag.get_operation(), Some(dest));
        let err_tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_ERR);
        let is_err = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, tag.get_result(ctx), err_tag);
        self.append(ctx, is_err.get_operation(), Some(dest));
        let region = self.region.expect("lowering is inside a function");
        let err_block = BasicBlock::new(ctx, None, vec![]);
        err_block.insert_at_back(region, ctx);
        let ok_block = BasicBlock::new(ctx, None, vec![]);
        ok_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_err.get_result(ctx),
            err_block,
            vec![],
            ok_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        self.current = Some(err_block);
        let err_slot = self.ensure_err_slot(ctx);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.mem_copy(ctx, err_slot, err_address, 24, dest);
        // A propagating call inside a finalbody overrides the pending
        // outcome, like a raise.
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let target = self.raise_edge_target(ctx)?;
        let jump = BrOp::new(ctx, target, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(ok_block);
        match outcome.ok {
            LowerTy::Scalar(scalar) => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { ty, .. } => {
                let address = self.offset_address(ctx, storage, outcome.ok_offset);
                self.reg_values.insert(dest.0, address);
                // The ok payload transferred ownership here, like an sret
                // result.
                if self.owns_heap(&ty) && self.releasable(&ty) {
                    self.mark_owned_temp(dest, *ty)?;
                }
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Load the value at `address` with checked type `ty` into `dest`:
    /// scalars load directly; aggregates copy out into fresh storage — the
    /// VM's clone-on-read place semantics.
    fn load_from(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        dest: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(dest))? {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, address, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { layout, .. } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                self.mem_copy(ctx, storage, address, layout.size, dest);
                self.reg_values.insert(dest.0, storage);
                Ok(())
            }
            LowerTy::ZeroSized => {
                self.erased.insert(dest.0);
                Ok(())
            }
        }
    }

    /// Store register `src` (checked type `ty`) to `address`.
    fn store_to(
        &mut self,
        ctx: &mut Context,
        address: Value,
        ty: &Ty,
        src: Reg,
    ) -> Result<(), PlironError> {
        match lower_ty(self.name, ty, &self.layout, self.reg_span(src))? {
            LowerTy::Scalar(scalar) => {
                // A pending literal entering literal-typed storage converts
                // exactly (reject-never-wrap) rather than at the slot kind.
                let value = if matches!(ty, Ty::IntLiteral | Ty::FloatLiteral)
                    && let Some(literal) = self.pending_literals.get(&src.0).cloned()
                {
                    let constant = self.exact_literal_storage(ctx, &literal, ty, src)?;
                    self.reg_values.insert(src.0, constant);
                    constant
                } else {
                    self.reg_value(ctx, src, scalar)?
                };
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(src));
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let ptr = self.reg_ptr(ctx, src)?;
                self.mem_copy(ctx, address, ptr, layout.size, src);
                // The designated storage owns the value now.
                self.owned_temps.remove(&src.0);
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    /// The storage pointer of an aggregate-valued register. A compile-time
    /// StringLiteral consumed as storage materializes on first use as a
    /// borrowed `MjStrDesc` over its interned constant bytes.
    fn reg_ptr(&mut self, ctx: &mut Context, reg: Reg) -> Result<Value, PlironError> {
        if let Some(value) = self.reg_values.get(&reg.0) {
            return Ok(*value);
        }
        if let Some(bytes) = self.str_consts.get(&reg.0).cloned() {
            let storage = self.entry_alloca(ctx, 16, 8);
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, reg);
            let store_data = StoreOp::new(ctx, data, storage);
            self.append(ctx, store_data.get_operation(), Some(reg));
            let len_address = self.gep_byte(ctx, storage, 8, reg);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            let store_len = StoreOp::new(ctx, len, len_address);
            self.append(ctx, store_len.get_operation(), Some(reg));
            self.reg_values.insert(reg.0, storage);
            return Ok(storage);
        }
        Err(self.unsupported(
            format!("read of undefined aggregate register %r{}", reg.0),
            self.reg_span(reg),
        ))
    }

    /// `base + offset` bytes as an opaque pointer (a GEP over `i8`).
    fn gep_byte(&mut self, ctx: &mut Context, base: Value, offset: u64, dest: Reg) -> Value {
        let gep = self.gep_byte_op(ctx, base, offset);
        self.append(ctx, gep.get_operation(), Some(dest));
        gep.get_result(ctx)
    }

    /// [`FnLowering::gep_byte`] without a span register (drop paths).
    fn gep_byte_unspanned(&mut self, ctx: &mut Context, base: Value, offset: u64) -> Value {
        let gep = self.gep_byte_op(ctx, base, offset);
        self.append(ctx, gep.get_operation(), None);
        gep.get_result(ctx)
    }

    fn gep_byte_op(&mut self, ctx: &mut Context, base: Value, offset: u64) -> GetElementPtrOp {
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let index = u32::try_from(offset).expect("aggregate offsets fit u32");
        GetElementPtrOp::new(ctx, base, vec![GepIndex::Constant(index)], i8_ty)
    }

    /// `llvm.memcpy.p0.p0.i64(dest, src, len, volatile=false)`.
    fn mem_copy(&mut self, ctx: &mut Context, dest: Value, src: Value, len: u64, span_reg: Reg) {
        if len == 0 {
            return;
        }
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let i64_ty: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i1_ty: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let void = VoidType::get(ctx).to_handle();
        let fn_ty = FuncType::get(ctx, void, vec![ptr_ty, ptr_ty, i64_ty, i1_ty], false);
        let len_value = self.uint_constant(ctx, len);
        let volatile = self.bool_constant(ctx, false);
        let call = CallIntrinsicOp::new(
            ctx,
            StringAttr::new("llvm.memcpy.p0.p0.i64".to_string()),
            fn_ty,
            vec![dest, src, len_value, volatile],
        );
        self.append(ctx, call.get_operation(), Some(span_reg));
    }

    /// Fresh typed scalar storage hoisted to the entry block. Scalar slots
    /// loaded and stored at their own type must carry that element type —
    /// mem2reg promotes an alloca at its element type, and a byte-array slot
    /// would promote as `i8` under typed loads.
    fn entry_typed_alloca(&mut self, ctx: &mut Context, handle: TypeHandle) -> Value {
        let entry = self.entry.expect("lowering is inside a function");
        let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(1, bw(64)));
        let count = ConstantOp::new(ctx, Box::new(attr));
        let alloca = AllocaOp::new(ctx, handle, count.get_result(ctx));
        alloca.get_operation().insert_at_front(entry, ctx);
        count.get_operation().insert_at_front(entry, ctx);
        alloca.get_result(ctx)
    }

    /// Fresh byte storage hoisted to the top of the entry block, so blocks
    /// that execute repeatedly (loops) reuse one slot instead of growing the
    /// stack. Zero-sized storage still allocates one byte for a stable
    /// address.
    fn entry_alloca(&mut self, ctx: &mut Context, size: u64, align: u64) -> Value {
        let entry = self.entry.expect("lowering is inside a function");
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let i64_int = IntegerType::get(ctx, 64, Signedness::Signless);
        let attr = IntegerAttr::new(i64_int, APInt::from_u64(size.max(1), bw(64)));
        let count = ConstantOp::new(ctx, Box::new(attr));
        let alloca = AllocaOp::new(ctx, i8_ty, count.get_result(ctx));
        alloca.set_alignment(ctx, align as u32);
        // Prepend `[count, alloca]` so the storage precedes every use.
        alloca.get_operation().insert_at_front(entry, ctx);
        count.get_operation().insert_at_front(entry, ctx);
        alloca.get_result(ctx)
    }

    /// `print(args…)`: format each argument through the runtime `mjrt_fmt_*`
    /// family (string-literal, Bool, and None text comes from the constant
    /// pool), joined by single spaces with a trailing newline — composing the
    /// same bytes as the VM's `format_value` join (`backend/vm.rs`). The
    /// destination register is `None`-typed and erased.
    fn lower_print(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.write_literal_bytes(ctx, b" ", dest);
            }
            self.print_value(ctx, *arg, dest)?;
        }
        self.write_literal_bytes(ctx, b"\n", dest);
        self.erased.insert(dest.0);
        Ok(())
    }

    /// Emit the display bytes of one `print` argument.
    fn print_value(&mut self, ctx: &mut Context, arg: Reg, dest: Reg) -> Result<(), PlironError> {
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.write_literal_bytes(ctx, &bytes, dest);
            return Ok(());
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            self.write_stdout(ctx, descriptor.data, descriptor.len, dest);
            return Ok(());
        }
        // A nominal String prints its byte buffer (the VM's `write_to`
        // bridge reads the same bytes).
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0)
            && crate::symbol::is_stdlib_string_struct(name)
        {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(());
        }
        // An error value prints its bare message (the VM's `format_value`
        // over `Value::Error`).
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::Error)) {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(());
        }
        // A `None`-typed argument prints its constant text without reading
        // the (erased) register.
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::None)) {
            self.write_literal_bytes(ctx, b"None", dest);
            return Ok(());
        }
        // A runtime StringLiteral value (typed storage) prints its
        // descriptor's bytes.
        if matches!(self.func.reg_types.get(&arg.0), Some(Ty::StringLiteral)) {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, len) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, len, dest);
            return Ok(());
        }
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A bare literal argument materializes at the VM's default kind.
            // A runtime FloatLiteral value rejects: the VM displays its
            // exact rational (`1/10`), which f64 storage cannot reproduce.
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => {
                    if !self.pending_literals.contains_key(&arg.0) {
                        return Err(self.unsupported_reg(
                            "display of a runtime FloatLiteral value".into(),
                            dest,
                        ));
                    }
                    ScalarTy::Float64
                }
                _ => ScalarTy::Int,
            },
        };
        let value = self.reg_value(ctx, arg, ty)?;
        self.print_scalar(ctx, ty, value, dest)
    }

    /// Emit the display bytes of one scalar value: `mjrt_fmt_*` into the
    /// scratch buffer for the numeric kinds (the runtime formats exactly the
    /// VM's display text), pooled `True`/`False` selection for Bool.
    fn print_scalar(
        &mut self,
        ctx: &mut Context,
        ty: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let (data, len) = self.format_scalar(ctx, ty, value, dest)?;
        self.write_stdout(ctx, data, len, dest);
        Ok(())
    }

    /// The display bytes of one scalar value as a `(data, len)` pair. The
    /// numeric kinds live in the shared scratch buffer, valid until the next
    /// formatting call.
    fn format_scalar(
        &mut self,
        ctx: &mut Context,
        ty: ScalarTy,
        value: Value,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        let (symbol, value) = match ty {
            ScalarTy::Int => ("mjrt_fmt_i64", value),
            ScalarTy::UInt => ("mjrt_fmt_u64", value),
            ScalarTy::Float64 => ("mjrt_fmt_f64", value),
            // A `Float32` displays as its f64 view (the VM formats the lane's
            // stored f64 with the same shortest-round-trip rules).
            ScalarTy::Sized(Dtype::Float32) => ("mjrt_fmt_f64", self.f32_to_f64(ctx, value, dest)),
            // Sized integers display their mathematical value.
            ScalarTy::Sized(dtype) => {
                let (_, signed) =
                    crate::runtime::integer_dtype_bits(dtype).expect("Float32 is matched above");
                let wide = self.sized_to_i64(ctx, value, dtype, dest);
                (
                    if signed {
                        "mjrt_fmt_i64"
                    } else {
                        "mjrt_fmt_u64"
                    },
                    wide,
                )
            }
            ScalarTy::Ptr => {
                return Err(self.unsupported_reg("display of a Pointer".into(), dest));
            }
            ScalarTy::Bool => {
                let true_global = self.shared.intern_string(ctx, b"True");
                let false_global = self.shared.intern_string(ctx, b"False");
                let true_ptr = self.global_address(ctx, &true_global, dest);
                let false_ptr = self.global_address(ctx, &false_global, dest);
                let data = SelectOp::new(ctx, value, true_ptr, false_ptr);
                self.append(ctx, data.get_operation(), Some(dest));
                let true_len = self.uint_constant(ctx, 4);
                let false_len = self.uint_constant(ctx, 5);
                let len = SelectOp::new(ctx, value, true_len, false_len);
                self.append(ctx, len.get_operation(), Some(dest));
                return Ok((data.get_result(ctx), len.get_result(ctx)));
            }
        };
        let scratch = self.scratch_buffer(ctx);
        let fmt_ty = self.shared.ensure_rt(ctx, symbol);
        let identifier: Identifier = symbol
            .try_into()
            .expect("runtime symbols are identifier-safe");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(identifier),
            fmt_ty,
            vec![value, scratch],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        Ok((scratch, call.get_result(ctx)))
    }

    /// Intern `bytes` and write them to stdout.
    fn write_literal_bytes(&mut self, ctx: &mut Context, bytes: &[u8], dest: Reg) {
        let global = self.shared.intern_string(ctx, bytes);
        self.write_global(ctx, &global, bytes.len() as u64, dest);
    }

    /// Write `len` bytes of a constant-pool global to stdout.
    fn write_global(&mut self, ctx: &mut Context, global: &Identifier, len: u64, dest: Reg) {
        let data = self.global_address(ctx, global, dest);
        let len = self.uint_constant(ctx, len);
        self.write_stdout(ctx, data, len, dest);
    }

    /// `mjrt_write_stdout(data, len)` — writes exactly the given bytes or
    /// traps (category 4).
    fn write_stdout(&mut self, ctx: &mut Context, data: Value, len: Value, dest: Reg) {
        let write_ty = self.shared.ensure_rt(ctx, "mjrt_write_stdout");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_write_stdout".try_into().expect("valid identifier")),
            write_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(dest));
    }

    /// The address of a module global in the current block.
    fn global_address(&mut self, ctx: &mut Context, global: &Identifier, dest: Reg) -> Value {
        let address = AddressOfOp::new(ctx, global.clone(), 0);
        self.append(ctx, address.get_operation(), Some(dest));
        address.get_result(ctx)
    }

    /// The function's 32-byte formatting buffer (`mjrt_fmt_i64`/`u64` need
    /// at least 20 bytes, `mjrt_fmt_f64` at least 32), created once at the
    /// top of the entry block so loops reuse one slot.
    fn scratch_buffer(&mut self, ctx: &mut Context) -> Value {
        if let Some(scratch) = self.scratch {
            return scratch;
        }
        let value = self.entry_alloca(ctx, 32, 8);
        self.scratch = Some(value);
        value
    }

    /// Compile-time folding of the string-literal operators the VM evaluates
    /// on `Value::Str`: `+` concatenates into a new interned literal, `==` and
    /// `!=` fold to Bool constants. Both operands must be compile-time
    /// literals — no runtime StringLiteral representation exists.
    fn lower_str_literal_binop(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (Some(lhs), Some(rhs)) = (
            self.str_consts.get(&a.0).cloned(),
            self.str_consts.get(&b.0).cloned(),
        ) else {
            return Err(self.unsupported_reg("runtime StringLiteral operand".into(), dest));
        };
        match op {
            InfixOp::Add => {
                let mut bytes = lhs;
                bytes.extend_from_slice(&rhs);
                self.str_consts.insert(dest.0, bytes);
                Ok(())
            }
            InfixOp::Eq | InfixOp::Ne => {
                let equal = lhs == rhs;
                let value = if matches!(op, InfixOp::Eq) {
                    equal
                } else {
                    !equal
                };
                let constant = self.bool_constant(ctx, value);
                self.reg_values.insert(dest.0, constant);
                Ok(())
            }
            other => Err(self.unsupported_reg(
                format!("operator `{other:?}` on StringLiteral operands"),
                dest,
            )),
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
            MirConst::Str(text) => {
                self.str_consts.insert(dest.0, text.as_bytes().to_vec());
                Ok(())
            }
            MirConst::Function(_) | MirConst::None => {
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
    fn exact_literal_storage(
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
        if self.str_consts.contains_key(&a.0) || self.str_consts.contains_key(&b.0) {
            return self.lower_str_literal_binop(ctx, op, dest, a, b);
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
    fn lower_sized_int_binop(
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
    fn lower_f32_binop(
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

    /// Widen a `Float32` SSA value to its f64 view (exact — the VM stores
    /// f32 lanes as f64 views).
    fn f32_to_f64(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f64_ty: TypeHandle = FP64Type::get(ctx).into();
        let cast = FPExtOp::new(ctx, value, f64_ty);
        cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    /// Round an f64 value to single precision (the VM's `round_f32`).
    fn f64_to_f32(&mut self, ctx: &mut Context, value: Value, dest: Reg) -> Value {
        let f32_ty: TypeHandle = FP32Type::get(ctx).into();
        let cast = FPTruncOp::new(ctx, value, f32_ty);
        cast.set_fast_math_flags(ctx, FastmathFlagsAttr::default());
        self.append(ctx, cast.get_operation(), Some(dest));
        cast.get_result(ctx)
    }

    /// A sized integer lane's mathematical value as i64: sign-extend a
    /// signed lane, zero-extend an unsigned one (the VM's i128 lane content,
    /// which always fits i64 bits for 64-bit-and-under lanes).
    fn sized_to_i64(&mut self, ctx: &mut Context, value: Value, dtype: Dtype, dest: Reg) -> Value {
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
    fn resize_int(
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

    /// Replace the divisor with `1` in the single overflowing signed case
    /// (`lhs == i64::MIN && rhs == -1`): LLVM `sdiv`/`srem` are poison there,
    /// while the ABI defines the wrapped results `i64::MIN` and `0` — exactly
    /// what the floor expansions produce for a divisor of `1`.
    fn sanitized_divisor(
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

    /// The function's trap block for `category`: `mjrt_trap(code)` (which
    /// reports on stderr and exits `64 + code`) then `unreachable`, created on
    /// first use.
    fn trap_block(&mut self, ctx: &mut Context, category: TrapCategory) -> Ptr<BasicBlock> {
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
            MirTerm::Return(value) => self.lower_return_edge(ctx, value.as_ref().copied(), &[]),
            MirTerm::ReturnWithCleanup { value, cleanup } => {
                self.lower_return_edge(ctx, value.as_ref().copied(), cleanup)
            }
            MirTerm::FallOff => {
                let Some(target) = self.falloff_target else {
                    return Err(self.unsupported("`FallOff` outside a try region".into(), None));
                };
                let jump = BrOp::new(ctx, target, vec![]);
                self.append(ctx, jump.get_operation(), None);
                Ok(())
            }
            MirTerm::EscapeJump { target, cleanup } => {
                self.lower_escape_edge(ctx, *target, cleanup)
            }
        }
    }

    /// A return terminator: with no finalbody in the way, enclosing
    /// `Try.cleanup` lists run inner to outer, then the return's own carried
    /// roots, then the value returns. Crossing a finalbody (or overriding
    /// one's pending outcome) instead stages the value, registers an exit
    /// site, and routes outward through the pending machinery.
    fn lower_return_edge(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        let crosses_finally = self.try_frames.iter().any(|frame| frame.finally.is_some());
        if !crosses_finally && self.finally_overrides.is_empty() {
            self.emit_scope_exit_cleanups(ctx, cleanup)?;
            return self.lower_return(ctx, value);
        }
        // A value-less return inside a value-returning function is
        // checker-guaranteed unreachable fall-off scaffolding.
        let signature = &self.signatures[self.name];
        let value_returning = match &signature.outcome {
            Some(outcome) => !matches!(outcome.ok, LowerTy::ZeroSized),
            None => signature.returns_value || signature.sret.is_some(),
        };
        if value.is_none() && value_returning {
            let unreachable = UnreachableOp::new(ctx);
            self.append(ctx, unreachable.get_operation(), None);
            return Ok(());
        }
        self.stage_return_value(ctx, value)?;
        let code = 2 + self.exit_sites.len() as u32;
        self.exit_sites.push(ExitSiteInfo {
            action: ExitAction::Return {
                cleanup: cleanup.to_vec(),
            },
            overrides: self.finally_overrides.clone(),
            terminal: None,
        });
        self.emit_exit_crossing(ctx, code)
    }

    /// A `break`/`continue` escaping to an enclosing-function block: the
    /// edge's own cleanup runs first (the VM's `run_region`), then the exit
    /// crosses enclosing frames — pending on finalbodies on the way — until
    /// the function-level target. An escape inside a finalbody resolves the
    /// overridden pending outcome at the site (the VM runs the pending
    /// return's roots before propagating the jump).
    fn lower_escape_edge(
        &mut self,
        ctx: &mut Context,
        target: usize,
        cleanup: &[u32],
    ) -> Result<(), PlironError> {
        for &var in cleanup {
            self.lower_drop_var(ctx, var)?;
        }
        let crosses_finally = self.try_frames.iter().any(|frame| frame.finally.is_some());
        if !crosses_finally && self.finally_overrides.is_empty() {
            self.emit_scope_exit_cleanups(ctx, &[])?;
            let Some(&block) = self.function_blocks.get(target) else {
                return Err(self.unsupported(format!("escape to missing block bb{target}"), None));
            };
            let jump = BrOp::new(ctx, block, vec![]);
            self.append(ctx, jump.get_operation(), None);
            return Ok(());
        }
        let overrides = self.finally_overrides.clone();
        for idx in overrides.into_iter().rev() {
            self.emit_pending_resolution(ctx, idx)?;
        }
        let code = 2 + self.exit_sites.len() as u32;
        self.exit_sites.push(ExitSiteInfo {
            action: ExitAction::Escape { target },
            overrides: Vec::new(),
            terminal: None,
        });
        self.emit_exit_crossing(ctx, code)
    }

    /// The function-exit half of a return terminator (scope-exit cleanups
    /// already ran): store/copy the value per the return ABI and return.
    fn lower_return(&mut self, ctx: &mut Context, value: Option<Reg>) -> Result<(), PlironError> {
        self.emit_frame_exit_error_releases(ctx)?;
        if let Some(outcome) = self.signatures[self.name].outcome.clone() {
            return self.lower_raising_return(ctx, value, &outcome);
        }
        // A value-less return inside a value-returning function is
        // checker-guaranteed unreachable fall-off scaffolding.
        let ret_lower = self.return_value_lower()?;
        let lowered = match (value, ret_lower) {
            (Some(reg), Some(LowerTy::Aggregate { layout, .. })) => {
                // Copy the returned aggregate into the sret out-pointer and
                // return void; the caller owns it.
                let sret = self
                    .sret_ptr
                    .expect("aggregate-returning functions receive an sret pointer");
                let ptr = self.reg_ptr(ctx, reg)?;
                self.mem_copy(ctx, sret, ptr, layout.size, reg);
                self.owned_temps.remove(&reg.0);
                None
            }
            (Some(reg), Some(LowerTy::Scalar(expected))) => {
                Some(self.reg_value(ctx, reg, expected)?)
            }
            (Some(_), Some(LowerTy::ZeroSized)) => None,
            (None, Some(_)) => {
                let unreachable = UnreachableOp::new(ctx);
                self.append(ctx, unreachable.get_operation(), None);
                return Ok(());
            }
            (_, None) => None,
        };
        let ret = ReturnOp::new(ctx, lowered);
        self.append(ctx, ret.get_operation(), value);
        Ok(())
    }

    /// A normal return from a raising function: store the ok payload into the
    /// outcome, tag it `MJ_TAG_OK`, and return void.
    fn lower_raising_return(
        &mut self,
        ctx: &mut Context,
        value: Option<Reg>,
        outcome: &OutcomeAbi,
    ) -> Result<(), PlironError> {
        let outcome_ptr = self
            .outcome_ptr
            .expect("raising functions receive an outcome pointer");
        match (&outcome.ok, value) {
            (LowerTy::ZeroSized, _) => {}
            (_, None) => {
                // A value-less return inside a value-returning function is
                // checker-guaranteed unreachable fall-off scaffolding.
                let unreachable = UnreachableOp::new(ctx);
                self.append(ctx, unreachable.get_operation(), None);
                return Ok(());
            }
            (LowerTy::Scalar(expected), Some(reg)) => {
                let value = self.reg_value(ctx, reg, *expected)?;
                let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(reg));
            }
            (LowerTy::Aggregate { layout, .. }, Some(reg)) => {
                let size = layout.size;
                let ptr = self.reg_ptr(ctx, reg)?;
                let address = self.offset_address(ctx, outcome_ptr, outcome.ok_offset);
                self.mem_copy(ctx, address, ptr, size, reg);
                // The caller owns the payload now.
                self.owned_temps.remove(&reg.0);
            }
        }
        let tag = self.tag_constant(ctx, crate::native::rt_abi::MJ_TAG_OK);
        let store = StoreOp::new(ctx, tag, outcome_ptr);
        self.append(ctx, store.get_operation(), None);
        let ret = ReturnOp::new(ctx, None);
        self.append(ctx, ret.get_operation(), None);
        Ok(())
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

    /// Emit an f32 constant in the current block and return its value.
    fn f32_constant(&mut self, ctx: &mut Context, value: f32) -> Value {
        let attr = FPSingleAttr::from(value);
        let op = ConstantOp::new(ctx, Box::new(attr));
        self.append(ctx, op.get_operation(), None);
        op.get_result(ctx)
    }

    /// Emit an integer constant at a sized lane width, carrying `value`'s
    /// low `bits` bits.
    fn sized_int_constant(&mut self, ctx: &mut Context, dtype: Dtype, value: u64) -> Value {
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

    fn var_lower_ty(&self, var: u32) -> Result<LowerTy, PlironError> {
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

/// The LLVM-dialect type of one runtime-contract primitive. LLVM integers
/// are signless, so `U64` and `I64` share `i64`; pointers are opaque.
fn c_abi_type(ctx: &mut Context, ty: crate::native::rt_abi::CAbiTy) -> TypeHandle {
    use crate::native::rt_abi::CAbiTy;
    match ty {
        CAbiTy::U32 => IntegerType::get(ctx, 32, Signedness::Signless).into(),
        CAbiTy::U64 | CAbiTy::I64 => IntegerType::get(ctx, 64, Signedness::Signless).into(),
        CAbiTy::F64 => FP64Type::get(ctx).into(),
        CAbiTy::PtrConstU8 | CAbiTy::PtrMutU8 => PointerType::get(ctx, 0).into(),
    }
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
        Ty::Simd { dtype, width: 1 } => Ok(ScalarTy::of_dtype(*dtype)),
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

/// Every register `instr` reads (operands, not destinations), for last-use
/// bookkeeping. Instructions outside the supported subset reject before any
/// owned temporary could reach them, so their operands may be approximate.
/// Record final operand appearances over `blocks` (whose position-space ids
/// are `ids`), recursing into `try` regions. Each region's blocks take the
/// next contiguous ids at the moment its `try` is reached, regions in
/// body → handler → orelse → finalbody order — mirroring `lower_region`'s
/// assignment exactly so positions agree.
fn record_last_uses(
    last_uses: &mut HashMap<u32, (usize, usize)>,
    blocks: &[MirBlock],
    ids: &[usize],
    next_id: &mut usize,
) {
    fn record_region(
        last_uses: &mut HashMap<u32, (usize, usize)>,
        blocks: &[MirBlock],
        next_id: &mut usize,
    ) {
        let ids: Vec<usize> = (*next_id..*next_id + blocks.len()).collect();
        *next_id += blocks.len();
        record_last_uses(last_uses, blocks, &ids, next_id);
    }
    for (i, block) in blocks.iter().enumerate() {
        for (index, instr) in block.instrs.iter().enumerate() {
            for reg in operand_regs(instr) {
                last_uses.insert(reg.0, (ids[i], index));
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                record_region(last_uses, body, next_id);
                if let Some((_, handler_blocks)) = handler {
                    record_region(last_uses, handler_blocks, next_id);
                }
                if let Some(orelse_blocks) = orelse {
                    record_region(last_uses, orelse_blocks, next_id);
                }
                if let Some(final_blocks) = finalbody {
                    record_region(last_uses, final_blocks, next_id);
                }
            }
        }
        for reg in terminator_regs(&block.term) {
            last_uses.insert(reg.0, (ids[i], usize::MAX));
        }
    }
}

fn operand_regs(instr: &MirInstr) -> Vec<Reg> {
    fn place_regs(place: &MirPlace, out: &mut Vec<Reg>) {
        for proj in &place.proj {
            if let Proj::Index(reg) = proj {
                out.push(*reg);
            }
        }
    }
    let mut out = Vec::new();
    match instr {
        MirInstr::MaterializeLiteral { value, .. } => out.push(*value),
        MirInstr::UnOp { a, .. } => out.push(*a),
        MirInstr::BinOp { a, b, .. } => out.extend([*a, *b]),
        MirInstr::DefVar { src, .. } => out.push(*src),
        MirInstr::CopyValue { value, .. } => out.push(*value),
        MirInstr::LoadPlace { place, .. } | MirInstr::MovePlace { place, .. } => {
            place_regs(place, &mut out);
        }
        MirInstr::Store { place, src } => {
            place_regs(place, &mut out);
            out.push(*src);
        }
        MirInstr::GetField { base, .. } => out.push(*base),
        MirInstr::MakeTuple { elems, .. } => out.extend(elems.iter().copied()),
        MirInstr::Index { base, index, .. } => out.extend([*base, *index]),
        MirInstr::Call { args, kwargs, .. } => {
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::MethodCall {
            recv, args, kwargs, ..
        } => {
            out.push(*recv);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, reg)| *reg));
        }
        MirInstr::Raise { src } => out.push(*src),
        _ => {}
    }
    out
}

/// Every register a terminator reads.
fn terminator_regs(term: &MirTerm) -> Vec<Reg> {
    match term {
        MirTerm::Branch { cond, .. } => vec![*cond],
        MirTerm::Return(Some(reg)) => vec![*reg],
        _ => Vec::new(),
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
