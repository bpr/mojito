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
    UnreachableOp, XorOp, ZExtOp, ZeroOp,
};
use pliron_llvm::types::{ArrayType, FuncType, PointerType, VoidType};

use crate::ast::{ArgConvention, Dtype, InfixOp, PrefixOp};
use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::checked::CheckedConst;
use crate::literal::{FloatLiteral, IntLiteral};
use crate::mir::{
    Const as MirConst, MirBlock, MirBlockId, MirCaptureMode, MirClosureCapture, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirStructDeclaration, MirTerm, Proj, Reg, UseMode,
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
    /// Whether the ok payload is a place pointer rather than the value — the
    /// reference-returning raising `__next__` convention. Only `TryNext`
    /// consumes this shape; the generic raising-call path rejects it.
    pub ok_is_reference: bool,
}

/// The physical ABI of an indirect call, derived from the callee register's
/// checked `Ty::Func` contract by the same classification rules
/// [`declare_function`] applies to a compiled callee. The `invoke` thunk a
/// callable value carries has exactly this signature with the environment
/// pointer prepended after the out-pointer: `[outcome*|sret*], env*,
/// params...` (never both an sret and an outcome — the inherited invariant).
pub(super) struct ContractAbi {
    /// The `invoke` call type, including the `env*` parameter.
    pub func_ty: TypedHandle<FuncType>,
    pub returns_value: bool,
    pub params: Vec<LowerTy>,
    pub sret: Option<Layout>,
    pub outcome: Option<OutcomeAbi>,
    /// Whether each contract parameter is consuming (`var`/`deinit`).
    pub owned_params: Vec<bool>,
    /// Whether each contract parameter is a `mut`/`ref`/`out` reference.
    pub ref_params: Vec<bool>,
    /// Structural binding facts, for keyword-argument slot matching.
    pub names: Vec<String>,
    pub required: Vec<bool>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
}

/// Module-level lowering state shared by every function: the module itself
/// plus the lazily declared runtime-contract symbols (trap blocks call
/// `mjrt_trap`), the emitted `mjrt_pow` helper, and the interned per-target
/// `invoke` thunks of retained callables (`mjthunk_<n>`). The `mjrt_` and
/// `mjthunk_` prefixes, like `main`, are outside the injective `mj_` mangle
/// image (see `mangle`).
pub(super) struct ModuleShared {
    module: ModuleOp,
    rt_types: HashMap<&'static str, TypedHandle<FuncType>>,
    strings: HashMap<Vec<u8>, Identifier>,
    pow_ty: Option<TypedHandle<FuncType>>,
    /// Interned callable thunks, keyed by (mangled target, capture-mode
    /// string — `r`/`c`/`m` per leading capture parameter). One lifted body
    /// gets distinct thunks per mode vector because declaration sites
    /// capture with real modes while in-body forwarding sites re-capture
    /// everything by reference.
    thunks: HashMap<(String, String), Identifier>,
    /// Interned capture-record teardown thunks (`mjdrop_<n>`), keyed like
    /// [`Self::thunks`] by (target, capture-mode string).
    drop_thunks: HashMap<(String, String), Identifier>,
}

impl ModuleShared {
    pub(super) fn new(module: ModuleOp) -> ModuleShared {
        ModuleShared {
            module,
            rt_types: HashMap::new(),
            strings: HashMap::new(),
            pow_ty: None,
            thunks: HashMap::new(),
            drop_thunks: HashMap::new(),
        }
    }

    /// Whether body lowering already declared the runtime symbol (the
    /// executable wrapper must not redeclare it).
    pub(super) fn declared_rt(&self, symbol: &str) -> bool {
        self.rt_types.contains_key(symbol)
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

    /// Intern the `invoke` thunk adapting compiled `target` to the uniform
    /// indirect-call ABI and return its symbol. `modes` encodes the leading
    /// capture parameters (`r` reference / `c` copy / `m` move — empty for a
    /// bare function value); `capture_offsets` gives each capture's byte
    /// offset in the environment record. The thunk takes
    /// `[outcome*|sret*], env*, params...`, rebuilds the target's leading
    /// capture arguments from the record — a `Reference` slot holds the
    /// captured place's address (loaded), an owned slot holds the value
    /// inline (its address is the argument, since every capture parameter is
    /// a reference parameter) — and forwards everything else unchanged,
    /// out-pointer included, so a raising target's tagged outcome flows
    /// through untouched.
    fn ensure_thunk(
        &mut self,
        ctx: &mut Context,
        target: &FnSignature,
        modes: &str,
        capture_offsets: &[u64],
    ) -> Identifier {
        let key = (target.mangled.clone(), modes.to_string());
        if let Some(name) = self.thunks.get(&key) {
            return name.clone();
        }
        let name: Identifier = format!("mjthunk_{}", self.thunks.len())
            .try_into()
            .expect("thunk names are identifier-safe");
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let captures = modes.len();
        let has_out = target.sret.is_some() || target.outcome.is_some();
        let mut param_handles: Vec<TypeHandle> = Vec::new();
        if has_out {
            param_handles.push(ptr_ty);
        }
        param_handles.push(ptr_ty);
        for (index, param) in target.params.iter().enumerate().skip(captures) {
            let by_reference = target.ref_params.get(index).copied().unwrap_or(false);
            match param {
                _ if by_reference => param_handles.push(ptr_ty),
                LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
                LowerTy::Aggregate { .. } => param_handles.push(ptr_ty),
                LowerTy::ZeroSized => {}
            }
        }
        let result = target.func_ty.deref(ctx).result_type();
        let thunk_ty = FuncType::get(ctx, result, param_handles, false);
        let func = FuncOp::new(ctx, name.clone(), thunk_ty);
        self.module.append_operation(ctx, func.get_operation(), 0);
        emit_thunk_body(ctx, func, target, modes, capture_offsets, has_out);
        self.thunks.insert(key, name.clone());
        name
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
            // A raising reference return rides the tagged outcome with a
            // single place pointer as the ok payload (the reference-yielding
            // raising `__next__` convention).
            let pointer = Ty::Pointer {
                element: Box::new(ret_ty.clone()),
                origin: crate::origin::PointerOrigin::Untracked { mutable: true },
            };
            let composed = layout
                .outcome_layout(&pointer)
                .map_err(|error| PlironError {
                    function: Some(name.to_string()),
                    kind: PlironErrorKind::Unsupported {
                        construct: format!("raising reference return ({error})"),
                    },
                    location: None,
                })?;
            let outcome = OutcomeAbi {
                layout: composed.layout,
                ok_offset: composed.offsets[1],
                err_offset: composed.offsets[2],
                ok: LowerTy::Scalar(ScalarTy::Ptr),
                ok_is_reference: true,
            };
            (
                VoidType::get(ctx).to_handle(),
                false,
                RetKind::Void,
                None,
                Some(outcome),
            )
        } else {
            // A reference returns as one pointer to caller-owned referent
            // storage; the checked return type names the referent.
            (
                PointerType::get(ctx, 0).into(),
                true,
                RetKind::Ptr,
                None,
                None,
            )
        }
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
            ok_is_reference: false,
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
            // A zero-sized parameter (a `NoneType` overload marker like
            // `__list_literal__`) carries no runtime data: it keeps its
            // signature entry but no physical argument.
            LowerTy::ZeroSized => {}
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
        pack_positions: HashMap::new(),
        str_consts: HashMap::new(),
        str_runtime: HashMap::new(),
        owned_temps: HashMap::new(),
        last_uses: HashMap::new(),
        position: (0, 0),
        erased: HashSet::new(),
        partially_moved: HashSet::new(),
        leaf_flags: HashMap::new(),
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
    unhandled_error_declared: bool,
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
    if callees.iter().any(|(_, outcome)| outcome.is_some()) && !unhandled_error_declared {
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
/// `StringLiteral`-descriptor aggregates take their shared-engine layout, as
/// does the two-word `{ invoke, env }` retained-callable value; everything
/// else (multi-lane SIMD, generic callable values) stays outside the
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
        // A retained callable is the two-word `{ invoke, env }` value.
        Ty::Error
        | Ty::Struct(..)
        | Ty::Tuple(_)
        | Ty::RuntimePack(_)
        | Ty::StringLiteral
        | Ty::Func { .. } => match layout.layout_of(ty) {
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
        },
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
    /// Backend-side advance positions of pack-fallback iterator slots
    /// (the slot itself keeps the pack layout), keyed by iterator variable.
    pack_positions: HashMap<u32, Value>,
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
    /// Per-variable `i1` presence flags for top-level leaves (struct fields
    /// or pack elements) that a depth-1 `MovePlace` moves out somewhere in
    /// the body — the VM's `Value::Moved` tombstones. Drops and consumption
    /// destroy exactly the surviving leaves and suppress the whole-value
    /// destructor when any tracked leaf is absent.
    leaf_flags: HashMap<u32, std::collections::BTreeMap<usize, Value>>,
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
        // Zero-sized parameters have no physical argument; later parameters'
        // argument indexes shift left past them.
        let physical_index = |var: usize| {
            arg_offset
                + param_tys[..var]
                    .iter()
                    .filter(|ty| !matches!(ty, Some(LowerTy::ZeroSized)))
                    .count()
        };
        for (var, param_ty) in param_tys.iter().enumerate() {
            match param_ty {
                // A zero-sized parameter's slot is never read (its uses
                // erase); a null placeholder keeps the slot indexes aligned.
                Some(LowerTy::ZeroSized) => {
                    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                    let null = ZeroOp::new(ctx, ptr_ty);
                    self.append(ctx, null.get_operation(), None);
                    self.var_slots.push(null.get_result(ctx));
                }
                // Aggregate and `mut`/`ref` parameter slots alias the
                // incoming pointer (write-through).
                _ if param_by_pointer(var, param_ty) => {
                    let incoming = entry.deref(ctx).get_argument(physical_index(var));
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
            if param_by_pointer(param, param_ty) || matches!(param_ty, Some(LowerTy::ZeroSized)) {
                continue;
            }
            let value = entry.deref(ctx).get_argument(physical_index(param));
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
        // Depth-1 projected moves leave a variable partially initialized;
        // give each moved top-level leaf a presence flag so later drops and
        // consumption destroy exactly the surviving leaves.
        let mut move_places = Vec::new();
        collect_projected_move_places(&self.func.blocks, &mut move_places);
        let leaf_targets: Vec<(u32, usize)> = move_places
            .iter()
            .filter_map(|place| self.leaf_position(place).map(|pos| (place.root, pos)))
            .collect();
        for (var, position) in leaf_targets {
            let LowerTy::Aggregate { ty, .. } = self.var_lower_ty(var)? else {
                continue;
            };
            if !self.needs_drop(&ty) {
                continue;
            }
            if self
                .leaf_flags
                .get(&var)
                .is_some_and(|leaves| leaves.contains_key(&position))
            {
                continue;
            }
            let alloca = AllocaOp::new(ctx, i1, one);
            self.append(ctx, alloca.get_operation(), None);
            let init = self.bool_constant(ctx, true);
            let store = StoreOp::new(ctx, init, alloca.get_result(ctx));
            self.append(ctx, store.get_operation(), None);
            self.leaf_flags
                .entry(var)
                .or_default()
                .insert(position, alloca.get_result(ctx));
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
                // initialized. A tracked top-level leaf clears its presence
                // flag (later drops skip it, like the VM's `Value::Moved`
                // tombstone); anything deeper records the blanket marker so
                // a whole-variable drop refuses destructor work instead of
                // double-freeing.
                if !place.proj.is_empty() {
                    let flagged = self.leaf_position(place).and_then(|position| {
                        self.leaf_flags
                            .get(&place.root)
                            .and_then(|leaves| leaves.get(&position))
                            .copied()
                    });
                    match flagged {
                        Some(flag) => {
                            let absent = self.bool_constant(ctx, false);
                            let store = StoreOp::new(ctx, absent, flag);
                            self.append(ctx, store.get_operation(), None);
                        }
                        None => {
                            self.partially_moved.insert(place.root);
                        }
                    }
                }
                let (address, ty) = self.place_address(ctx, place, *dest)?;
                // A move relocates the bytes — ownership transfers to the
                // destination (the VM's `mem::replace`; no clone runs), so
                // the moved value is an owned temporary until consumed.
                match lower_ty(self.name, &ty, &self.layout, self.reg_span(*dest))? {
                    LowerTy::Scalar(scalar) => {
                        let handle = scalar.handle(ctx);
                        let load = LoadOp::new(ctx, address, handle);
                        self.define(ctx, *dest, load.get_operation(), load.get_result(ctx))
                    }
                    LowerTy::Aggregate { ty, layout } => {
                        let storage = self.entry_alloca(ctx, layout.size, layout.align);
                        self.mem_copy(ctx, storage, address, layout.size, *dest);
                        self.reg_values.insert(dest.0, storage);
                        if self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty) {
                            self.mark_owned_temp(*dest, (*ty).clone())?;
                        }
                        Ok(())
                    }
                    LowerTy::ZeroSized => {
                        self.erased.insert(dest.0);
                        Ok(())
                    }
                }
            }
            MirInstr::Store { place, src } => {
                // The VM overwrites the designated storage without dropping
                // the old value (drop elaboration emits explicit drops), so a
                // plain store/copy is exact.
                let (address, ty) = self.place_address(ctx, place, *src)?;
                self.store_to(ctx, address, &ty, *src)?;
                // A whole-variable store (re)initializes the slot; a store
                // into a tracked leaf restores that leaf's presence.
                if place.proj.is_empty() && place.through.is_none() {
                    self.set_drop_flag(ctx, place.root, true);
                } else if let Some(position) = self.leaf_position(place)
                    && let Some(&flag) = self
                        .leaf_flags
                        .get(&place.root)
                        .and_then(|leaves| leaves.get(&position))
                {
                    let present = self.bool_constant(ctx, true);
                    let store = StoreOp::new(ctx, present, flag);
                    self.append(ctx, store.get_operation(), None);
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
                // Capture accesses are static ownership facts execution
                // erases. Erased type-parameter slots (`value: None`) carry
                // no runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                let _ = capture_accesses;
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) {
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
                    kwarg_places,
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
                // Capture accesses are static ownership facts execution
                // erases. Erased type-parameter slots (`value: None`) carry
                // no runtime data and are permitted; argument places matter
                // only at `mut`/`ref` parameter positions (borrowed read
                // arguments pass their value copy).
                let _ = capture_accesses;
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                self.lower_call(ctx, *dest, &func.0, args, kwargs, arg_places, kwarg_places)
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
            MirInstr::PointerStorageTake {
                dest,
                pointer,
                index,
                element,
            } => self.lower_pointer_storage_take(ctx, *dest, *pointer, *index, element),
            MirInstr::PointerStorageDestroy {
                dest,
                pointer,
                index,
                element,
            } => self.lower_pointer_storage_destroy(ctx, *dest, *pointer, *index, element),
            MirInstr::UninitStorage { dest, init } => self.lower_uninit_storage(ctx, *dest, *init),
            MirInstr::UninitStorageTake {
                dest,
                storage,
                element,
            } => self.lower_uninit_storage_take(ctx, *dest, *storage, element),
            MirInstr::UninitStorageDestroy {
                dest,
                storage,
                element,
            } => self.lower_uninit_storage_destroy(ctx, *dest, *storage, element),
            MirInstr::Index {
                dest,
                base,
                index,
                base_place,
                index_place,
                call: Some(call),
                intrinsic: _,
            } => {
                // A parameterless specialized accessor (a Tuple element
                // getter) takes only `self`; an overloaded `__getitem__`
                // receives the runtime index — the VM's `call.arguments`
                // distinction.
                let positional = if call.arguments.is_empty() {
                    Vec::new()
                } else {
                    vec![SubscriptActual::Reg(*index, index_place.as_ref())]
                };
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *base,
                    base_place.as_ref(),
                    &positional,
                    &[],
                )
            }
            MirInstr::Index {
                dest,
                base,
                index,
                intrinsic: Some(intrinsic),
                ..
            } => self.lower_index_intrinsic(ctx, *dest, *base, *index, intrinsic),
            MirInstr::Slice {
                dest,
                object,
                lower,
                upper,
                step,
                object_place,
                call: Some(call),
                ..
            } => {
                let descriptor = self.build_slice_descriptor(ctx, *dest, *lower, *upper, *step)?;
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *object,
                    object_place.as_ref(),
                    &[SubscriptActual::Descriptor(descriptor)],
                    &[],
                )
            }
            MirInstr::MultiIndex {
                dest,
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call: Some(call),
            } => {
                let positional = self.subscript_actuals(ctx, *dest, args, arg_places)?;
                let mut keywords = Vec::with_capacity(kwargs.len());
                for (i, (name, arg)) in kwargs.iter().enumerate() {
                    let actual = self.subscript_actual(
                        ctx,
                        *dest,
                        arg,
                        kwarg_places.get(i).and_then(Option::as_ref),
                    )?;
                    keywords.push((name.as_str(), actual));
                }
                self.lower_subscript_call(
                    ctx,
                    *dest,
                    "__getitem__",
                    call,
                    *object,
                    object_place.as_ref(),
                    &positional,
                    &keywords,
                )
            }
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                value,
                value_place,
                value_keyword,
                call,
            } => {
                // The discarded `__setitem__` result binds to a scratch
                // register outside the function's register space.
                let scratch = Reg(u32::MAX);
                let mut positional = self.subscript_actuals(ctx, scratch, args, arg_places)?;
                let mut keywords = Vec::new();
                if *value_keyword {
                    keywords.push(("value", SubscriptActual::Reg(*value, value_place.as_ref())));
                } else {
                    positional.push(SubscriptActual::Reg(*value, value_place.as_ref()));
                }
                self.lower_subscript_call(
                    ctx,
                    scratch,
                    "__setitem__",
                    call,
                    *receiver,
                    receiver_place.as_ref(),
                    &positional,
                    &keywords,
                )
            }
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
            } => self.lower_make_closure(ctx, *dest, function, captures),
            MirInstr::CallIndirect {
                dest,
                callee,
                resolved,
                raises,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls,
                instantiated_contract,
                instantiated_args,
            } => {
                // The contract is authoritative for the raising ABI; the
                // checker-selected nominal target is consumed by
                // monomorphization's devirtualization; capture accesses are
                // static facts execution erases; the callable value itself
                // needs no stable storage natively — its environment record
                // is the stable storage.
                let _ = (
                    resolved,
                    raises,
                    callee_place,
                    capture_accesses,
                    instantiated_args,
                );
                // A generic-callable contract that still carries value
                // parameter arguments or unresolved declarations at lowering
                // is outside the monomorphized subset.
                if param_arg_regs.iter().any(|arg| arg.value.is_some()) || !param_decls.is_empty() {
                    return Err(
                        self.unsupported_reg("generic callable value invocation".into(), *dest)
                    );
                }
                self.lower_call_indirect(
                    ctx,
                    *dest,
                    *callee,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    instantiated_contract.as_ref(),
                )
            }
            MirInstr::Index { dest, .. }
            | MirInstr::Slice { dest, .. }
            | MirInstr::MultiIndex { dest, .. }
            | MirInstr::MakeVariant { dest, .. }
            | MirInstr::VariantIs { dest, .. }
            | MirInstr::VariantGet { dest, .. }
            | MirInstr::VariantTake { dest, .. }
            | MirInstr::VariantReplace { dest, .. }
            | MirInstr::SimdShuffle { dest, .. } => {
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
    #[allow(clippy::too_many_arguments)]
    fn lower_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
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
                return self.lower_constructor(
                    ctx,
                    dest,
                    name,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                );
            }
            // The numeric/IO builtins the VM's `call_named` implements
            // directly. Nominal-receiver `len`/`abs`/`round` were rewritten
            // to `__len__`/`__abs__`/`__round__` method calls during
            // monomorphization; only the scalar/pack forms arrive here.
            match name {
                "len" => return self.lower_len_builtin(ctx, dest, args, kwargs),
                "abs" => return self.lower_abs_builtin(ctx, dest, args, kwargs),
                "min" | "max" => {
                    return self.lower_min_max_builtin(ctx, dest, name == "min", args, kwargs);
                }
                "round" => return self.lower_round_builtin(ctx, dest, args, kwargs),
                "divmod" => return self.lower_divmod_builtin(ctx, dest, args, kwargs),
                "input" => return self.lower_input_builtin(ctx, dest, args, kwargs),
                "UnsafePointer.alloc" if kwargs.is_empty() && args.len() == 1 => {
                    return self.lower_alloc_core(ctx, dest, args[0], None);
                }
                "UnsafePointer.alloc_aligned" => {
                    let alignment = match (args, kwargs) {
                        ([_, alignment], []) => *alignment,
                        ([_], [(name, alignment)]) if name == "alignment" => *alignment,
                        _ => {
                            return Err(
                                self.unsupported_reg("allocation call contract".into(), dest)
                            );
                        }
                    };
                    return self.lower_alloc_core(ctx, dest, args[0], Some(alignment));
                }
                "UnsafePointer.unsafe_dangling" | "Pointer.unsafe_dangling"
                    if args.is_empty() && kwargs.is_empty() =>
                {
                    return self.lower_dangling_builtin(ctx, dest);
                }
                "_mojito_abort" if args.len() == 1 && kwargs.is_empty() => {
                    return self.lower_abort_builtin(ctx, dest, args[0]);
                }
                _ => {}
            }
            return Err(self.unsupported_reg(
                format!("call to unknown or builtin function `{name}`"),
                dest,
            ));
        }

        let params = self.signatures[name].params.clone();
        let owned = self.signatures[name].owned_params.clone();
        let by_reference = self.signatures[name].ref_params.clone();
        // A direct call to a compiled `__init__` (the checker's specialized
        // constructor symbols and their mono instances) binds its destination
        // as the `out self` receiver: allocate the result storage and bind
        // the remaining arguments past the receiver — the struct-name
        // constructor path's exact contract.
        if name.contains(".__init__")
            && !params.is_empty()
            && let Some(struct_ty @ Ty::Struct(..)) = self.func.reg_types.get(&dest.0).cloned()
        {
            let lowered = lower_ty(self.name, &struct_ty, &self.layout, self.reg_span(dest))?;
            let LowerTy::Aggregate { layout, .. } = lowered else {
                return Err(self.unsupported_reg(format!("constructor result `{struct_ty}`"), dest));
            };
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let rest = &params[1..];
            let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
            let rest_by_reference = if by_reference.len() > 1 {
                &by_reference[1..]
            } else {
                &[]
            };
            let mut lowered = vec![storage];
            // A variadic callee always binds through the slot matcher: an
            // argument count that happens to equal the physical parameter
            // count (arity one against the single pack slot) must still
            // build pack storage, never pass the argument as the pack.
            if kwargs.is_empty()
                && args.len() == rest.len()
                && !rest_by_reference.iter().any(|&by_ref| by_ref)
                && !self.variadic_callee(name)
            {
                for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                    let owned = rest_owned.get(i).copied().unwrap_or(false);
                    lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
                }
            } else {
                lowered.extend(self.bind_call_slots(
                    ctx,
                    dest,
                    name,
                    rest,
                    rest_owned,
                    rest_by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?);
            }
            self.emit_bound_call(ctx, dest, name, lowered)?;
            // `__init__` returns nothing; the constructed value is the
            // storage its `out self` wrote through.
            self.erased.remove(&dest.0);
            self.reg_values.insert(dest.0, storage);
            return Ok(());
        }
        let lowered_args =
            if kwargs.is_empty() && args.len() == params.len() && !self.variadic_callee(name) {
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
                self.bind_call_slots(
                    ctx,
                    dest,
                    name,
                    &params,
                    &owned,
                    &by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?
            };
        self.emit_bound_call(ctx, dest, name, lowered_args)
    }

    /// Resolve keyword arguments and constant defaults into the callee's
    /// positional parameter order via `call::match_call_slots` — the same
    /// structural binding the VM applies (`src/call.rs` owns the policy).
    /// `params`, `owned`, and `by_reference` are the expected slices of value
    /// parameters: a method or constructor caller passes its signature minus
    /// the receiver. A `mut`/`ref` slot passes the address of its checked
    /// place, taken from the source array the matched slot names —
    /// `arg_places[p]` for `Positional(p)`, `kwarg_places[k]` for
    /// `Keyword(k)` — never the parameter position.
    #[allow(clippy::too_many_arguments)]
    fn bind_call_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        params: &[LowerTy],
        owned: &[bool],
        by_reference: &[bool],
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<Vec<Value>, PlironError> {
        let Some(decl) = self.declarations.get(name) else {
            return Err(self.unsupported_reg(
                format!("call to `{name}` without a recorded declaration"),
                dest,
            ));
        };
        if decl.kw_variadic.is_some() {
            return Err(self.unsupported_reg(format!("keyword-variadic call to `{name}`"), dest));
        }
        let variadic = decl.variadic.clone().map(|ty| (ty, decl.variadic_index));
        let kw_names: Vec<&str> = kwargs.iter().map(|(n, _)| n.as_str()).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: variadic.is_some(),
                keyword: false,
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("call binding for `{name}` failed: {error:?}"), dest)
        })?;
        let defaults = decl.defaults.clone();
        // The physical parameter list is the named parameters with the
        // collected pack inserted at `variadic_index` (the VM's `bind_args`
        // packs positional overflow into one tuple-shaped argument).
        let pack = match variadic {
            None => None,
            Some((pack_ty, index)) => {
                let Some(index) = index else {
                    return Err(self.unsupported_reg(
                        format!("variadic call to `{name}` without a recorded pack position"),
                        dest,
                    ));
                };
                let elements = match &pack_ty {
                    Ty::RuntimePack(elements) | Ty::Tuple(elements) => elements.clone(),
                    other => {
                        return Err(self.unsupported_reg(
                            format!("variadic call to `{name}` over unspecialized `{other}`"),
                            dest,
                        ));
                    }
                };
                if matched.positional_overflow.len() != elements.len() {
                    return Err(self.unsupported_reg(
                        format!(
                            "variadic call to `{name}`: {} overflow arguments against a \
                             {}-element pack",
                            matched.positional_overflow.len(),
                            elements.len()
                        ),
                        dest,
                    ));
                }
                let composed = self.struct_layout_of(&elements, dest)?;
                let storage = self.entry_alloca(
                    ctx,
                    composed.layout.size.max(1),
                    composed.layout.align.max(1),
                );
                for ((arg, element), offset) in matched
                    .positional_overflow
                    .iter()
                    .zip(&elements)
                    .zip(&composed.offsets)
                {
                    let address = if *offset == 0 {
                        storage
                    } else {
                        self.gep_byte(ctx, storage, *offset, dest)
                    };
                    // Overflow arguments relocate into the pack (the VM's
                    // `Tuple(*args^)` move); `store_to` transfers owned
                    // temporaries and forks borrowed heap-owners.
                    self.store_to(ctx, address, element, args[*arg])?;
                }
                Some((index, storage))
            }
        };
        let named = matched.slots.len() + usize::from(pack.is_some());
        if named != params.len() {
            return Err(self.unsupported_reg(
                format!("call binding for `{name}` disagrees with its compiled arity"),
                dest,
            ));
        }
        let mut lowered = Vec::with_capacity(params.len());
        let mut slots = matched.slots.iter().enumerate();
        for (i, param) in params.iter().enumerate() {
            if let Some((pack_index, storage)) = pack
                && i == pack_index
            {
                lowered.push(storage);
                continue;
            }
            let Some((slot_index, slot)) = slots.next() else {
                return Err(self.unsupported_reg(
                    format!("call binding for `{name}` disagrees with its compiled arity"),
                    dest,
                ));
            };
            let expected = param.clone();
            // A zero-sized marker parameter (`__list_literal__`) has no
            // physical operand; its slot is consumed and skipped.
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let owned = owned.get(i).copied().unwrap_or(false);
            let by_ref = by_reference.get(i).copied().unwrap_or(false);
            let place_address = |lowering: &mut Self,
                                 ctx: &mut Context,
                                 place: Option<&MirPlace>|
             -> Result<Value, PlironError> {
                let Some(place) = place.cloned() else {
                    return Err(lowering.unsupported_reg(
                        format!("`mut`/`ref` argument of `{name}` without a place"),
                        dest,
                    ));
                };
                Ok(lowering.place_address(ctx, &place, dest)?.0)
            };
            let value = match slot {
                ArgSlot::Positional(p) if by_ref => {
                    place_address(self, ctx, arg_places.get(*p).and_then(Option::as_ref))?
                }
                ArgSlot::Keyword(k) if by_ref => {
                    place_address(self, ctx, kwarg_places.get(*k).and_then(Option::as_ref))?
                }
                ArgSlot::Positional(p) => self.arg_value(ctx, args[*p], &expected, owned, dest)?,
                ArgSlot::Keyword(k) => self.arg_value(ctx, kwargs[*k].1, &expected, owned, dest)?,
                ArgSlot::Default => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("defaulted `mut`/`ref` parameter of `{name}`"),
                            dest,
                        ));
                    }
                    let Some(default) = defaults.get(slot_index).and_then(Option::as_ref) else {
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

    /// Derive the physical indirect-call ABI from a checked `Ty::Func`
    /// contract, by the same classification rules `declare_function` applies
    /// to a compiled callee (a raising contract returns through a prepended
    /// outcome out-pointer, an aggregate return through prepended sret
    /// storage, never both). Contract shapes the thunk cannot bind reject
    /// contextually.
    fn contract_abi(
        &mut self,
        ctx: &mut Context,
        contract: &Ty,
        dest: Reg,
    ) -> Result<ContractAbi, PlironError> {
        let Ty::Func {
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            conventions,
            ref_params,
            ref_return,
            ..
        } = contract
        else {
            let construct = match contract {
                Ty::GenericFunc { .. } => "generic callable value invocation".to_string(),
                other => format!("indirect call through `{other}`"),
            };
            return Err(self.unsupported_reg(construct, dest));
        };
        if variadic.is_some() {
            return Err(self.unsupported_reg("variadic indirect-call contract".into(), dest));
        }
        if kw_variadic.is_some() {
            return Err(
                self.unsupported_reg("keyword-variadic indirect-call contract".into(), dest)
            );
        }
        if ref_return.is_some() {
            return Err(self.unsupported_reg("reference-returning indirect call".into(), dest));
        }
        let (result, returns_value, sret, outcome) = if *raises {
            let ok = lower_ty(self.name, ret, &self.layout, self.reg_span(dest))?;
            let composed = self.layout.outcome_layout(ret).map_err(|error| {
                self.unsupported_reg(
                    format!("raising indirect return of `{ret}` ({error})"),
                    dest,
                )
            })?;
            let outcome = OutcomeAbi {
                layout: composed.layout,
                ok_offset: composed.offsets[1],
                err_offset: composed.offsets[2],
                ok,
                ok_is_reference: false,
            };
            (VoidType::get(ctx).to_handle(), false, None, Some(outcome))
        } else {
            match lower_ty(self.name, ret, &self.layout, self.reg_span(dest))? {
                LowerTy::ZeroSized => (VoidType::get(ctx).to_handle(), false, None, None),
                LowerTy::Scalar(scalar) => (scalar.handle(ctx), true, None, None),
                LowerTy::Aggregate { layout, .. } => {
                    (VoidType::get(ctx).to_handle(), false, Some(layout), None)
                }
            }
        };
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let mut param_handles: Vec<TypeHandle> = Vec::new();
        if sret.is_some() || outcome.is_some() {
            param_handles.push(ptr_ty);
        }
        // The environment pointer rides every indirect call (null for a
        // bare function value); the thunk unpacks it.
        param_handles.push(ptr_ty);
        let mut lowered_params = Vec::with_capacity(params.len());
        let mut owned_params = Vec::with_capacity(params.len());
        let mut by_reference = Vec::with_capacity(params.len());
        for (index, ty) in params.iter().enumerate() {
            let lowered = lower_ty(self.name, ty, &self.layout, self.reg_span(dest))?;
            let convention = conventions.get(index).copied().flatten();
            let by_ref = matches!(
                convention,
                Some(ArgConvention::Mut | ArgConvention::Ref | ArgConvention::Out)
            ) || ref_params.get(index).is_some_and(Option::is_some);
            let owned = matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit));
            match &lowered {
                _ if by_ref => param_handles.push(ptr_ty),
                LowerTy::Scalar(scalar) => param_handles.push(scalar.handle(ctx)),
                LowerTy::Aggregate { .. } => param_handles.push(ptr_ty),
                LowerTy::ZeroSized => {}
            }
            lowered_params.push(lowered);
            owned_params.push(owned);
            by_reference.push(by_ref);
        }
        let func_ty = FuncType::get(ctx, result, param_handles, false);
        Ok(ContractAbi {
            func_ty,
            returns_value,
            params: lowered_params,
            sret,
            outcome,
            owned_params,
            ref_params: by_reference,
            names: names.clone(),
            required: required.clone(),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
        })
    }

    /// Check that the physical shape an indirect caller derives from its
    /// contract agrees with the compiled target the thunk forwards to —
    /// out-pointer kind, parameter classification, and reference-ness must
    /// match slot for slot after the capture prefix, or the call would pass
    /// values where pointers are expected. Checked programs agree here; a
    /// disagreement is surfaced as a contextual rejection, never a silent
    /// miscompile.
    fn check_contract_target(
        &self,
        abi: &ContractAbi,
        target: &FnSignature,
        captures: usize,
        name: &str,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let disagree = |lowering: &Self| -> PlironError {
            lowering.unsupported_reg(
                format!("indirect-call contract disagreeing with compiled `{name}`"),
                dest,
            )
        };
        if abi.outcome.is_some() != target.outcome.is_some()
            || target.outcome.as_ref().is_some_and(|o| o.ok_is_reference)
            || abi.sret.is_some() != target.sret.is_some()
            || abi.params.len() + captures != target.params.len()
        {
            return Err(disagree(self));
        }
        for (index, param) in abi.params.iter().enumerate() {
            let target_index = captures + index;
            let target_param = &target.params[target_index];
            let contract_ref = abi.ref_params.get(index).copied().unwrap_or(false);
            let target_ref = target
                .ref_params
                .get(target_index)
                .copied()
                .unwrap_or(false);
            let agree = contract_ref == target_ref
                && match (param, target_param) {
                    (LowerTy::Scalar(a), LowerTy::Scalar(b)) => a == b,
                    (LowerTy::Aggregate { .. }, LowerTy::Aggregate { .. }) => true,
                    (LowerTy::ZeroSized, LowerTy::ZeroSized) => true,
                    _ => false,
                };
            if !agree {
                return Err(disagree(self));
            }
        }
        Ok(())
    }

    /// Build the two-word `{ invoke, env }` value of a retained callable:
    /// intern the target's `invoke` thunk and store its address next to the
    /// environment pointer. Bare function values and empty-capture closures
    /// carry a null environment.
    fn lower_make_closure(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        function: &str,
        captures: &[MirClosureCapture],
    ) -> Result<(), PlironError> {
        let Some(contract) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped closure result".into(), dest));
        };
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let signatures = self.signatures;
        let Some(target) = signatures.get(function) else {
            return Err(self.unsupported_reg(format!("closure over uncompiled `{function}`"), dest));
        };
        self.check_contract_target(&abi, target, captures.len(), function, dest)?;
        // The environment record: `{ drop: ptr, slots... }`. A `Reference`
        // slot stores the captured place's address; an owned (`copy`/`move`)
        // slot stores the value inline — the record is the stable storage
        // whose address the invoke thunk passes as the capture's reference
        // parameter (in-place mutation across repeated invocations, the
        // VM's owned-capture re-referencing).
        let pointer_slot = Ty::Pointer {
            element: Box::new(Ty::None),
            origin: crate::origin::PointerOrigin::Untracked { mutable: true },
        };
        let mut modes = String::with_capacity(captures.len());
        let mut slot_tys = vec![pointer_slot.clone()];
        for capture in captures {
            let (mode, slot_ty) = match capture.mode {
                MirCaptureMode::Reference => ('r', pointer_slot.clone()),
                MirCaptureMode::Copy | MirCaptureMode::Move => {
                    let Some(ty) = capture
                        .place
                        .ty
                        .clone()
                        .or_else(|| self.func.var_tys.get(&capture.place.root).cloned())
                    else {
                        return Err(self.unsupported_reg("untyped closure capture".into(), dest));
                    };
                    // The VM's owned capture runs the user's copy/move
                    // constructor; the native record relocates or forks
                    // bytes, so a user-observable constructor rejects. The
                    // nominal String's bridged constructors are exactly the
                    // native fork/relocation semantics.
                    let ctor = if capture.mode == MirCaptureMode::Copy {
                        "__copyinit__"
                    } else {
                        "__moveinit__"
                    };
                    if self.chain_runs_user_lifecycle(&ty, ctor) {
                        return Err(self.unsupported_reg(
                            format!("owned closure capture of `{ty}` with a user `{ctor}`"),
                            dest,
                        ));
                    }
                    if capture.mode == MirCaptureMode::Move && !capture.place.proj.is_empty() {
                        // A projected move capture leaves a residual
                        // aggregate whose partial-drop bookkeeping the
                        // leaf-flag pre-scan does not cover here.
                        return Err(
                            self.unsupported_reg("projected move closure capture".into(), dest)
                        );
                    }
                    (
                        if capture.mode == MirCaptureMode::Copy {
                            'c'
                        } else {
                            'm'
                        },
                        ty,
                    )
                }
            };
            modes.push(mode);
            slot_tys.push(slot_ty);
        }
        let (env, capture_offsets) = if captures.is_empty() {
            let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
            let null = ZeroOp::new(ctx, ptr_ty);
            self.append(ctx, null.get_operation(), Some(dest));
            (null.get_result(ctx), Vec::new())
        } else {
            let composed = self.struct_layout_of(&slot_tys, dest)?;
            let record = self.entry_alloca(ctx, composed.layout.size, composed.layout.align);
            // Header: the per-site drop thunk when some owned slot needs
            // drop work, else null. Re-stored on every execution — a loop
            // re-creating a dropped closure revives the tombstoned header.
            let droppable: Vec<(char, Ty, u64)> = modes
                .chars()
                .zip(&slot_tys[1..])
                .zip(&composed.offsets[1..])
                .map(|((mode, ty), offset)| (mode, ty.clone(), *offset))
                .collect();
            let header = if droppable
                .iter()
                .any(|(mode, ty, _)| *mode != 'r' && self.needs_drop(ty))
            {
                let thunk = self.ensure_capture_drop_thunk(ctx, function, &modes, &droppable)?;
                let address = AddressOfOp::new(ctx, thunk, 0);
                self.append(ctx, address.get_operation(), Some(dest));
                address.get_result(ctx)
            } else {
                let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                let null = ZeroOp::new(ctx, ptr_ty);
                self.append(ctx, null.get_operation(), Some(dest));
                null.get_result(ctx)
            };
            let store_header = StoreOp::new(ctx, header, record);
            self.append(ctx, store_header.get_operation(), Some(dest));
            for (capture, ((mode, ty, _), offset)) in captures
                .iter()
                .zip(droppable.iter().zip(&composed.offsets[1..]))
            {
                let slot = if *offset == 0 {
                    record
                } else {
                    self.gep_byte(ctx, record, *offset, dest)
                };
                let place = capture.place.clone();
                let (source, _) = self.place_address(ctx, &place, dest)?;
                if *mode == 'r' {
                    let store = StoreOp::new(ctx, source, slot);
                    self.append(ctx, store.get_operation(), Some(dest));
                    continue;
                }
                let layout = self.layout.layout_of(ty).map_err(|error| {
                    self.unsupported_reg(format!("closure capture layout ({error})"), dest)
                })?;
                if *mode == 'c' && self.owns_heap(ty) {
                    // A copy capture of a borrowed heap owner forks; a byte
                    // copy would alias buffers both owners release.
                    self.fork_value_into(ctx, slot, ty, layout, source, dest)?;
                } else {
                    self.mem_copy(ctx, slot, source, layout.size, dest);
                }
                if *mode == 'm' {
                    // The VM's move capture runs the compiled stdlib
                    // `__moveinit__` (`move_value`), whose `deinit other`
                    // teardown reports one consume event; the byte
                    // relocation above is that constructor's exact
                    // semantics, so mirror the event.
                    if self.trace_lifecycle
                        && let Ty::Struct(name, _) = ty
                        && self
                            .declarations
                            .contains_key(&format!("{name}.__moveinit__"))
                    {
                        let name = name.clone();
                        self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_CONSUME, &name);
                    }
                    // A whole-root move capture: ownership analysis already
                    // suppressed the source's ordinary drop; clearing the
                    // flag mirrors the VM's tombstoned source.
                    self.set_drop_flag(ctx, place.root, false);
                }
            }
            (record, composed.offsets[1..].to_vec())
        };
        let thunk = self
            .shared
            .ensure_thunk(ctx, target, &modes, &capture_offsets);
        let storage = self.entry_alloca(ctx, 16, 8);
        let invoke = AddressOfOp::new(ctx, thunk, 0);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let store_invoke = StoreOp::new(ctx, invoke.get_result(ctx), storage);
        self.append(ctx, store_invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, storage, 8, dest);
        let store_env = StoreOp::new(ctx, env, env_address);
        self.append(ctx, store_env.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Emit (once per `(target, modes)`) the teardown thunk a capture
    /// record's header names: destroy the owned droppable slots in reverse
    /// capture order (the VM's closure-drop order), then null the header —
    /// drops are idempotent per record, which is what keeps aliasing
    /// two-word copies sound.
    fn ensure_capture_drop_thunk(
        &mut self,
        ctx: &mut Context,
        function: &str,
        modes: &str,
        slots: &[(char, Ty, u64)],
    ) -> Result<Identifier, PlironError> {
        let key = (function.to_string(), modes.to_string());
        if let Some(name) = self.shared.drop_thunks.get(&key) {
            return Ok(name.clone());
        }
        let name: Identifier = format!("mjdrop_{}", self.shared.drop_thunks.len())
            .try_into()
            .expect("thunk names are identifier-safe");
        let void = VoidType::get(ctx).to_handle();
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let thunk_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
        let func = FuncOp::new(ctx, name.clone(), thunk_ty);
        self.shared
            .module
            .append_operation(ctx, func.get_operation(), 0);
        let entry = func.get_or_create_entry_block(ctx);
        let region = func
            .get_operation()
            .deref(ctx)
            .regions()
            .next()
            .expect("llvm.func has a body region");
        // Retarget the emission cursor into the thunk body: the drop chain
        // is ordinary `emit_drop_value` output (trace events included, so
        // capture drops report at drop time like the VM's).
        let saved_current = self.current;
        let saved_region = self.region;
        self.current = Some(entry);
        self.region = Some(region);
        let emit = |lowering: &mut Self, ctx: &mut Context| -> Result<(), PlironError> {
            let env = entry.deref(ctx).get_argument(0);
            for (mode, ty, offset) in slots.iter().rev() {
                if *mode == 'r' || !lowering.needs_drop(ty) {
                    continue;
                }
                let address = if *offset == 0 {
                    env
                } else {
                    lowering.gep_byte_unspanned(ctx, env, *offset)
                };
                lowering.emit_drop_value(ctx, address, ty, false)?;
            }
            let null = ZeroOp::new(ctx, ptr_ty);
            lowering.append(ctx, null.get_operation(), None);
            let tombstone = StoreOp::new(ctx, null.get_result(ctx), env);
            lowering.append(ctx, tombstone.get_operation(), None);
            let ret = ReturnOp::new(ctx, None);
            lowering.append(ctx, ret.get_operation(), None);
            Ok(())
        };
        let emitted = emit(self, ctx);
        self.current = saved_current;
        self.region = saved_region;
        emitted?;
        self.shared.drop_thunks.insert(key, name.clone());
        Ok(name)
    }

    /// Call through a retained callable value: bind the arguments against
    /// the callee register's checked contract, load `{ invoke, env }`, and
    /// call `invoke` indirectly with the environment pointer prepended
    /// (after the outcome/sret out-pointer, when one exists).
    #[allow(clippy::too_many_arguments)]
    fn lower_call_indirect(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callee: Reg,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
        instantiated_contract: Option<&Ty>,
    ) -> Result<(), PlironError> {
        let contract = instantiated_contract
            .cloned()
            .or_else(|| self.func.reg_types.get(&callee.0).cloned());
        let Some(mut contract) = contract else {
            return Err(self.unsupported_reg("untyped indirect callee".into(), dest));
        };
        while let Ty::Ref(reference) = contract {
            contract = *reference.referent;
        }
        if let Ty::Struct(name, _) = &contract {
            // Monomorphization devirtualizes nominal callables into direct
            // `__call__` method calls; one that survives to lowering is a
            // shape it could not rewrite.
            return Err(self.unsupported_reg(
                format!("indirect call through nominal callable `{name}`"),
                dest,
            ));
        }
        let abi = self.contract_abi(ctx, &contract, dest)?;
        let bound =
            self.bind_contract_slots(ctx, dest, &abi, args, kwargs, arg_places, kwarg_places)?;
        let base = self.reg_ptr(ctx, callee)?;
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let invoke = LoadOp::new(ctx, base, ptr_ty);
        self.append(ctx, invoke.get_operation(), Some(dest));
        let env_address = self.gep_byte(ctx, base, 8, dest);
        let env = LoadOp::new(ctx, env_address, ptr_ty);
        self.append(ctx, env.get_operation(), Some(dest));
        let mut operands = Vec::with_capacity(bound.len() + 1);
        operands.push(env.get_result(ctx));
        operands.extend(bound);
        self.emit_call_shaped(
            ctx,
            dest,
            CallOpCallable::Indirect(invoke.get_result(ctx)),
            abi.func_ty,
            abi.returns_value,
            abi.sret,
            abi.outcome.clone(),
            operands,
        )
    }

    /// Resolve an indirect call's arguments into the contract's positional
    /// parameter order via `call::match_call_slots` — the same structural
    /// binding as `bind_call_slots`, off the `Ty::Func` contract instead of
    /// a compiled declaration. Defaults reject: the VM binds an omitted
    /// argument from the runtime callee's declaration, which the native
    /// caller cannot see behind the thunk.
    #[allow(clippy::too_many_arguments)]
    fn bind_contract_slots(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        abi: &ContractAbi,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
    ) -> Result<Vec<Value>, PlironError> {
        let kw_names: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
        let matched = match_call_slots(
            &abi.names,
            &abi.required,
            abi.positional_only,
            abi.keyword_only,
            args.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .map_err(|error| {
            self.unsupported_reg(format!("indirect-call binding failed: {error:?}"), dest)
        })?;
        if matched.slots.len() != abi.params.len() {
            return Err(self.unsupported_reg(
                "indirect-call binding disagrees with the contract arity".into(),
                dest,
            ));
        }
        let mut lowered = Vec::with_capacity(abi.params.len());
        for (index, (slot, expected)) in matched.slots.iter().zip(&abi.params).enumerate() {
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let expected = expected.clone();
            let owned = abi.owned_params.get(index).copied().unwrap_or(false);
            let by_ref = abi.ref_params.get(index).copied().unwrap_or(false);
            let place_address = |lowering: &mut Self,
                                 ctx: &mut Context,
                                 place: Option<&MirPlace>|
             -> Result<Value, PlironError> {
                let Some(place) = place.cloned() else {
                    return Err(lowering.unsupported_reg(
                        "`mut`/`ref` indirect argument without a place".into(),
                        dest,
                    ));
                };
                Ok(lowering.place_address(ctx, &place, dest)?.0)
            };
            let value = match slot {
                ArgSlot::Positional(p) if by_ref => {
                    place_address(self, ctx, arg_places.get(*p).and_then(Option::as_ref))?
                }
                ArgSlot::Keyword(k) if by_ref => {
                    place_address(self, ctx, kwarg_places.get(*k).and_then(Option::as_ref))?
                }
                ArgSlot::Positional(p) => self.arg_value(ctx, args[*p], &expected, owned, dest)?,
                ArgSlot::Keyword(k) => self.arg_value(ctx, kwargs[*k].1, &expected, owned, dest)?,
                ArgSlot::Default => {
                    return Err(self.unsupported_reg(
                        "defaulted argument at an indirect call site".into(),
                        dest,
                    ));
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
                        // A `^` transfer of a struct with a compiled
                        // `__moveinit__` runs it — the VM's `move_value` —
                        // when the type also owns its allocations through a
                        // destructor. A destructor-less pointer owner leaks
                        // under real frees (the VM's arena tolerates it) and
                        // stays rejected until the S5.7 move-residues slice.
                        if let Ty::Struct(name, _) = ty.as_ref()
                            && self
                                .signatures
                                .contains_key(&format!("{name}.__moveinit__"))
                            && (self.stdlib_deinit_temp(&ty)
                                || self
                                    .declarations
                                    .contains_key(&format!("{name}.__deinit__"))
                                || !self.type_owns_pointer(&ty))
                        {
                            let name = name.clone();
                            return self.move_via_moveinit(ctx, dest, var, &name, layout);
                        }
                        return Err(self.unsupported_reg(
                            format!("move of `{ty}` with a user `__moveinit__`"),
                            dest,
                        ));
                    }
                    let storage = self.entry_alloca(ctx, layout.size, layout.align);
                    self.mem_copy(ctx, storage, src, layout.size, dest);
                    self.reg_values.insert(dest.0, storage);
                    // The move vacates the slot (the VM tombstones it); a
                    // later cleanup-edge drop must find it empty. The moved
                    // value is an owned temporary until consumed.
                    self.set_drop_flag(ctx, var, false);
                    if self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty) {
                        self.mark_owned_temp(dest, (*ty).clone())?;
                    }
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
        self.lower_alloc_core(ctx, dest, args[0], alignment)
    }

    /// The shared allocation core behind `unsafe_alloc` and the
    /// `UnsafePointer.alloc`/`alloc_aligned` builtins: `mjrt_alloc` of
    /// `count * sizeof(element)` bytes at the element's natural alignment
    /// (or the requested one; `0` selects natural).
    fn lower_alloc_core(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        count: Reg,
        alignment: Option<Reg>,
    ) -> Result<(), PlironError> {
        let Some(Ty::Pointer { element, .. }) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(
                self.unsupported_reg("allocation without a concrete pointer result".into(), dest)
            );
        };
        let element_layout = self.layout.layout_of(&element).map_err(|error| {
            self.unsupported_reg(format!("allocation element layout ({error})"), dest)
        })?;
        let count = self.reg_value(ctx, count, ScalarTy::Int)?;
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

    /// `UnsafePointer.unsafe_dangling` / `Pointer.unsafe_dangling`: the null
    /// pointer (the VM's `allocation: 0` sentinel). Dereference and free
    /// misuse are off-gate runtime errors; the VM rejects `free` of a
    /// dangling pointer while `mjrt_free(null)` is a no-op — a recorded
    /// divergence.
    fn lower_dangling_builtin(&mut self, ctx: &mut Context, dest: Reg) -> Result<(), PlironError> {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let null = ZeroOp::new(ctx, ptr_ty);
        self.define(ctx, dest, null.get_operation(), null.get_result(ctx))
    }

    /// `len(x)` over the non-nominal shapes (the VM's `call_named` arm):
    /// string byte length, or the static element count of a pack. Nominal
    /// receivers were rewritten to `__len__` method calls during
    /// monomorphization.
    fn lower_len_builtin(
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
    fn lower_abs_builtin(
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
    fn lower_min_max_builtin(
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
    fn lower_round_builtin(
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
    fn lower_divmod_builtin(
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
    fn lower_input_builtin(
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

    /// `__floor__`/`__ceil__`/`__trunc__` on a scalar receiver — the VM's
    /// `builtin_round_dir`: integers are already whole (identity), Float64
    /// rounds toward the requested direction.
    fn lower_round_dir(
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
    fn lower_ceildiv(
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

    /// `PointerStorageTake`: move an initialized element out of
    /// `UnsafePointer` collection storage — the VM's `heap_take`
    /// (`mem::replace`): a raw byte move with no `__copyinit__` and no
    /// tombstone. Ownership verification guarantees single-take on the
    /// runnable subset; the uninitialized-misuse traps live in off-gate
    /// runtime_error fixtures.
    fn lower_pointer_storage_take(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_value(ctx, pointer, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, element, dest)?;
        self.load_from(ctx, address, element, dest)?;
        // The destination owns the moved value now: free its heap buffers if
        // it dies as a discarded temporary (the VM's Rust runtime frees
        // register temporaries invisibly).
        self.mark_owned_temp(dest, element.clone())
    }

    /// `PointerStorageDestroy`: run the element destructor in place at the
    /// element address — the VM's `heap_destroy` (`heap_take` +
    /// `drop_value`). `emit_drop_value` supplies the compiled-`__deinit__`
    /// dispatch, rejection of raising/droppable-field destructors, and the
    /// lifecycle-trace event.
    fn lower_pointer_storage_destroy(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_value(ctx, pointer, ScalarTy::Ptr)?;
        let address = self.pointer_element_address(ctx, ptr, index, element, dest)?;
        self.emit_drop_value(ctx, address, element, false)?;
        self.erased.insert(dest.0);
        Ok(())
    }

    /// `UninitStorage`: payload-only frame storage for `__UninitStorage[T]`
    /// (no init flag — see the layout arm). An `init` payload moves in raw
    /// (the VM's `mem::replace`, no `__moveinit__`).
    fn lower_uninit_storage(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        init: Option<Reg>,
    ) -> Result<(), PlironError> {
        let Some(dest_ty) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped uninit storage result".into(), dest));
        };
        let Some(element) = crate::types::uninit_storage_element(&dest_ty).cloned() else {
            return Err(self.unsupported_reg(
                format!("uninit storage of non-storage type `{dest_ty}`"),
                dest,
            ));
        };
        let layout = self
            .layout
            .layout_of(&dest_ty)
            .map_err(|error| self.unsupported_reg(format!("uninit storage ({error})"), dest))?;
        if layout.size == 0 {
            self.erased.insert(dest.0);
            return Ok(());
        }
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        if let Some(src) = init {
            self.store_to(ctx, storage, &element, src)?;
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// `UninitStorageTake`: move the payload out of inline uninit storage —
    /// a raw byte move (the VM's `mem::replace` of the payload box).
    fn lower_uninit_storage_take(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        storage: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_ptr(ctx, storage)?;
        self.load_from(ctx, ptr, element, dest)?;
        self.mark_owned_temp(dest, element.clone())
    }

    /// `UninitStorageDestroy`: run the payload destructor in place — the
    /// VM's take-then-`drop_value`.
    fn lower_uninit_storage_destroy(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        storage: Reg,
        element: &Ty,
    ) -> Result<(), PlironError> {
        let ptr = self.reg_ptr(ctx, storage)?;
        self.emit_drop_value(ctx, ptr, element, false)?;
        self.erased.insert(dest.0);
        Ok(())
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
            // A compiled copy chain byte-copies elements that own raw
            // pointer storage (the VM's arena-safe shallow clone); releasing
            // such a copy through its destructor would free the shared
            // buffers, and leaving it leaks — reject the copy instead.
            if let Ty::Struct(_, args) = ty
                && args.iter().any(|arg| match arg {
                    crate::types::TyArg::Ty(element) => {
                        self.type_owns_pointer(element)
                            && matches!(element, Ty::Struct(element_name, _)
                                if self.declarations.contains_key(
                                    &format!("{element_name}.__deinit__")))
                    }
                    _ => false,
                })
            {
                return Err(self.unsupported_reg(
                    format!("copy of `{ty}` with pointer-owning elements"),
                    dest,
                ));
            }
            // A copy constructor may have allocated; release what the
            // invisible rule understands (String buffers) or, for a stdlib
            // collection copy, its own compiled destructor chain.
            if self.releasable(ty) || self.stdlib_deinit_temp(ty) {
                self.mark_owned_temp(dest, ty.clone())?;
            }
        } else if self.has_nested_lifecycle(ty, "__copyinit__") {
            return Err(self.unsupported_reg(
                format!("copy of `{ty}` with a nested user `__copyinit__`"),
                dest,
            ));
        } else if self.owns_heap(ty) {
            // Drop elaboration may destroy the owning variable immediately
            // after its last use — before this temporary is read — so
            // aliasing its buffers is not an option under real frees: fork
            // the copy and release it after its own last use (the VM's
            // arena-shared plain clone, made explicit).
            self.fork_value_into(ctx, storage, ty, layout, src_ptr, dest)?;
            self.reg_values.insert(dest.0, storage);
            self.mark_owned_temp(dest, ty.clone())?;
            return Ok(());
        } else {
            // A byte copy of a heap-less value carries everything it needs.
            self.mem_copy(ctx, storage, src_ptr, layout.size, dest);
        }
        self.reg_values.insert(dest.0, storage);
        Ok(())
    }

    /// Fork the value at `src_ptr` into `dst`: a byte copy whose
    /// String/Error components are re-duplicated so each copy owns its own
    /// buffers — the native analog of the VM's arena-shared plain clone
    /// (whose aliasing is invisible because the arena never reclaims). User
    /// copy constructors never run here; the VM's plain clone does not run
    /// them either. Values owning raw pointer storage cannot fork bufferwise
    /// and reject contextually.
    fn fork_value_into(
        &mut self,
        ctx: &mut Context,
        dst: Value,
        ty: &Ty,
        layout: Layout,
        src_ptr: Value,
        span: Reg,
    ) -> Result<(), PlironError> {
        if let Ty::Struct(name, _) = ty
            && crate::symbol::is_stdlib_string_struct(name)
        {
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, span);
            let src_cap = self.string_cap(ctx, src_ptr, span);
            let new_data = self.emit_alloc(ctx, src_cap, 1, span);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, span);
            self.store_string_fields(ctx, dst, new_data, src_size, src_cap, span);
            return Ok(());
        }
        if matches!(ty, Ty::Error) {
            let (src_data, src_size) = self.string_parts(ctx, src_ptr, span);
            let new_data = self.emit_alloc(ctx, src_size, 1, span);
            self.mem_copy_dynamic(ctx, new_data, src_data, src_size, span);
            self.store_string_fields(ctx, dst, new_data, src_size, src_size, span);
            return Ok(());
        }
        let elements: Vec<(Ty, u64)> = match ty {
            Ty::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()) else {
                    return Err(self.unsupported_reg(format!("fork of undeclared `{ty}`"), span));
                };
                let fields: Vec<Ty> = decl.fields.iter().map(|(_, ty)| ty.clone()).collect();
                let composed = self.struct_layout_of(&fields, span)?;
                fields.into_iter().zip(composed.offsets).collect()
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => {
                let elements = elements.clone();
                let composed = self.struct_layout_of(&elements, span)?;
                elements.into_iter().zip(composed.offsets).collect()
            }
            other => {
                return Err(self.unsupported_reg(format!("fork of `{other}`"), span));
            }
        };
        self.mem_copy(ctx, dst, src_ptr, layout.size, span);
        for (element, offset) in elements {
            if !self.owns_heap(&element) {
                continue;
            }
            let element_layout = self.layout.layout_of(&element).map_err(|error| {
                self.unsupported_reg(format!("fork element layout ({error})"), span)
            })?;
            let src_field = self.gep_byte(ctx, src_ptr, offset, span);
            let dst_field = self.gep_byte(ctx, dst, offset, span);
            self.fork_value_into(ctx, dst_field, &element, element_layout, src_field, span)?;
        }
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

    /// Whether `name`'s declaration takes a variadic pack. Such callees
    /// always bind through the slot matcher — an argument count equal to the
    /// physical parameter count (arity one against the pack slot) must still
    /// build pack storage.
    fn variadic_callee(&self, name: &str) -> bool {
        self.declarations
            .get(name)
            .is_some_and(|decl| decl.variadic.is_some())
    }

    /// Record `dest` as an owned heap-carrying temporary, released after its
    /// final use in this block. A temporary whose final use sits in another
    /// block would need liveness analysis — reject instead of leaking.
    fn mark_owned_temp(&mut self, dest: Reg, ty: Ty) -> Result<(), PlironError> {
        if !self.owns_heap(&ty) && !matches!(ty, Ty::StringLiteral) && !self.stdlib_deinit_temp(&ty)
        {
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
        if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
            eprintln!("TEMP-DBG {} mark %r{} {ty}", self.name, dest.0);
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
            if std::env::var_os("MOJITO_PLIRON_DBG_TEMPS").is_some() {
                eprintln!("TEMP-DBG {} release %r{} {ty}", self.name, reg);
            }
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
        // A discarded stdlib collection temporary (a printed slice result,
        // a read-receiver copy) releases through its compiled destructor —
        // a pure free chain the VM's never-reclaiming arena leaves to the
        // collector. The invisible rule stays first where it covers the
        // type (String shapes — untraced, like the VM, which never drops
        // register temporaries), and the destructor dispatch here is
        // untraced for the same reason; user destructors stay under the
        // invisible rule.
        if self.stdlib_deinit_temp(ty) && !self.owns_heap(ty) {
            let traced = self.trace_lifecycle;
            self.trace_lifecycle = false;
            let released = self.emit_drop_value(ctx, storage, ty, false);
            self.trace_lifecycle = traced;
            return released;
        }
        self.emit_release_storage(ctx, storage, ty)
    }

    /// Whether `ty` transitively stores a raw pointer field — an allocation
    /// only a destructor (or explicit free) can release.
    fn type_owns_pointer(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Pointer { .. } => true,
            // The nominal String is opaque here: its buffer pointer is
            // handled by the native deep-copy/release bridges, so
            // String-carrying shapes copy and move soundly.
            Ty::Struct(name, _) if crate::symbol::is_stdlib_string_struct(name) => false,
            Ty::Struct(name, _) => self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                decl.fields
                    .iter()
                    .any(|(_, field)| self.type_owns_pointer(field))
            }),
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.type_owns_pointer(element)),
            _ => false,
        }
    }

    /// Whether `ty` is a stdlib-owned aggregate whose compiled destructor may
    /// release a discarded temporary: the destructor chain is
    /// stdlib-authored (pure frees, nothing user-observable).
    fn stdlib_deinit_temp(&self, ty: &Ty) -> bool {
        let Ty::Struct(name, _) = ty else {
            return false;
        };
        let template = name.split("$mono").next().unwrap_or(name);
        let stdlib = template.starts_with("__module$std$")
            || matches!(
                template,
                "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
            );
        stdlib
            && (self.signatures.contains_key(&format!("{name}.__deinit__")) || self.needs_drop(ty))
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
                    // A literal index into pack storage projects statically,
                    // like `Proj::ConstIndex` (the Tuple accessor bodies'
                    // `self.storage[0]` shape).
                    if let Ty::Tuple(elements) | Ty::RuntimePack(elements) = &ty {
                        let elements = elements.clone();
                        let Some(PendingLiteral::Int(literal)) =
                            self.pending_literals.get(&index.0).cloned()
                        else {
                            return Err(self.unsupported_reg(
                                "runtime subscript projection into pack storage".into(),
                                dest,
                            ));
                        };
                        let element = literal
                            .to_i64()
                            .and_then(|value| usize::try_from(value).ok())
                            .filter(|value| *value < elements.len())
                            .ok_or_else(|| {
                                self.unsupported_reg(
                                    "pack subscript projection index out of range".into(),
                                    dest,
                                )
                            })?;
                        let composed = self.struct_layout_of(&elements, dest)?;
                        offset += composed.offsets[element];
                        ty = elements[element].clone();
                        continue;
                    }
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
                Proj::UninitPayload => {
                    let Some(element) = crate::types::uninit_storage_element(&ty).cloned() else {
                        return Err(self.unsupported_reg(
                            format!("uninit-payload projection on `{ty}`"),
                            dest,
                        ));
                    };
                    // Payload-only storage: the payload sits at the storage's
                    // own address, so the projection changes only the
                    // designated type. Stores through it overwrite raw — the
                    // old payload leaks by design, exactly the VM's
                    // `unsafe_write`.
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
        // A slice-descriptor bound access materializes a fresh `Optional`
        // through its compiled constructor — the VM's `slice_bound_optional`.
        if slice_struct_name(&base_ty).is_some() && matches!(field, "start" | "end" | "step") {
            return self.lower_slice_bound_field(ctx, dest, base, field);
        }
        let (offset, field_ty) = self.field_offset(&base_ty, field, dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let address = if offset == 0 {
            base_ptr
        } else {
            self.gep_byte(ctx, base_ptr, offset, dest)
        };
        self.load_from(ctx, address, &field_ty, dest)
    }

    /// Move a variable through its compiled `__moveinit__` — the VM's
    /// `move_value` over a `^` transfer: fresh `out self` storage, the source
    /// storage as the consumed `move` argument, and a vacated source slot.
    fn move_via_moveinit(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        var: u32,
        name: &str,
        layout: Layout,
    ) -> Result<(), PlironError> {
        let moveinit = format!("{name}.__moveinit__");
        let signature = &self.signatures[&moveinit];
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{moveinit}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let storage = self.entry_alloca(ctx, layout.size, layout.align);
        let src = self.var_slots[var as usize];
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![storage, src],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, storage);
        // The move vacates the slot (the VM tombstones it); the moved value
        // is an owned temporary until consumed.
        self.set_drop_flag(ctx, var, false);
        if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
            && (self.owns_heap(&ty) || self.stdlib_deinit_temp(&ty))
        {
            self.mark_owned_temp(dest, ty)?;
        }
        Ok(())
    }

    /// One bound of the raw 32-byte slice descriptor (`{start, end, step,
    /// flags}` i64 fields — the layout `discover_structs` synthesizes),
    /// materialized as a real `Optional` by calling the destination type's
    /// compiled positional constructor: 1-argument when the bound's flag bit
    /// is set, 0-argument otherwise — the VM's `slice_bound_optional`.
    /// One bound of the raw 32-byte slice descriptor (`{start, end, step,
    /// flags}` i64 fields — the layout `discover_structs` synthesizes),
    /// materialized as an `Optional` value over a frame-backed payload slot:
    /// `{data → payload, _size ∈ {0, 1}}` — the observable state the VM's
    /// `slice_bound_optional` constructor calls produce, with no heap
    /// allocation to own.
    fn lower_slice_bound_field(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        field: &str,
    ) -> Result<(), PlironError> {
        let Some(optional_ty @ Ty::Struct(..)) = self.func.reg_types.get(&dest.0).cloned() else {
            return Err(self.unsupported_reg("untyped slice bound access".into(), dest));
        };
        let lowered = lower_ty(self.name, &optional_ty, &self.layout, self.reg_span(dest))?;
        let LowerTy::Aggregate { layout, .. } = lowered else {
            return Err(self.unsupported_reg("slice bound Optional layout".into(), dest));
        };
        let (offset, bit) = match field {
            "start" => (0u64, 1i64),
            "end" => (8, 2),
            _ => (16, 4),
        };
        let descriptor = self.reg_ptr(ctx, base)?;
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let value_address = self.offset_address(ctx, descriptor, offset);
        let value = LoadOp::new(ctx, value_address, i64_handle);
        self.append(ctx, value.get_operation(), Some(dest));
        let flags_address = self.offset_address(ctx, descriptor, 24);
        let flags = LoadOp::new(ctx, flags_address, i64_handle);
        self.append(ctx, flags.get_operation(), Some(dest));
        let mask = self.int_constant(ctx, bit);
        let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
        self.append(ctx, masked.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let is_set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
        self.append(ctx, is_set.get_operation(), Some(dest));
        let payload = self.entry_alloca(ctx, 8, 8);
        let store = StoreOp::new(ctx, value.get_result(ctx), payload);
        self.append(ctx, store.get_operation(), Some(dest));
        let temp = self.entry_alloca(ctx, layout.size, layout.align);
        let store = StoreOp::new(ctx, payload, temp);
        self.append(ctx, store.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let size = SelectOp::new(ctx, is_set.get_result(ctx), one, zero);
        self.append(ctx, size.get_operation(), Some(dest));
        let size_address = self.offset_address(ctx, temp, 8);
        let store = StoreOp::new(ctx, size.get_result(ctx), size_address);
        self.append(ctx, store.get_operation(), Some(dest));
        self.reg_values.insert(dest.0, temp);
        Ok(())
    }

    /// Materialize one slice descriptor in the backend's raw layout: three
    /// i64 bounds at offsets 0/8/16 (absent bounds store 0) and the presence
    /// bitmask at offset 24 (start=1, end=2, step=4) — `Value::Slice`'s
    /// `Option<i64>` fields.
    fn build_slice_descriptor(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
    ) -> Result<Value, PlironError> {
        let storage = self.entry_alloca(ctx, 32, 8);
        let mut flags = 0i64;
        for (index, (bound, bit)) in [(lower, 1i64), (upper, 2), (step, 4)].iter().enumerate() {
            let value = match bound {
                Some(reg) => self.reg_value(ctx, *reg, ScalarTy::Int)?,
                None => self.int_constant(ctx, 0),
            };
            let address = self.offset_address(ctx, storage, index as u64 * 8);
            let store = StoreOp::new(ctx, value, address);
            self.append(ctx, store.get_operation(), Some(anchor));
            if bound.is_some() {
                flags |= bit;
            }
        }
        let flags = self.int_constant(ctx, flags);
        let address = self.offset_address(ctx, storage, 24);
        let store = StoreOp::new(ctx, flags, address);
        self.append(ctx, store.get_operation(), Some(anchor));
        Ok(storage)
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
        kwarg_places: &[Option<MirPlace>],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        // Pointer receivers dispatch to runtime intrinsics, never to compiled
        // stdlib bodies.
        if matches!(self.func.reg_types.get(&recv.0), Some(Ty::Pointer { .. })) {
            return self.lower_pointer_method(ctx, dest, recv, method, args);
        }
        // Slice descriptors are checker-virtual: `indices` is the VM's
        // intrinsic normalization, and no other method exists on them.
        if self
            .func
            .reg_types
            .get(&recv.0)
            .and_then(slice_struct_name)
            .is_some()
        {
            if method == "indices" {
                return self.lower_slice_indices(ctx, dest, recv, args);
            }
            return Err(self.unsupported_reg(format!("slice descriptor method `{method}`"), dest));
        }
        // The builtin-string writer receiver (`write_to`'s `Value::Str`
        // accumulator) appends each argument's display text in place.
        if resolved.is_none()
            && method == "write"
            && matches!(self.func.reg_types.get(&recv.0), Some(Ty::StringLiteral))
        {
            return self.lower_str_writer_write(ctx, dest, recv, args, recv_place);
        }
        // The struct-to-literal bridge (the VM's `string_struct_literal`):
        // the declared stub body must never execute, and the bridged bytes
        // would need an owner the literal value model cannot record — the
        // VM's arena never reclaims, while a native copy stored into a
        // drop-inert literal-typed field leaks with no releasing owner.
        // Reject until a literal-ownership design lands.
        if method == "_as_string_literal"
            && matches!(self.func.reg_types.get(&recv.0), Some(Ty::Struct(name, _))
                if crate::symbol::is_stdlib_string_struct(name))
        {
            return Err(self.unsupported_reg("String struct-to-literal bridge".into(), dest));
        }
        // The VM-synthesized `Writer.write` dispatch: format each argument
        // and feed it through the receiver's compiled `write_string`.
        if resolved.is_none()
            && method == "write"
            && let Some(Ty::Struct(writer, _)) = self.func.reg_types.get(&recv.0).cloned()
            && self
                .signatures
                .contains_key(&format!("{writer}.write_string"))
        {
            return self.lower_writer_write(ctx, dest, &writer, args, recv_place);
        }
        // Unresolved scalar-receiver dunders are the VM's non-struct
        // intrinsic dispatch (`builtin_round_dir`/`builtin_ceildiv`); a
        // struct receiver with its own method arrives resolved instead.
        if resolved.is_none()
            && let Some(recv_ty) = self.func.reg_types.get(&recv.0).cloned()
            && matches!(recv_ty, Ty::Int | Ty::UInt | Ty::Float64)
        {
            match (method, args.len()) {
                ("__floor__" | "__ceil__" | "__trunc__", 0) => {
                    return self.lower_round_dir(ctx, dest, recv, &recv_ty, method);
                }
                ("__ceildiv__", 1) => {
                    return self.lower_ceildiv(ctx, dest, recv, args[0], &recv_ty);
                }
                _ => {}
            }
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
            self.aliased_receiver_address(ctx, &place, dest)?
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
        if kwargs.is_empty() && args.len() == rest.len() && !self.variadic_callee(resolved) {
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
            lowered.extend(self.bind_call_slots(
                ctx,
                dest,
                resolved,
                rest,
                rest_owned,
                rest_by_reference,
                args,
                kwargs,
                arg_places,
                kwarg_places,
            )?);
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

    /// `_mojito_abort(message)` — the `std.os.abort` crossing: report the
    /// message through `mjrt_unhandled_error` and never return (the VM's
    /// uncatchable `RuntimeError::Abort`; the native exit code is the
    /// unhandled-error trap's — a recorded divergence from the VM's distinct
    /// abort reporting).
    fn lower_abort_builtin(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        message: Reg,
    ) -> Result<(), PlironError> {
        let (data, len) = self.writer_argument_text(ctx, message, dest)?;
        let unhandled_ty = self.shared.ensure_rt(ctx, "mjrt_unhandled_error");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_unhandled_error".try_into().expect("valid identifier")),
            unhandled_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        let unreachable = UnreachableOp::new(ctx);
        self.append(ctx, unreachable.get_operation(), None);
        // Dead continuation for the rest of the MIR block; the unreachable
        // pruning pass removes it.
        let region = self.region.expect("lowering is inside a function region");
        let dead = BasicBlock::new(ctx, None, vec![]);
        dead.insert_at_back(region, ctx);
        self.current = Some(dead);
        self.erased.insert(dest.0);
        Ok(())
    }

    /// The VM-synthesized `Writer.write` dispatch: each argument's display
    /// text feeds one `write_string` call on the aliased `mut self` receiver.
    /// The payload `String` borrows the text bytes (`cap == len`); the callee
    /// reads it and never takes ownership.
    fn lower_writer_write(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        writer: &str,
        args: &[Reg],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let write_string = format!("{writer}.write_string");
        let signature = &self.signatures[&write_string];
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{write_string}`"), dest));
        }
        let nominal_payload = matches!(
            self.declarations
                .get(&write_string)
                .and_then(|decl| decl.param_types.first()),
            Some(Ty::Struct(payload, args)) if args.is_empty()
                && crate::symbol::is_stdlib_string_struct(payload)
        );
        if !nominal_payload {
            return Err(self.unsupported_reg(
                format!("`{write_string}` without a nominal String payload"),
                dest,
            ));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let Some(place) = recv_place else {
            return Err(self.unsupported_reg("`Writer.write` needs a mutable place".into(), dest));
        };
        let place = place.clone();
        let writer_address = self.place_address(ctx, &place, dest)?.0;
        for arg in args {
            let (data, len) = self.writer_argument_text(ctx, *arg, dest)?;
            let payload = self.entry_alloca(ctx, 24, 8);
            self.store_string_fields(ctx, payload, data, len, len, dest);
            let call = CallOp::new(
                ctx,
                CallOpCallable::Direct(callee.clone()),
                func_ty,
                vec![writer_address, payload],
            );
            self.append(ctx, call.get_operation(), Some(dest));
        }
        self.erased.insert(dest.0);
        Ok(())
    }

    /// The display bytes of one `Writer.write` argument as a `(data, len)`
    /// pair — the VM's `format_value` over the supported argument shapes.
    fn writer_argument_text(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<(Value, Value), PlironError> {
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            let global = self.shared.intern_string(ctx, &bytes);
            let data = self.global_address(ctx, &global, dest);
            let len = self.uint_constant(ctx, bytes.len() as u64);
            return Ok((data, len));
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            return Ok((descriptor.data, descriptor.len));
        }
        match self.func.reg_types.get(&arg.0) {
            Some(Ty::Error | Ty::StringLiteral) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            Some(Ty::Struct(name, _)) if crate::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, arg)?;
                return Ok(self.string_parts(ctx, ptr, dest));
            }
            _ => {}
        }
        let Some(ty) = self.concrete_scalar_ty(arg)? else {
            return Err(self.unsupported_reg("formatted write argument".into(), dest));
        };
        let value = self.reg_value(ctx, arg, ty)?;
        self.format_scalar(ctx, ty, value, dest)
    }

    /// The storage a `mut`/`ref` receiver aliases: the place's address,
    /// dereferenced once when the place designates a reference handle (a ref
    /// field like an iterator's `src`) — the VM reads through `Value::Ref`
    /// receivers before dispatch.
    fn aliased_receiver_address(
        &mut self,
        ctx: &mut Context,
        place: &MirPlace,
        anchor: Reg,
    ) -> Result<Value, PlironError> {
        let (address, ty) = self.place_address(ctx, place, anchor)?;
        if matches!(ty, Ty::Ref(_)) {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(anchor));
            Ok(load.get_result(ctx))
        } else {
            Ok(address)
        }
    }

    /// One checker-selected subscript invocation (`Index`/`Slice`/
    /// `MultiIndex`/`MultiSet` nominal dispatch): bind the receiver by its
    /// compiled convention, match the actuals (index registers and
    /// inline-built slice descriptors) against the callee's slots, call, and
    /// write a `mut self` receiver back — the VM's `method_call` over the
    /// subscript contract. `anchor` is the result register for the get forms
    /// and a scratch register for `MultiSet` (whose result is discarded).
    #[allow(clippy::too_many_arguments)]
    fn lower_subscript_call(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        method: &str,
        call: &crate::mir::MirSubscriptCall,
        recv: Reg,
        recv_place: Option<&MirPlace>,
        positional: &[SubscriptActual],
        keywords: &[(&str, SubscriptActual)],
    ) -> Result<(), PlironError> {
        let resolved = call.target.clone();
        let Some(signature) = self.signatures.get(&resolved) else {
            return Err(
                self.unsupported_reg(format!("subscript call to uncompiled `{resolved}`"), anchor)
            );
        };
        let params = signature.params.clone();
        let owned = signature.owned_params.clone();
        let by_reference = signature.ref_params.clone();
        if params.is_empty() {
            return Err(self.unsupported_reg(
                format!("subscript target `{resolved}` without a receiver"),
                anchor,
            ));
        }
        let Some(decl) = self.declarations.get(&resolved) else {
            return Err(self.unsupported_reg(
                format!("subscript call to `{resolved}` without a recorded declaration"),
                anchor,
            ));
        };
        if decl.variadic.is_some() || decl.kw_variadic.is_some() {
            return Err(
                self.unsupported_reg(format!("variadic subscript call to `{resolved}`"), anchor)
            );
        }
        let kw_names: Vec<&str> = keywords.iter().map(|(name, _)| *name).collect();
        let matched = match_call_slots(
            &decl.param_names,
            &decl.required,
            decl.positional_only,
            decl.keyword_only,
            positional.len(),
            &kw_names,
            CallVariadics {
                positional: false,
                keyword: false,
            },
        )
        .map_err(|error| {
            self.unsupported_reg(
                format!("subscript binding for `{resolved}` failed: {error:?}"),
                anchor,
            )
        })?;
        let defaults = decl.defaults.clone();
        let receiver_convention = decl.receiver_convention;
        let receiver_alias = recv_place.is_some()
            && matches!(
                receiver_convention,
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
        let recv_owned = owned.first().copied().unwrap_or(false);
        let recv_value = if receiver_alias {
            let place = recv_place.expect("aliased receivers have a place").clone();
            self.aliased_receiver_address(ctx, &place, anchor)?
        } else {
            self.arg_value(ctx, recv, &params[0], recv_owned, anchor)?
        };
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let rest_by_reference = if by_reference.len() > 1 {
            &by_reference[1..]
        } else {
            &[]
        };
        if matched.slots.len() != rest.len() {
            return Err(self.unsupported_reg(
                format!("subscript binding for `{resolved}` disagrees with its compiled arity"),
                anchor,
            ));
        }
        let mut operands = vec![recv_value];
        for (i, slot) in matched.slots.iter().enumerate() {
            let expected = rest[i].clone();
            if matches!(expected, LowerTy::ZeroSized) {
                continue;
            }
            let owned = rest_owned.get(i).copied().unwrap_or(false);
            let by_ref = rest_by_reference.get(i).copied().unwrap_or(false);
            let actual = match slot {
                ArgSlot::Positional(p) => Some(&positional[*p]),
                ArgSlot::Keyword(k) => Some(&keywords[*k].1),
                ArgSlot::Default => None,
            };
            let value = match actual {
                Some(SubscriptActual::Reg(reg, place)) => {
                    if by_ref {
                        let Some(place) = place else {
                            return Err(self.unsupported_reg(
                                format!(
                                    "`mut`/`ref` subscript argument of `{resolved}` without a place"
                                ),
                                anchor,
                            ));
                        };
                        let place = (*place).clone();
                        self.place_address(ctx, &place, anchor)?.0
                    } else {
                        self.arg_value(ctx, *reg, &expected, owned, anchor)?
                    }
                }
                Some(SubscriptActual::Descriptor(value)) => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("`mut`/`ref` slice-descriptor argument of `{resolved}`"),
                            anchor,
                        ));
                    }
                    *value
                }
                None => {
                    if by_ref {
                        return Err(self.unsupported_reg(
                            format!("defaulted `mut`/`ref` parameter of `{resolved}`"),
                            anchor,
                        ));
                    }
                    let Some(default) = defaults.get(i).and_then(Option::as_ref) else {
                        return Err(self.unsupported_reg(
                            format!("non-constant default argument in call to `{resolved}`"),
                            anchor,
                        ));
                    };
                    let LowerTy::Scalar(scalar) = expected else {
                        return Err(self.unsupported_reg(
                            format!("non-scalar default argument in call to `{resolved}`"),
                            anchor,
                        ));
                    };
                    let default = default.clone();
                    self.checked_const_value(ctx, &default, scalar, anchor)?
                }
            };
            operands.push(value);
        }
        self.emit_bound_call(ctx, anchor, &resolved, operands)?;
        // A reference result is the callee's returned place pointer — the
        // caller-side handle convention; a handle to pointer-typed storage
        // joins `pointer_slot_refs` like `MakeRef`.
        if let Some(reference) = &call.reference_result
            && matches!(*reference.referent, Ty::Pointer { .. })
        {
            self.pointer_slot_refs.insert(anchor.0);
        }
        // `mut self` receivers without an aliased place write the modified
        // receiver back — the `lower_method_call` contract.
        let write_back = !receiver_alias
            && match self.func.reg_types.get(&recv.0) {
                Some(Ty::Struct(struct_name, _)) => self
                    .struct_decls
                    .get(struct_name.as_str())
                    .is_some_and(|d| {
                        d.mut_self_methods.contains(resolved.as_str())
                            || d.mut_self_methods.contains(method)
                    }),
                _ => false,
            };
        if write_back && let Some(place) = recv_place {
            let LowerTy::Aggregate { layout, .. } = &params[0] else {
                return Err(
                    self.unsupported_reg("mutating subscript on a scalar receiver".into(), anchor)
                );
            };
            let size = layout.size;
            let recv_ptr = self.reg_ptr(ctx, recv)?;
            let place = place.clone();
            let (address, _) = self.place_address(ctx, &place, anchor)?;
            self.mem_copy(ctx, address, recv_ptr, size, anchor);
        }
        Ok(())
    }

    /// One `MirSubscriptArg` as a lowered actual: an index register or an
    /// inline-built slice descriptor.
    fn subscript_actual<'p>(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        arg: &crate::mir::MirSubscriptArg,
        place: Option<&'p MirPlace>,
    ) -> Result<SubscriptActual<'p>, PlironError> {
        Ok(match arg {
            crate::mir::MirSubscriptArg::Index(reg) => SubscriptActual::Reg(*reg, place),
            crate::mir::MirSubscriptArg::Slice {
                lower, upper, step, ..
            } => SubscriptActual::Descriptor(
                self.build_slice_descriptor(ctx, anchor, *lower, *upper, *step)?,
            ),
        })
    }

    fn subscript_actuals<'p>(
        &mut self,
        ctx: &mut Context,
        anchor: Reg,
        args: &[crate::mir::MirSubscriptArg],
        places: &'p [Option<MirPlace>],
    ) -> Result<Vec<SubscriptActual<'p>>, PlironError> {
        args.iter()
            .enumerate()
            .map(|(i, arg)| {
                self.subscript_actual(ctx, anchor, arg, places.get(i).and_then(Option::as_ref))
            })
            .collect()
    }

    /// An intrinsic storage subscript: a constant index into heterogeneous
    /// (`TupleStorage`) or homogeneous (`VariadicStorage`) pack storage — the
    /// VM's `index_value` clone at a statically composed offset. Runtime
    /// indexes stay rejected until the packs slice.
    fn lower_index_intrinsic(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        base: Reg,
        index: Reg,
        intrinsic: &crate::mir::MirIntrinsicSubscript,
    ) -> Result<(), PlironError> {
        use crate::mir::MirIntrinsicSubscript as Sub;
        if !matches!(intrinsic, Sub::TupleStorage | Sub::VariadicStorage) {
            return Err(self.unsupported_reg("intrinsic subscript".into(), dest));
        }
        let elements = match self.func.reg_types.get(&base.0) {
            Some(Ty::Tuple(elements) | Ty::RuntimePack(elements)) => elements.clone(),
            other => {
                return Err(self.unsupported_reg(
                    format!(
                        "intrinsic subscript on `{}`",
                        other.map(|ty| ty.to_string()).unwrap_or_default()
                    ),
                    dest,
                ));
            }
        };
        let Some(PendingLiteral::Int(literal)) = self.pending_literals.get(&index.0).cloned()
        else {
            return Err(self.unsupported_reg("runtime index into pack storage".into(), dest));
        };
        let element = literal
            .to_i64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value < elements.len())
            .ok_or_else(|| {
                self.unsupported_reg("pack subscript index out of range".into(), dest)
            })?;
        let composed = self.struct_layout_of(&elements, dest)?;
        let base_ptr = self.reg_ptr(ctx, base)?;
        let offset = composed.offsets[element];
        let address = if offset == 0 {
            base_ptr
        } else {
            self.gep_byte(ctx, base_ptr, offset, dest)
        };
        self.load_from(ctx, address, &elements[element], dest)
    }

    /// The slice-descriptor `indices(length)` normalization — the VM's
    /// `normalize_slice_bounds` — as branch-free selects over the raw
    /// descriptor, producing the three-element bounds tuple. A zero step
    /// traps (the VM's runtime error; the negative-length check is
    /// unreachable over container sizes and is not replicated).
    fn lower_slice_indices(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        args: &[Reg],
    ) -> Result<(), PlironError> {
        if args.len() != 1 {
            return Err(self.unsupported_reg("slice `indices` call contract".into(), dest));
        }
        let length = self.reg_value(ctx, args[0], ScalarTy::Int)?;
        let descriptor = self.reg_ptr(ctx, recv)?;
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let bound = |lowering: &mut Self, ctx: &mut Context, offset: u64, bit: i64| {
            let address = lowering.offset_address(ctx, descriptor, offset);
            let value = LoadOp::new(ctx, address, i64_handle);
            lowering.append(ctx, value.get_operation(), Some(dest));
            let flags_address = lowering.offset_address(ctx, descriptor, 24);
            let flags = LoadOp::new(ctx, flags_address, i64_handle);
            lowering.append(ctx, flags.get_operation(), Some(dest));
            let mask = lowering.int_constant(ctx, bit);
            let masked = AndOp::new(ctx, flags.get_result(ctx), mask);
            lowering.append(ctx, masked.get_operation(), Some(dest));
            let zero = lowering.int_constant(ctx, 0);
            let is_set = ICmpOp::new(ctx, ICmpPredicateAttr::NE, masked.get_result(ctx), zero);
            lowering.append(ctx, is_set.get_operation(), Some(dest));
            (value.get_result(ctx), is_set.get_result(ctx))
        };
        let (raw_lower, has_lower) = bound(self, ctx, 0, 1);
        let (raw_upper, has_upper) = bound(self, ctx, 8, 2);
        let (raw_step, has_step) = bound(self, ctx, 16, 4);
        let one = self.int_constant(ctx, 1);
        let step = SelectOp::new(ctx, has_step, raw_step, one);
        self.append(ctx, step.get_operation(), Some(dest));
        let step = step.get_result(ctx);
        let zero = self.int_constant(ctx, 0);
        let step_is_zero = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, step, zero);
        self.append(ctx, step_is_zero.get_operation(), Some(dest));
        self.emit_trap_guard(
            ctx,
            step_is_zero.get_result(ctx),
            TrapCategory::UnhandledError,
            dest,
        )?;
        let step_positive = ICmpOp::new(ctx, ICmpPredicateAttr::SGT, step, zero);
        self.append(ctx, step_positive.get_operation(), Some(dest));
        let step_positive = step_positive.get_result(ctx);
        let minus_one = self.int_constant(ctx, -1);
        let len_minus_one = SubOp::new_with_overflow_flag(ctx, length, one, no_overflow_flags());
        self.append(ctx, len_minus_one.get_operation(), Some(dest));
        let len_minus_one = len_minus_one.get_result(ctx);
        // Clamp an explicit bound to a valid range, wrapping a negative
        // index once (`runtime::normalize_slice_bounds`'s `adjust`).
        let adjust = |lowering: &mut Self, ctx: &mut Context, raw: Value| {
            let wrapped = AddOp::new_with_overflow_flag(ctx, raw, length, no_overflow_flags());
            lowering.append(ctx, wrapped.get_operation(), Some(dest));
            let negative = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, raw, zero);
            lowering.append(ctx, negative.get_operation(), Some(dest));
            let adjusted =
                SelectOp::new(ctx, negative.get_result(ctx), wrapped.get_result(ctx), raw);
            lowering.append(ctx, adjusted.get_operation(), Some(dest));
            let adjusted = adjusted.get_result(ctx);
            let clamp =
                |lowering: &mut Self, ctx: &mut Context, value: Value, low: Value, high: Value| {
                    let below = ICmpOp::new(ctx, ICmpPredicateAttr::SLT, value, low);
                    lowering.append(ctx, below.get_operation(), Some(dest));
                    let floored = SelectOp::new(ctx, below.get_result(ctx), low, value);
                    lowering.append(ctx, floored.get_operation(), Some(dest));
                    let above =
                        ICmpOp::new(ctx, ICmpPredicateAttr::SGT, floored.get_result(ctx), high);
                    lowering.append(ctx, above.get_operation(), Some(dest));
                    let clamped =
                        SelectOp::new(ctx, above.get_result(ctx), high, floored.get_result(ctx));
                    lowering.append(ctx, clamped.get_operation(), Some(dest));
                    clamped.get_result(ctx)
                };
            let positive = clamp(lowering, ctx, adjusted, zero, length);
            let negative = clamp(lowering, ctx, adjusted, minus_one, len_minus_one);
            let result = SelectOp::new(ctx, step_positive, positive, negative);
            lowering.append(ctx, result.get_operation(), Some(dest));
            result.get_result(ctx)
        };
        let adjusted_lower = adjust(self, ctx, raw_lower);
        let adjusted_upper = adjust(self, ctx, raw_upper);
        let default_start = SelectOp::new(ctx, step_positive, zero, len_minus_one);
        self.append(ctx, default_start.get_operation(), Some(dest));
        let start = SelectOp::new(
            ctx,
            has_lower,
            adjusted_lower,
            default_start.get_result(ctx),
        );
        self.append(ctx, start.get_operation(), Some(dest));
        let default_stop = SelectOp::new(ctx, step_positive, length, minus_one);
        self.append(ctx, default_stop.get_operation(), Some(dest));
        let stop = SelectOp::new(ctx, has_upper, adjusted_upper, default_stop.get_result(ctx));
        self.append(ctx, stop.get_operation(), Some(dest));
        let storage = self.entry_alloca(ctx, 24, 8);
        for (index, value) in [start.get_result(ctx), stop.get_result(ctx), step]
            .into_iter()
            .enumerate()
        {
            let address = self.offset_address(ctx, storage, index as u64 * 8);
            let store = StoreOp::new(ctx, value, address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.reg_values.insert(dest.0, storage);
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
        // The compiler-private pack fallback (the VM's `remove(0)` loop):
        // the split slot keeps the pack layout; a backend-side shadow slot
        // holds the advance position. Handled before the identity check —
        // in-place normalization still zeroes the position.
        if prepare.is_empty()
            && let Some(elements) = self.pack_iter_elements(dest)
            && (source == dest
                || matches!(
                    self.func.var_tys.get(&source),
                    Some(Ty::RuntimePack(_) | Ty::Tuple(_))
                ))
        {
            return self.lower_pack_iter_init(ctx, source, dest, &elements);
        }
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
    /// The pack element list of `iter`'s split slot, when the monomorphizer
    /// typed it for the compiler-private pack fallback.
    fn pack_iter_elements(&self, iter: u32) -> Option<Vec<Ty>> {
        match self.func.var_tys.get(&iter) {
            Some(Ty::RuntimePack(elements) | Ty::Tuple(elements)) => Some(elements.clone()),
            _ => None,
        }
    }

    /// The backend-side advance position of a pack-fallback iterator slot
    /// (the slot itself keeps the pack layout), created on first use.
    fn pack_position_slot(&mut self, ctx: &mut Context, iter: u32) -> Value {
        if let Some(slot) = self.pack_positions.get(&iter) {
            return *slot;
        }
        // A typed slot: mem2reg promotes an alloca at its element type, and
        // a byte-array slot would promote as `i8` under the i64 loads.
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let slot = self.entry_typed_alloca(ctx, i64_handle);
        self.pack_positions.insert(iter, slot);
        slot
    }

    /// Initialize a pack-fallback iterator: position zero, and — for a
    /// distinct destination slot — the pack bytes relocated (a raw move,
    /// the VM's iterator-slot pack copy).
    fn lower_pack_iter_init(
        &mut self,
        ctx: &mut Context,
        source: u32,
        dest: u32,
        elements: &[Ty],
    ) -> Result<(), PlironError> {
        for element in elements {
            if self.owns_heap(element) || self.has_nested_lifecycle(element, "__deinit__") {
                return Err(self.unsupported(
                    format!("pack iteration over lifecycle element `{element}`"),
                    None,
                ));
            }
        }
        let position = self.pack_position_slot(ctx, dest);
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, position);
        self.append(ctx, store.get_operation(), None);
        if source != dest {
            let composed = self.struct_layout_of(elements, Reg(u32::MAX))?;
            let from = self.var_slots[source as usize];
            let to = self.var_slots[dest as usize];
            self.mem_copy(ctx, to, from, composed.layout.size, Reg(u32::MAX));
        }
        Ok(())
    }

    fn lower_has_next(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        iter: u32,
        method: Option<&str>,
    ) -> Result<(), PlironError> {
        let Some(method) = method else {
            // The compiler-private pack fallback: the shadow position
            // against the static element count.
            if let Some(elements) = self.pack_iter_elements(iter) {
                let position_slot = self.pack_position_slot(ctx, iter);
                let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let position = LoadOp::new(ctx, position_slot, i64_handle);
                self.append(ctx, position.get_operation(), Some(dest));
                let count = self.int_constant(ctx, elements.len() as i64);
                let more =
                    ICmpOp::new(ctx, ICmpPredicateAttr::SLT, position.get_result(ctx), count);
                return self.define(ctx, dest, more.get_operation(), more.get_result(ctx));
            }
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
            // The compiler-private pack fallback: read the element at the
            // cursor position and advance (the VM's `remove(0)` pop, with
            // the position standing in for the shift).
            if let Some(elements) = self.pack_iter_elements(iter) {
                let Some(first) = elements.first() else {
                    // An empty pack's advance is dead code (`HasNext` is
                    // statically false); define a zeroed destination.
                    match lower_ty(
                        self.name,
                        self.func.reg_types.get(&dest.0).unwrap_or(&Ty::Int),
                        &self.layout,
                        self.reg_span(dest),
                    )? {
                        LowerTy::Scalar(_) => {
                            let zero = self.int_constant(ctx, 0);
                            self.reg_values.insert(dest.0, zero);
                        }
                        LowerTy::Aggregate { layout, .. } => {
                            let storage = self.entry_alloca(ctx, layout.size, layout.align);
                            self.reg_values.insert(dest.0, storage);
                        }
                        LowerTy::ZeroSized => {
                            self.erased.insert(dest.0);
                        }
                    }
                    return Ok(());
                };
                if elements.iter().any(|element| element != first) {
                    return Err(self.unsupported_reg("heterogeneous pack advance".into(), dest));
                }
                let composed = self.struct_layout_of(&elements, dest)?;
                let stride = if elements.len() > 1 {
                    composed.offsets[1] - composed.offsets[0]
                } else {
                    0
                };
                let slot = self.var_slots[iter as usize];
                let position_slot = self.pack_position_slot(ctx, iter);
                let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
                let position = LoadOp::new(ctx, position_slot, i64_handle);
                self.append(ctx, position.get_operation(), Some(dest));
                let stride_value = self.int_constant(ctx, stride as i64);
                let scaled = MulOp::new_with_overflow_flag(
                    ctx,
                    position.get_result(ctx),
                    stride_value,
                    no_overflow_flags(),
                );
                self.append(ctx, scaled.get_operation(), Some(dest));
                let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
                let address = GetElementPtrOp::new(
                    ctx,
                    slot,
                    vec![GepIndex::Value(scaled.get_result(ctx))],
                    i8_ty,
                );
                self.append(ctx, address.get_operation(), Some(dest));
                let one = self.int_constant(ctx, 1);
                let next = AddOp::new_with_overflow_flag(
                    ctx,
                    position.get_result(ctx),
                    one,
                    no_overflow_flags(),
                );
                self.append(ctx, next.get_operation(), Some(dest));
                let store = StoreOp::new(ctx, next.get_result(ctx), position_slot);
                self.append(ctx, store.get_operation(), Some(dest));
                return self.load_from(ctx, address.get_result(ctx), first, dest);
            }
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
        if outcome.ok_is_reference {
            let mangled = signature.mangled.clone();
            let func_ty = signature.func_ty;
            return self.lower_try_next_reference(
                ctx, dest, yielded, iter, call, &outcome, mangled, func_ty,
            );
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

    /// `TryNext` over a reference-yielding raising `__next__`: the ok payload
    /// is a place pointer into the iterator's element storage. The ok edge
    /// reads through it and copies the element out (the VM's
    /// `CopyIteratorReference` adapter); the exhausted edge releases the
    /// caught error and leaves zeroed element bytes, like the value form.
    #[allow(clippy::too_many_arguments)]
    fn lower_try_next_reference(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        yielded: Reg,
        iter: u32,
        call: &crate::checked::CheckedIteratorCall,
        outcome: &OutcomeAbi,
        mangled: String,
        func_ty: TypedHandle<FuncType>,
    ) -> Result<(), PlironError> {
        // A byte copy stands in for the VM's lifecycle clone only when no
        // nested copy constructor could observe the difference (destructors
        // already rejected in `lower_try_next`).
        if self.has_nested_lifecycle(&call.result_ty, "__copyinit__") {
            return Err(self.unsupported_reg(
                format!(
                    "reference-yielded iterator element `{}` with a copy constructor",
                    call.result_ty
                ),
                dest,
            ));
        }
        let element = lower_ty(
            self.name,
            &call.result_ty,
            &self.layout,
            self.reg_span(dest),
        )?;
        let mut element_layout = self.layout.layout_of(&call.result_ty).map_err(|error| {
            self.unsupported_reg(format!("iterator element layout ({error})"), dest)
        })?;
        // A `for ref x` contract's temp slot holds the handle, not the
        // element (see `yields_reference` below).
        if call.reference_result.is_some() && call.result_adapter.is_none() {
            element_layout = Layout::new(8, 8);
        }
        let callee: Identifier = mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        let temp = self.entry_alloca(ctx, element_layout.size, element_layout.align);
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
        let ok_block = BasicBlock::new(ctx, None, vec![]);
        ok_block.insert_at_back(region, ctx);
        let exhausted_block = BasicBlock::new(ctx, None, vec![]);
        exhausted_block.insert_at_back(region, ctx);
        let join_block = BasicBlock::new(ctx, None, vec![]);
        join_block.insert_at_back(region, ctx);
        let branch = CondBrOp::new(
            ctx,
            is_ok.get_result(ctx),
            ok_block,
            vec![],
            exhausted_block,
            vec![],
        );
        self.append(ctx, branch.get_operation(), Some(dest));
        // A `for ref x` contract keeps the yielded reference itself (the
        // destination is a handle written through by the loop body); the
        // adapter contract copies the element out.
        let yields_reference = call.reference_result.is_some() && call.result_adapter.is_none();
        self.current = Some(ok_block);
        let ok_address = self.offset_address(ctx, storage, outcome.ok_offset);
        let ptr_handle: TypeHandle = PointerType::get(ctx, 0).into();
        let place = LoadOp::new(ctx, ok_address, ptr_handle);
        self.append(ctx, place.get_operation(), Some(dest));
        if yields_reference {
            let store = StoreOp::new(ctx, place.get_result(ctx), temp);
            self.append(ctx, store.get_operation(), Some(dest));
        } else {
            match &element {
                LowerTy::Scalar(scalar) => {
                    let handle = scalar.handle(ctx);
                    let value = LoadOp::new(ctx, place.get_result(ctx), handle);
                    self.append(ctx, value.get_operation(), Some(dest));
                    let store = StoreOp::new(ctx, value.get_result(ctx), temp);
                    self.append(ctx, store.get_operation(), Some(dest));
                }
                LowerTy::Aggregate { layout, .. } => {
                    self.mem_copy(ctx, temp, place.get_result(ctx), layout.size, dest);
                }
                LowerTy::ZeroSized => {}
            }
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(exhausted_block);
        let err_address = self.offset_address(ctx, storage, outcome.err_offset);
        self.emit_release_storage(ctx, err_address, &Ty::Error)?;
        if element_layout.size > 0 {
            self.mem_zero(ctx, temp, element_layout.size);
        }
        let jump = BrOp::new(ctx, join_block, vec![]);
        self.append(ctx, jump.get_operation(), None);
        self.current = Some(join_block);
        if yields_reference {
            // The handle value (never read on the exhausted edge — the loop
            // has ended). A handle to pointer-typed storage joins
            // `pointer_slot_refs` like `MakeRef`.
            let load = LoadOp::new(ctx, temp, ptr_handle);
            if let Some(reference) = &call.reference_result
                && matches!(*reference.referent, Ty::Pointer { .. })
            {
                self.pointer_slot_refs.insert(dest.0);
            }
            return self.define(ctx, dest, load.get_operation(), load.get_result(ctx));
        }
        match element {
            LowerTy::Scalar(scalar) => {
                let handle = scalar.handle(ctx);
                let load = LoadOp::new(ctx, temp, handle);
                self.define(ctx, dest, load.get_operation(), load.get_result(ctx))
            }
            LowerTy::Aggregate { .. } => {
                // Deliberately not an owned temporary: the following `DefVar`
                // copies the element out, and the zeroed exhausted bytes must
                // never release.
                self.reg_values.insert(dest.0, temp);
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
    #[allow(clippy::too_many_arguments)]
    fn lower_constructor(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        args: &[Reg],
        kwargs: &[(String, Reg)],
        arg_places: &[Option<MirPlace>],
        kwarg_places: &[Option<MirPlace>],
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
            let by_reference = self.signatures[&init].ref_params.clone();
            if params.is_empty() {
                return Err(
                    self.unsupported_reg(format!("`{init}` without an `out self` parameter"), dest)
                );
            }
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            let rest = &params[1..];
            let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
            let rest_by_reference = if by_reference.len() > 1 {
                &by_reference[1..]
            } else {
                &[]
            };
            let mut lowered = vec![storage];
            if kwargs.is_empty()
                && args.len() == rest.len()
                && !rest_by_reference.iter().any(|&by_ref| by_ref)
            {
                for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                    let owned = rest_owned.get(i).copied().unwrap_or(false);
                    lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
                }
            } else {
                lowered.extend(self.bind_call_slots(
                    ctx,
                    dest,
                    &init,
                    rest,
                    rest_owned,
                    rest_by_reference,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                )?);
            }
            self.emit_bound_call(ctx, dest, &init, lowered)?;
            // `__init__` returns nothing; the constructed value is the
            // storage its `out self` wrote through.
            self.erased.remove(&dest.0);
            self.reg_values.insert(dest.0, storage);
            // The constructed value owns its heap: consumers relocate it
            // (`DefVar`, stores) and a discarded result releases invisibly.
            // Without the mark, a store forks the value and the original's
            // buffers lose their releasing owner. A cross-block lifetime
            // keeps the pre-existing shared-bytes behavior instead of
            // rejecting.
            if self.owns_heap(&ty)
                && self
                    .last_uses
                    .get(&dest.0)
                    .is_none_or(|(block, _)| *block == self.position.0)
            {
                self.mark_owned_temp(dest, (*ty).clone())?;
            }
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
        if args.is_empty() && kwargs.len() == 1 && kwargs[0].0 == "copy" {
            // `String(copy=value)` deep-copies through the native bridge, the
            // VM's `construct_via_copy` over the stdlib copy constructor.
            let ty = Ty::Struct(crate::symbol::STDLIB_STRING_STRUCT.to_string(), vec![]);
            let lowered = lower_ty(self.name, &ty, &self.layout, self.reg_span(dest))?;
            let LowerTy::Aggregate { ty, layout } = lowered else {
                return Err(self.unsupported_reg("String copy layout".into(), dest));
            };
            let src = self.reg_ptr(ctx, kwargs[0].1)?;
            return self.copy_aggregate(ctx, dest, &ty, layout, src);
        }
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
        // A nominal struct converts through its `write_to` conformance over
        // a fresh accumulator — the VM's `format_value` struct arm. The
        // accumulated buffer transfers into the resulting owned string.
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
            && !crate::symbol::is_stdlib_string_struct(&name)
        {
            let writer = self.entry_alloca(ctx, 16, 8);
            self.mem_zero(ctx, writer, 16);
            self.append_struct_via_write_to(ctx, arg, &name, writer, dest)?;
            let (data, len) = self.string_parts(ctx, writer, dest);
            self.str_runtime.insert(
                dest.0,
                RuntimeStr {
                    data,
                    len,
                    owned: true,
                },
            );
            // The accumulator alloca is exactly the 16-byte `MjStrDesc`
            // StringLiteral storage; aggregate consumers read it directly.
            self.reg_values.insert(dest.0, writer);
            self.mark_owned_temp(dest, Ty::StringLiteral)?;
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
        // A bare reference-typed variable re-borrows: its slot stores a
        // handle (reference slots always hold real referent addresses), and
        // the made reference is that stored handle, collapsing the chain
        // like the VM's recursive `Value::Ref` reads. A projected place
        // whose designated element is itself a reference (a `List[ref T]`
        // element) instead addresses the slot — its consumers dereference
        // explicitly.
        if place.proj.is_empty()
            && let Ty::Ref(reference) = &ty
        {
            let handle = ScalarTy::Ptr.handle(ctx);
            let load = LoadOp::new(ctx, address, handle);
            self.append(ctx, load.get_operation(), Some(dest));
            if matches!(*reference.referent, Ty::Pointer { .. }) {
                self.pointer_slot_refs.insert(dest.0);
            }
            self.reg_values.insert(dest.0, load.get_result(ctx));
            return Ok(());
        }
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
        // Lifecycle events name types as the VM logs them: the bare template
        // (`List`), never the backend's monomorphized instance spelling
        // (`List$mono$TInt`). Checker-specialized names (`Tuple$t2[…]`) are
        // the runtime struct name on both sides and pass through.
        let text = text.split("$mono").next().unwrap_or(text);
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
                if let Some(leaves) = self.leaf_flags.get(&var).cloned() {
                    // Tracked depth-1 moves: destroy the surviving leaves;
                    // any absent leaf suppresses the whole-value destructor
                    // (the VM's partial-aggregate rule).
                    let cont = flag.map(|flag| self.begin_flag_guard(ctx, flag));
                    if self.has_lifecycle_method(&ty, "__deinit__") {
                        if self.fields_need_drop(&ty) {
                            let name = match ty.as_ref() {
                                Ty::Struct(name, _) => name.as_str(),
                                _ => "?",
                            };
                            return Err(self.unsupported(
                                format!("destructor of `{name}` with droppable fields"),
                                None,
                            ));
                        }
                        // `__deinit__` runs only when every tracked leaf
                        // survives.
                        let guards: Vec<Ptr<BasicBlock>> = leaves
                            .values()
                            .map(|&leaf| self.begin_flag_guard(ctx, leaf))
                            .collect();
                        self.emit_drop_value(ctx, ptr, &ty, false)?;
                        for guard in guards.into_iter().rev() {
                            self.end_flag_guard(ctx, guard);
                        }
                    } else {
                        self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves)?;
                    }
                    self.set_drop_flag(ctx, var, false);
                    if let Some(cont) = cont {
                        self.end_flag_guard(ctx, cont);
                    }
                    return Ok(());
                }
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
                if self.trace_lifecycle
                    && let Ty::Struct(name, _) = ty.as_ref()
                {
                    let name = name.clone();
                    self.emit_trace_text(ctx, crate::native::rt_abi::TRACE_CONSUME, &name);
                }
                if self.fields_need_drop(&ty) {
                    // The named explicit destructor consumed the aggregate;
                    // its surviving fields still receive their ordinary
                    // reverse-order destruction (the VM's `ConsumeVar`).
                    // Only struct consumption destroys fields, and deeper
                    // untracked moves reject rather than guess.
                    if !matches!(ty.as_ref(), Ty::Struct(..)) || self.partially_moved.contains(&var)
                    {
                        return Err(self.unsupported(
                            "variable consumption with droppable fields".into(),
                            None,
                        ));
                    }
                    let leaves = self.leaf_flags.get(&var).cloned().unwrap_or_default();
                    let ptr = self.var_slots[var as usize];
                    self.emit_surviving_leaf_drops(ctx, ptr, &ty, &leaves)?;
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

    /// Branch on `value != null` into a fresh guarded-work block, returning
    /// the continuation block — the pointer analogue of
    /// [`Self::begin_flag_guard`], closed by the same
    /// [`Self::end_flag_guard`].
    fn begin_nonnull_guard(&mut self, ctx: &mut Context, value: Value) -> Ptr<BasicBlock> {
        let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
        let null = ZeroOp::new(ctx, ptr_ty);
        self.append(ctx, null.get_operation(), None);
        let compare = ICmpOp::new(ctx, ICmpPredicateAttr::NE, value, null.get_result(ctx));
        self.append(ctx, compare.get_operation(), None);
        let region = self.region.expect("lowering is inside a function");
        let work = BasicBlock::new(ctx, None, vec![]);
        work.insert_at_back(region, ctx);
        let cont = BasicBlock::new(ctx, None, vec![]);
        cont.insert_at_back(region, ctx);
        let branch = CondBrOp::new(ctx, compare.get_result(ctx), work, vec![], cont, vec![]);
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
        // A whole-value (re)initialization makes every tracked leaf present
        // again.
        if value && let Some(leaves) = self.leaf_flags.get(&var) {
            let leaves: Vec<Value> = leaves.values().copied().collect();
            let present = self.bool_constant(ctx, true);
            for flag in leaves {
                let store = StoreOp::new(ctx, present, flag);
                self.append(ctx, store.get_operation(), None);
            }
        }
        let Some(&flag) = self.drop_flags.get(&var) else {
            return;
        };
        let constant = self.bool_constant(ctx, value);
        let store = StoreOp::new(ctx, constant, flag);
        self.append(ctx, store.get_operation(), None);
    }

    /// Destroy the surviving droppable top-level leaves of partially-tracked
    /// storage: leaves with a presence flag drop under that flag's guard, the
    /// rest unconditionally. Struct fields destroy in reverse declaration
    /// order and pack elements left-to-right — the VM's `drop_value` orders.
    fn emit_surviving_leaf_drops(
        &mut self,
        ctx: &mut Context,
        ptr: Value,
        ty: &Ty,
        leaves: &std::collections::BTreeMap<usize, Value>,
    ) -> Result<(), PlironError> {
        let (element_tys, forward) = match ty {
            Ty::Struct(name, _) => {
                let Some(decl) = self.struct_decls.get(name.as_str()).copied() else {
                    return Ok(());
                };
                let tys: Vec<Ty> = decl.fields.iter().map(|(_, field)| field.clone()).collect();
                (tys, false)
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => (elements.clone(), true),
            _ => return Ok(()),
        };
        let composed = self
            .layout
            .struct_layout(&element_tys)
            .map_err(|error| self.unsupported(format!("drop layout ({error})"), None))?;
        let order: Vec<usize> = if forward {
            (0..element_tys.len()).collect()
        } else {
            (0..element_tys.len()).rev().collect()
        };
        for position in order {
            let element = element_tys[position].clone();
            if !self.needs_drop(&element) {
                continue;
            }
            let offset = composed.offsets[position];
            let address = if offset == 0 {
                ptr
            } else {
                self.gep_byte_unspanned(ctx, ptr, offset)
            };
            match leaves.get(&position).copied() {
                Some(flag) => {
                    let cont = self.begin_flag_guard(ctx, flag);
                    self.emit_drop_value(ctx, address, &element, false)?;
                    self.end_flag_guard(ctx, cont);
                }
                None => self.emit_drop_value(ctx, address, &element, false)?,
            }
        }
        Ok(())
    }

    /// The top-level leaf position a depth-1 projection addresses — a
    /// declared field of a struct-typed variable or a constant element of
    /// tuple/pack storage. These are the only shapes the per-leaf presence
    /// flags track; anything else keeps the blanket partially-moved marker.
    fn leaf_position(&self, place: &MirPlace) -> Option<usize> {
        if place.proj.len() != 1 || place.through.is_some() {
            return None;
        }
        let ty = self.func.var_tys.get(&place.root)?;
        match (ty, &place.proj[0]) {
            (Ty::Struct(name, _), Proj::Field(field)) => self
                .struct_decls
                .get(name.as_str())
                .and_then(|decl| decl.fields.iter().position(|(name, _)| name == field)),
            (Ty::Tuple(elements) | Ty::RuntimePack(elements), Proj::ConstIndex(index)) => {
                (*index < elements.len()).then_some(*index)
            }
            _ => None,
        }
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
            // A retained callable's teardown lives behind its record header:
            // env null (thin/bare value) and header null (no owned droppable
            // captures) are no-ops, and the drop thunk nulls the header
            // after running, so drops of aliasing two-word copies are
            // idempotent per record — the VM's deep-copying closure clones
            // are a recorded divergence.
            Ty::Func { .. } => {
                let handle = ScalarTy::Ptr.handle(ctx);
                let env_address = self.gep_byte_unspanned(ctx, ptr, 8);
                let env = LoadOp::new(ctx, env_address, handle);
                self.append(ctx, env.get_operation(), None);
                let env = env.get_result(ctx);
                let cont_env = self.begin_nonnull_guard(ctx, env);
                let drop_thunk = LoadOp::new(ctx, env, handle);
                self.append(ctx, drop_thunk.get_operation(), None);
                let drop_thunk = drop_thunk.get_result(ctx);
                let cont_thunk = self.begin_nonnull_guard(ctx, drop_thunk);
                let void = VoidType::get(ctx).to_handle();
                let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
                let thunk_ty = FuncType::get(ctx, void, vec![ptr_ty], false);
                let call = CallOp::new(
                    ctx,
                    CallOpCallable::Indirect(drop_thunk),
                    thunk_ty,
                    vec![env],
                );
                self.append(ctx, call.get_operation(), None);
                self.end_flag_guard(ctx, cont_thunk);
                self.end_flag_guard(ctx, cont_env);
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
    /// `__deinit__`, any transitive field/element that does, the built-in
    /// error (its message buffer frees on drop), or a retained callable
    /// (its environment record may carry owned droppable captures — a
    /// null-guarded header dispatch, free when there are none).
    fn needs_drop(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Error | Ty::Func { .. })
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
            LowerTy::Aggregate { ty, .. } => {
                // A literal argument entering a nominal-String parameter
                // materializes through the constructor bridge — the VM's
                // runtime coercion for generic parameters the checker could
                // not wrap at check time.
                if matches!(ty.as_ref(), Ty::Struct(name, _)
                        if crate::symbol::is_stdlib_string_struct(name))
                    && !matches!(self.func.reg_types.get(&reg.0), Some(Ty::Struct(..)))
                    && (self.str_consts.contains_key(&reg.0)
                        || self.str_runtime.contains_key(&reg.0)
                        || matches!(self.func.reg_types.get(&reg.0), Some(Ty::StringLiteral)))
                {
                    return self.materialize_string_argument(ctx, reg, ty, owned, dest);
                }
                self.reg_ptr(ctx, reg)
            }
            LowerTy::ZeroSized => Err(self.unsupported_reg("zero-sized argument".into(), dest)),
        }
    }

    /// Materialize a literal-shaped register as an owned nominal String for
    /// a String-typed parameter slot. The register's storage becomes the
    /// materialized struct (its first 16 bytes still read as the literal
    /// descriptor); a borrowed materialization is released after the
    /// register's last use, an owned one transfers to the callee.
    fn materialize_string_argument(
        &mut self,
        ctx: &mut Context,
        reg: Reg,
        string_ty: &Ty,
        owned: bool,
        dest: Reg,
    ) -> Result<Value, PlironError> {
        let (data, len) = self.writer_argument_text(ctx, reg, dest)?;
        let copy = self.emit_alloc(ctx, len, 1, dest);
        self.mem_copy_dynamic(ctx, copy, data, len, dest);
        let storage = self.entry_alloca(ctx, 24, 8);
        self.store_string_fields(ctx, storage, copy, len, len, dest);
        self.reg_values.insert(reg.0, storage);
        if !owned {
            self.mark_owned_temp(reg, string_ty.clone())?;
        }
        Ok(storage)
    }

    /// Emit the call to compiled `name` with fully bound operands, prepending
    /// fresh sret storage for an aggregate return and defining or erasing
    /// `dest` by the callee's result kind.
    fn emit_bound_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        name: &str,
        operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        let signature = &self.signatures[name];
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let (func_ty, returns_value, sret, outcome) = (
            signature.func_ty,
            signature.returns_value,
            signature.sret,
            signature.outcome.clone(),
        );
        self.emit_call_shaped(
            ctx,
            dest,
            CallOpCallable::Direct(callee),
            func_ty,
            returns_value,
            sret,
            outcome,
            operands,
        )
    }

    /// Emit a direct or indirect call with fully bound operands under the
    /// shared result shape: a raising callee branches on its tagged outcome,
    /// an aggregate return takes prepended fresh sret storage, and `dest` is
    /// defined or erased by the result kind.
    #[allow(clippy::too_many_arguments)]
    fn emit_call_shaped(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        callable: CallOpCallable,
        func_ty: TypedHandle<FuncType>,
        returns_value: bool,
        sret: Option<Layout>,
        outcome: Option<OutcomeAbi>,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        if let Some(outcome) = outcome {
            return self.emit_raising_call(ctx, dest, callable, func_ty, outcome, operands);
        }
        if let Some(layout) = sret {
            let storage = self.entry_alloca(ctx, layout.size, layout.align);
            operands.insert(0, storage);
            let call = CallOp::new(ctx, callable, func_ty, operands);
            self.append(ctx, call.get_operation(), Some(dest));
            self.reg_values.insert(dest.0, storage);
            // The callee's return transferred ownership here; a discarded or
            // borrowed-only aggregate result is an owned temporary.
            if let Some(ty) = self.func.reg_types.get(&dest.0).cloned()
                && ((self.owns_heap(&ty) && self.releasable(&ty)) || self.stdlib_deinit_temp(&ty))
            {
                self.mark_owned_temp(dest, ty)?;
            }
            Ok(())
        } else {
            let call = CallOp::new(ctx, callable, func_ty, operands);
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
        callable: CallOpCallable,
        func_ty: TypedHandle<FuncType>,
        outcome: OutcomeAbi,
        mut operands: Vec<Value>,
    ) -> Result<(), PlironError> {
        // A reference-yielding raising callee's ok payload is the place
        // pointer; the `Scalar(Ptr)` extraction below defines the destination
        // as that handle — the checked `reference_result` contract.
        let storage = self.entry_alloca(ctx, outcome.layout.size, outcome.layout.align);
        operands.insert(0, storage);
        let call = CallOp::new(ctx, callable, func_ty, operands);
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
                if (self.owns_heap(&ty) && self.releasable(&ty)) || self.stdlib_deinit_temp(&ty) {
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
    /// VM's clone-on-read place semantics. A heap-owning aggregate clones
    /// deeply (a byte copy would alias buffers both owners release), and a
    /// releasable clone is an owned temporary.
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
            LowerTy::Aggregate { ty, layout } => {
                let storage = self.entry_alloca(ctx, layout.size, layout.align);
                if self.owns_heap(&ty) {
                    self.fork_value_into(ctx, storage, &ty, layout, address, dest)?;
                    self.reg_values.insert(dest.0, storage);
                    // The fork's own allocations are exactly its duplicated
                    // String/Error buffers, which the invisible-release rule
                    // frees regardless of user copy constructors.
                    self.mark_owned_temp(dest, (*ty).clone())?;
                    return Ok(());
                }
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
            LowerTy::Aggregate { ty, layout } => {
                let ptr = self.reg_ptr(ctx, src)?;
                // Owned string bytes cannot enter literal-typed storage:
                // the literal value model is drop-inert, so the buffer would
                // lose its releasing owner (the recorded literal-ownership
                // gap behind the struct-to-literal bridge rejection).
                if matches!(*ty, Ty::StringLiteral) && self.owned_temps.contains_key(&src.0) {
                    return Err(self.unsupported(
                        "owned string bytes entering drop-inert literal storage".into(),
                        self.reg_span(src),
                    ));
                }
                // An owned temporary transfers into the designated storage;
                // a borrowed heap-owning source clones instead — its byte
                // copy would alias buffers both owners release.
                if self.owned_temps.remove(&src.0).is_some() || !self.owns_heap(&ty) {
                    self.mem_copy(ctx, address, ptr, layout.size, src);
                    return Ok(());
                }
                self.fork_value_into(ctx, address, &ty, layout, ptr, src)
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

    /// Display one nominal struct by calling its unique compiled `write_to`
    /// instance over a fresh builtin-string accumulator, then write the
    /// accumulated bytes to stdout and free them.
    fn print_struct_via_write_to(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        name: &str,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let prefix = format!("{name}.write_to");
        let mut candidates = self
            .signatures
            .iter()
            .filter(|(fname, _)| fname.starts_with(&prefix));
        let Some((_, signature)) = candidates.next() else {
            return Err(self.unsupported_reg(
                format!("display of `{name}` without a compiled `write_to`"),
                dest,
            ));
        };
        if candidates.next().is_some() {
            return Err(self.unsupported_reg(
                format!("display of `{name}` with ambiguous `write_to` instances"),
                dest,
            ));
        }
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{prefix}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let writer = self.entry_alloca(ctx, 16, 8);
        self.mem_zero(ctx, writer, 16);
        let recv_ptr = self.reg_ptr(ctx, arg)?;
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![recv_ptr, writer],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        let (data, len) = self.string_parts(ctx, writer, dest);
        self.write_stdout(ctx, data, len, dest);
        self.emit_free(ctx, data);
        Ok(())
    }

    /// Append one nominal struct's display text into an existing
    /// builtin-string writer by calling its unique compiled `write_to`
    /// instance with that writer — the VM's `format_value` recursion when a
    /// `Writer.write` argument is itself a struct.
    fn append_struct_via_write_to(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        name: &str,
        writer: Value,
        dest: Reg,
    ) -> Result<(), PlironError> {
        let prefix = format!("{name}.write_to");
        let mut candidates = self
            .signatures
            .iter()
            .filter(|(fname, _)| fname.starts_with(&prefix));
        let Some((_, signature)) = candidates.next() else {
            return Err(self.unsupported_reg(
                format!("display of `{name}` without a compiled `write_to`"),
                dest,
            ));
        };
        if candidates.next().is_some() {
            return Err(self.unsupported_reg(
                format!("display of `{name}` with ambiguous `write_to` instances"),
                dest,
            ));
        }
        if signature.outcome.is_some() {
            return Err(self.unsupported_reg(format!("raising `{prefix}`"), dest));
        }
        let callee: Identifier = signature
            .mangled
            .as_str()
            .try_into()
            .expect("mangled names are identifier-safe");
        let func_ty = signature.func_ty;
        let recv_ptr = self.reg_ptr(ctx, arg)?;
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct(callee),
            func_ty,
            vec![recv_ptr, writer],
        );
        self.append(ctx, call.get_operation(), Some(dest));
        Ok(())
    }

    /// The builtin-string writer's `write`: grow-and-append each argument's
    /// display text into the `mut`-aliased `{data, len}` descriptor — the
    /// VM's `Value::Str` writer.
    fn lower_str_writer_write(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        args: &[Reg],
        recv_place: Option<&MirPlace>,
    ) -> Result<(), PlironError> {
        let descriptor = match recv_place {
            Some(place) => {
                let place = place.clone();
                self.place_address(ctx, &place, dest)?.0
            }
            None => self.reg_ptr(ctx, recv)?,
        };
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let ptr_handle: TypeHandle = PointerType::get(ctx, 0).into();
        let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        for arg in args {
            if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
                && !crate::symbol::is_stdlib_string_struct(&name)
            {
                self.append_struct_via_write_to(ctx, *arg, &name, descriptor, dest)?;
                continue;
            }
            let (chunk, chunk_len) = self.writer_argument_text(ctx, *arg, dest)?;
            let data = LoadOp::new(ctx, descriptor, ptr_handle);
            self.append(ctx, data.get_operation(), Some(dest));
            let len_address = self.offset_address(ctx, descriptor, 8);
            let len = LoadOp::new(ctx, len_address, i64_handle);
            self.append(ctx, len.get_operation(), Some(dest));
            let total = AddOp::new_with_overflow_flag(
                ctx,
                len.get_result(ctx),
                chunk_len,
                no_overflow_flags(),
            );
            self.append(ctx, total.get_operation(), Some(dest));
            let merged = self.emit_alloc(ctx, total.get_result(ctx), 1, dest);
            self.mem_copy_dynamic(ctx, merged, data.get_result(ctx), len.get_result(ctx), dest);
            let tail = GetElementPtrOp::new(
                ctx,
                merged,
                vec![GepIndex::Value(len.get_result(ctx))],
                i8_ty,
            );
            self.append(ctx, tail.get_operation(), Some(dest));
            self.mem_copy_dynamic(ctx, tail.get_result(ctx), chunk, chunk_len, dest);
            self.emit_free(ctx, data.get_result(ctx));
            let store = StoreOp::new(ctx, merged, descriptor);
            self.append(ctx, store.get_operation(), Some(dest));
            let store = StoreOp::new(ctx, total.get_result(ctx), len_address);
            self.append(ctx, store.get_operation(), Some(dest));
        }
        self.erased.insert(dest.0);
        Ok(())
    }

    /// Write the UTF-8 bytes of a string-valued register to stdout when the
    /// register holds one of the supported string shapes — an interned
    /// constant, a runtime StringLiteral (descriptor or typed storage), or a
    /// nominal String. Returns whether the register was such a string.
    fn try_write_string_bytes(
        &mut self,
        ctx: &mut Context,
        arg: Reg,
        dest: Reg,
    ) -> Result<bool, PlironError> {
        if let Some(bytes) = self.str_consts.get(&arg.0).cloned() {
            self.write_literal_bytes(ctx, &bytes, dest);
            return Ok(true);
        }
        if let Some(descriptor) = self.str_runtime.get(&arg.0).copied() {
            self.write_stdout(ctx, descriptor.data, descriptor.len, dest);
            return Ok(true);
        }
        // A nominal String's byte buffer (the VM's `write_to` bridge reads
        // the same bytes), or a runtime StringLiteral value's (typed
        // storage) descriptor bytes.
        let is_string = match self.func.reg_types.get(&arg.0) {
            Some(Ty::Struct(name, _)) => crate::symbol::is_stdlib_string_struct(name),
            Some(Ty::StringLiteral) => true,
            _ => false,
        };
        if is_string {
            let ptr = self.reg_ptr(ctx, arg)?;
            let (data, size) = self.string_parts(ctx, ptr, dest);
            self.write_stdout(ctx, data, size, dest);
            return Ok(true);
        }
        Ok(false)
    }

    /// Emit the display bytes of one `print` argument.
    fn print_value(&mut self, ctx: &mut Context, arg: Reg, dest: Reg) -> Result<(), PlironError> {
        if self.try_write_string_bytes(ctx, arg, dest)? {
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
        // A nominal struct displays through its `write_to` conformance over
        // the builtin-string accumulator — the VM's `format_value` dispatch.
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&arg.0).cloned()
            && !crate::symbol::is_stdlib_string_struct(&name)
        {
            return self.print_struct_via_write_to(ctx, arg, &name, dest);
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
    /// The `(data, len)` byte pair of a string-shaped operand: an interned
    /// constant, a runtime string pair, or StringLiteral/String descriptor
    /// storage.
    fn string_operand_parts(
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
            Some(Ty::StringLiteral | Ty::Error) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            Some(Ty::Struct(name, _)) if crate::symbol::is_stdlib_string_struct(name) => {
                let ptr = self.reg_ptr(ctx, reg)?;
                Ok(self.string_parts(ctx, ptr, dest))
            }
            _ => Err(self.unsupported_reg("string operand".into(), dest)),
        }
    }

    /// Runtime string equality: equal lengths and equal bytes, via an inline
    /// byte-compare loop over slot-backed state (mem2reg promotes it).
    fn lower_str_runtime_eq(
        &mut self,
        ctx: &mut Context,
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
    ) -> Result<(), PlironError> {
        let (a_data, a_len) = self.string_operand_parts(ctx, a, dest)?;
        let (b_data, b_len) = self.string_operand_parts(ctx, b, dest)?;
        let i1_handle: TypeHandle = IntegerType::get(ctx, 1, Signedness::Signless).into();
        let i64_handle: TypeHandle = IntegerType::get(ctx, 64, Signedness::Signless).into();
        let i8_handle: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
        let result_slot = self.entry_typed_alloca(ctx, i1_handle);
        let index_slot = self.entry_typed_alloca(ctx, i64_handle);
        let len_eq = ICmpOp::new(ctx, ICmpPredicateAttr::EQ, a_len, b_len);
        self.append(ctx, len_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, len_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let zero = self.int_constant(ctx, 0);
        let store = StoreOp::new(ctx, zero, index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let region = self.region.expect("lowering is inside a function");
        let head = BasicBlock::new(ctx, None, vec![]);
        head.insert_at_back(region, ctx);
        let body = BasicBlock::new(ctx, None, vec![]);
        body.insert_at_back(region, ctx);
        let done = BasicBlock::new(ctx, None, vec![]);
        done.insert_at_back(region, ctx);
        let enter = BrOp::new(ctx, head, vec![]);
        self.append(ctx, enter.get_operation(), Some(dest));
        // head: continue while `index < len` and no mismatch was found.
        self.current = Some(head);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let in_range = ICmpOp::new(ctx, ICmpPredicateAttr::ULT, index.get_result(ctx), a_len);
        self.append(ctx, in_range.get_operation(), Some(dest));
        let live = AndOp::new(ctx, in_range.get_result(ctx), result.get_result(ctx));
        self.append(ctx, live.get_operation(), Some(dest));
        let branch = CondBrOp::new(ctx, live.get_result(ctx), body, vec![], done, vec![]);
        self.append(ctx, branch.get_operation(), Some(dest));
        // body: compare one byte, fold into the result, advance.
        self.current = Some(body);
        let index = LoadOp::new(ctx, index_slot, i64_handle);
        self.append(ctx, index.get_operation(), Some(dest));
        let a_byte_ptr = GetElementPtrOp::new(
            ctx,
            a_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, a_byte_ptr.get_operation(), Some(dest));
        let a_byte = LoadOp::new(ctx, a_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, a_byte.get_operation(), Some(dest));
        let b_byte_ptr = GetElementPtrOp::new(
            ctx,
            b_data,
            vec![GepIndex::Value(index.get_result(ctx))],
            i8_handle,
        );
        self.append(ctx, b_byte_ptr.get_operation(), Some(dest));
        let b_byte = LoadOp::new(ctx, b_byte_ptr.get_result(ctx), i8_handle);
        self.append(ctx, b_byte.get_operation(), Some(dest));
        let byte_eq = ICmpOp::new(
            ctx,
            ICmpPredicateAttr::EQ,
            a_byte.get_result(ctx),
            b_byte.get_result(ctx),
        );
        self.append(ctx, byte_eq.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, byte_eq.get_result(ctx), result_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let one = self.int_constant(ctx, 1);
        let next =
            AddOp::new_with_overflow_flag(ctx, index.get_result(ctx), one, no_overflow_flags());
        self.append(ctx, next.get_operation(), Some(dest));
        let store = StoreOp::new(ctx, next.get_result(ctx), index_slot);
        self.append(ctx, store.get_operation(), Some(dest));
        let advance = BrOp::new(ctx, head, vec![]);
        self.append(ctx, advance.get_operation(), Some(dest));
        // done: the folded verdict, negated for `!=`.
        self.current = Some(done);
        let result = LoadOp::new(ctx, result_slot, i1_handle);
        self.append(ctx, result.get_operation(), Some(dest));
        let mut value = result.get_result(ctx);
        if matches!(op, InfixOp::Ne) {
            let truth = self.bool_constant(ctx, true);
            let flipped = XorOp::new(ctx, value, truth);
            self.append(ctx, flipped.get_operation(), Some(dest));
            value = flipped.get_result(ctx);
        }
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

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
        self.float_unary(ctx, "llvm.floor.f64", value, dest)
    }

    /// One unary f64 → f64 LLVM intrinsic (`llvm.floor.f64`,
    /// `llvm.ceil.f64`, `llvm.trunc.f64`, `llvm.round.f64`, `llvm.fabs.f64`).
    fn float_unary(
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
        let value = self.floor_div_value(ctx, dest, lhs, rhs)?;
        self.reg_values.insert(dest.0, value);
        Ok(())
    }

    /// The flooring quotient as a bare value (shared with `divmod`, which
    /// computes both halves for one destination).
    fn floor_div_value(
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
    fn lower_floor_mod(
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
    fn floor_mod_value(
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
        if self.name == "__toplevel__" {
            self.emit_toplevel_binding_releases(ctx)?;
        }
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

    /// Release `__toplevel__`'s heap-carrying bindings at its exit. Module
    /// scope admits only declarations, so the runtime values of `comptime`
    /// bindings are pure materialization residue: the VM abandons them to
    /// its arena (no destructor ever runs), and every later use reads a
    /// compile-time folded copy. The native release is the same invisible
    /// bookkeeping as the owned-temporary rule — stdlib-authored destructor
    /// chains are pure frees; a chain that would run a user destructor
    /// rejects rather than diverging from the VM's silence.
    fn emit_toplevel_binding_releases(&mut self, ctx: &mut Context) -> Result<(), PlironError> {
        for var in (0..self.func.n_vars as u32).rev() {
            let Some(ty) = self.func.var_tys.get(&var).cloned() else {
                continue;
            };
            if !matches!(self.var_lower_ty(var)?, LowerTy::Aggregate { .. })
                || !(self.needs_drop(&ty) || self.owns_heap(&ty))
            {
                continue;
            }
            if self.chain_runs_user_lifecycle(&ty, "__deinit__") {
                return Err(self.unsupported(
                    format!("module-level binding of `{ty}` whose teardown runs a user destructor"),
                    None,
                ));
            }
            let Some(flag) = self.drop_flags.get(&var).copied() else {
                return Err(self.unsupported(
                    format!("module-level binding of `{ty}` without a guarded slot"),
                    None,
                ));
            };
            let ptr = self.var_slots[var as usize];
            let cont = self.begin_flag_guard(ctx, flag);
            let traced = self.trace_lifecycle;
            self.trace_lifecycle = false;
            let released = self.emit_drop_value(ctx, ptr, &ty, false);
            self.trace_lifecycle = traced;
            released?;
            self.set_drop_flag(ctx, var, false);
            self.end_flag_guard(ctx, cont);
        }
        Ok(())
    }

    /// Whether `ty`'s teardown/copy chain can reach a user-authored
    /// lifecycle method (`__deinit__`/`__copyinit__`/`__moveinit__`),
    /// walking struct fields and pointer element types (a container reaches
    /// pointed-to elements through its compiled chain). Stdlib-authored
    /// chains are exempt: pure frees/relocations, nothing user-observable.
    fn chain_runs_user_lifecycle(&self, ty: &Ty, method: &str) -> bool {
        match ty {
            Ty::Struct(name, _) => {
                let template = name.split("$mono").next().unwrap_or(name);
                let stdlib = template.starts_with("__module$std$")
                    || crate::symbol::is_stdlib_string_struct(name)
                    || matches!(
                        template,
                        "List" | "Dict" | "Set" | "Optional" | "Array" | "Span" | "StringSpan"
                    );
                if !stdlib && self.declarations.contains_key(&format!("{name}.{method}")) {
                    return true;
                }
                self.struct_decls.get(name.as_str()).is_some_and(|decl| {
                    decl.fields
                        .iter()
                        .any(|(_, field)| self.chain_runs_user_lifecycle(field, method))
                })
            }
            Ty::Tuple(elements) | Ty::RuntimePack(elements) => elements
                .iter()
                .any(|element| self.chain_runs_user_lifecycle(element, method)),
            Ty::Pointer { element, .. } => self.chain_runs_user_lifecycle(element, method),
            _ => false,
        }
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
/// Emit the body of an `invoke` thunk (see [`ModuleShared::ensure_thunk`]):
/// load/take the capture arguments out of the environment record, forward
/// the out-pointer and every user argument unchanged, call the lifted
/// target directly, and return its result.
fn emit_thunk_body(
    ctx: &mut Context,
    func: FuncOp,
    target: &FnSignature,
    modes: &str,
    capture_offsets: &[u64],
    has_out: bool,
) {
    let entry = func.get_or_create_entry_block(ctx);
    let ptr_ty: TypeHandle = PointerType::get(ctx, 0).into();
    let i8_ty: TypeHandle = IntegerType::get(ctx, 8, Signedness::Signless).into();
    let mut next_argument = 0;
    let argument = |ctx: &Context, next: &mut usize| {
        let value = entry.deref(ctx).get_argument(*next);
        *next += 1;
        value
    };
    let mut operands = Vec::new();
    if has_out {
        operands.push(argument(ctx, &mut next_argument));
    }
    let env = argument(ctx, &mut next_argument);
    for (mode, offset) in modes.chars().zip(capture_offsets) {
        let slot = if *offset == 0 {
            env
        } else {
            let index = u32::try_from(*offset).expect("environment offsets fit u32");
            let gep = GetElementPtrOp::new(ctx, env, vec![GepIndex::Constant(index)], i8_ty);
            gep.get_operation().insert_at_back(entry, ctx);
            gep.get_result(ctx)
        };
        // Every capture parameter is a reference parameter (the lifted
        // environment prefix): a `Reference` slot stores the captured
        // place's address, an owned (`c`/`m`) slot stores the value inline
        // and passes its own address — the record is the stable storage the
        // VM's owned-capture re-referencing requires.
        if mode == 'r' {
            let load = LoadOp::new(ctx, slot, ptr_ty);
            load.get_operation().insert_at_back(entry, ctx);
            operands.push(load.get_result(ctx));
        } else {
            operands.push(slot);
        }
    }
    let physical_captures = modes.len();
    let mut remaining = target
        .params
        .iter()
        .enumerate()
        .skip(physical_captures)
        .filter(|(index, param)| {
            !matches!(param, LowerTy::ZeroSized)
                || target.ref_params.get(*index).copied().unwrap_or(false)
        })
        .count();
    while remaining > 0 {
        operands.push(argument(ctx, &mut next_argument));
        remaining -= 1;
    }
    let callee: Identifier = target
        .mangled
        .as_str()
        .try_into()
        .expect("mangled names are identifier-safe");
    let call = CallOp::new(
        ctx,
        CallOpCallable::Direct(callee),
        target.func_ty,
        operands,
    );
    call.get_operation().insert_at_back(entry, ctx);
    let result = target.returns_value.then(|| call.get_result(ctx));
    let ret = ReturnOp::new(ctx, result);
    ret.get_operation().insert_at_back(entry, ctx);
}

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

/// Collect every `MovePlace` with a projection under `blocks`, recursing
/// into `try` sub-regions — the pre-scan that decides which variables need
/// per-leaf presence flags in the entry block.
fn collect_projected_move_places<'m>(
    blocks: &'m [crate::mir::MirBlock],
    out: &mut Vec<&'m MirPlace>,
) {
    for block in blocks {
        for instr in &block.instrs {
            match instr {
                MirInstr::MovePlace { place, .. } if !place.proj.is_empty() => out.push(place),
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    collect_projected_move_places(body, out);
                    if let Some((_, handler_blocks)) = handler {
                        collect_projected_move_places(handler_blocks, out);
                    }
                    if let Some(orelse_blocks) = orelse {
                        collect_projected_move_places(orelse_blocks, out);
                    }
                    if let Some(final_blocks) = finalbody {
                        collect_projected_move_places(final_blocks, out);
                    }
                }
                _ => {}
            }
        }
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

/// One already-lowered subscript actual: an index register (with its checked
/// place for `mut`/`ref` slots) or an inline-built slice-descriptor pointer.
enum SubscriptActual<'a> {
    Reg(Reg, Option<&'a MirPlace>),
    Descriptor(Value),
}

/// The checker-virtual slice-descriptor struct name behind `ty`, if any.
fn slice_struct_name(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name, _)
            if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") =>
        {
            Some(name)
        }
        Ty::Ref(reference) => slice_struct_name(&reference.referent),
        _ => None,
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
    fn subscript_arg_regs(arg: &crate::mir::MirSubscriptArg, out: &mut Vec<Reg>) {
        match arg {
            crate::mir::MirSubscriptArg::Index(reg) => out.push(*reg),
            crate::mir::MirSubscriptArg::Slice {
                lower, upper, step, ..
            } => out.extend([lower, upper, step].into_iter().flatten()),
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
        MirInstr::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            out.push(*object);
            out.extend([lower, upper, step].into_iter().flatten());
        }
        MirInstr::MultiIndex {
            object,
            args,
            kwargs,
            ..
        } => {
            out.push(*object);
            for arg in args.iter().chain(kwargs.iter().map(|(_, arg)| arg)) {
                subscript_arg_regs(arg, &mut out);
            }
        }
        MirInstr::MultiSet {
            receiver,
            args,
            value,
            ..
        } => {
            out.push(*receiver);
            for arg in args {
                subscript_arg_regs(arg, &mut out);
            }
            out.push(*value);
        }
        MirInstr::ReadRef { reference, .. } => out.push(*reference),
        MirInstr::WriteRef { reference, value } => out.extend([*reference, *value]),
        MirInstr::StoreRef { place, reference } => {
            place_regs(place, &mut out);
            out.push(*reference);
        }
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
