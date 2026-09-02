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
    AShrOp, AddOp, AddressOfOp, AllocaOp, AndOp, BitcastOp, BrOp, CallIntrinsicOp, CallOp,
    CondBrOp, ConstantOp, FAddOp, FCmpOp, FDivOp, FMulOp, FNegOp, FPExtOp, FPTruncOp, FSubOp,
    FuncOp, GepIndex, GetElementPtrOp, GlobalOp, ICmpOp, LShrOp, LoadOp, MulOp, OrOp, ReturnOp,
    SDivOp, SExtOp, SIToFPOp, SRemOp, SelectOp, ShlOp, StoreOp, SubOp, TruncOp, UDivOp, UIToFPOp,
    URemOp, UnreachableOp, XorOp, ZExtOp, ZeroOp,
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
    /// Whether the body contains no MIR instructions. Empty lifted handlers
    /// abandon owned arguments in the VM arena; native code releases their
    /// storage invisibly after the call.
    pub empty_body: bool,
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
    pub kw_pack_index: Option<usize>,
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

mod abi;
mod arith;
mod binops;
mod builtins;
mod calls;
mod closures;
mod consts;
mod ctors;
mod drops;
mod emit;
mod errors;
mod instr;
mod iter;
mod methods;
mod module_env;
mod places;
mod pointers;
mod print;
mod release;
mod simd;
mod subscripts;
mod support;
mod term;
mod try_flow;
mod types;
mod variants;
mod vars;

pub(in crate::backend::pliron) use module_env::*;
pub(in crate::backend::pliron) use support::*;
pub(in crate::backend::pliron) use types::*;

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
        conditional_values: HashMap::new(),
        last_uses: HashMap::new(),
        position: (0, 0),
        erased: HashSet::new(),
        partially_moved: HashSet::new(),
        leaf_flags: HashMap::new(),
        aliased_receiver_regs: collect_aliased_receiver_regs(func, env.declarations),
        loaded_places: collect_loaded_places(&func.blocks),
        pointer_slot_refs: HashSet::new(),
        initialized_vars: HashSet::new(),
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

/// Resolves MIR source spans to pliron [`Location`]s against the compilation's
/// registered sources.
pub(super) struct Locator {
    sources: Vec<(String, pliron::location::Source, Vec<usize>)>,
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
    /// Runtime presence for values produced only on one control-flow edge.
    /// `TryNext` uses this to make the following loop binding inert on the
    /// exhausted edge, including for types with observable destructors.
    conditional_values: HashMap<u32, Value>,
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
    /// Receiver registers whose method call uses a retained caller place.
    /// Their preceding `LoadPlace` is semantic scaffolding; materializing an
    /// owned clone would create an extra lifecycle object the call ignores.
    aliased_receiver_regs: HashSet<u32>,
    /// Original places behind `LoadPlace` scaffolding. Immutable aggregate
    /// calls can borrow these directly even when the checker did not retain
    /// an explicit `arg_place` on the call contract.
    loaded_places: HashMap<u32, MirPlace>,
    /// `MakeRef` results that address pointer-typed storage: a value access
    /// through such a handle first loads the stored pointer (the VM's
    /// reference-pointer boundary). A plain pointer value (a loaded pointer
    /// field) needs no extra dereference, so the distinction is per
    /// register, not per type.
    pointer_slot_refs: HashSet<u32>,
    /// Variable slots that already carry a runtime value. Static `ref`
    /// bindings may be represented only by `EstablishLoans`; this separates
    /// them from reference-result variables whose handle was stored normally.
    initialized_vars: HashSet<u32>,
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
