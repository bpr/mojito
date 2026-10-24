//! Per-function lowering of scalar MIR to pliron's LLVM dialect.
//!
//! Registers map to SSA values; variable slots lower to entry-block allocas
//! with `load`/`store` at each `UseVar`/`DefVar` (pliron's mem2reg pass
//! rebuilds SSA afterwards). Region ops lower as upstream's
//! `LowerControlFlow` lowers `hlcf`: each region's block becomes an LLVM
//! block, and each value a region terminator carries becomes an argument of
//! the block it enters (`regions.rs`). Integer and float literals fold through their
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
use pliron::builtin::attributes::{
    FPDoubleAttr, FPHalfAttr, FPSingleAttr, IntegerAttr, StringAttr,
};
use pliron::builtin::op_interfaces::{
    CallOpCallable, OneResultInterface, SingleBlockRegionInterface,
};
use pliron::builtin::ops::ModuleOp;
use pliron::builtin::types::{FP16Type, FP32Type, FP64Type, IntegerType, Signedness};
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::location::Location;
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::r#type::{TypeHandle, Typed, TypedHandle};
use pliron::utils::apint::{APInt, bw};
use pliron::value::Value;
use pliron_llvm::attributes::{
    BytesAttr, FCmpPredicateAttr, FastmathFlagsAttr, ICmpPredicateAttr, IntegerOverflowFlagsAttr,
    LinkageAttr,
};
use pliron_llvm::op_interfaces::{
    AlignableOpInterface, BinArithOp, CastOpInterface, CastOpWithNNegInterface, FastMathFlags,
    FloatBinArithOpWithFastMathFlags, IntBinArithOpWithOverflowFlag,
};
use pliron_llvm::ops::{
    AShrOp, AddOp, AddressOfOp, AllocaOp, AndOp, BitcastOp, BrOp, CallIntrinsicOp, CallOp,
    CondBrOp, ConstantOp, ExtractElementOp, FAddOp, FCmpOp, FDivOp, FMulOp, FNegOp, FPExtOp,
    FPToSIOp, FPTruncOp, FSubOp, FuncOp, GepIndex, GetElementPtrOp, GlobalOp, ICmpOp,
    InsertElementOp, LShrOp, LoadOp, MulOp, OrOp, PoisonOp, PtrToIntOp, ReturnOp, SDivOp, SExtOp,
    SIToFPOp, SRemOp, SelectOp, ShlOp, ShuffleVectorOp, StoreOp, SubOp, TruncOp, UDivOp, UIToFPOp,
    URemOp, UndefOp, UnreachableOp, XorOp, ZExtOp, ZeroOp,
};
use pliron_llvm::types::{ArrayType, FuncType, PointerType, VectorType, VectorTypeKind, VoidType};

use mojito_ast::ast::{ArgConvention, Dtype, InfixOp, PrefixOp};
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::SourceSpan;
use mojito_mir::mir::{
    Const as MirConst, MirBlock, MirCaptureMode, MirClosureCapture, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirStructDeclaration, MirTerm, Proj, Raises, Reg,
    RegionFrame, TryLabel, UseMode,
};
use mojito_types::types::{Ty, TyArg, canonical_simd_ty, scalar_simd_dtype};

use mojito_native::native::layout::{Layout, LayoutCx, LayoutError};
use mojito_native::native::mangle;

use super::{PlironError, PlironErrorKind, RetKind, TrapCategory};

/// The callable identity of one reachable function: its mangled symbol,
/// LLVM-dialect function type, and lowered parameter/result kinds. Built by
/// [`declare_function`], consumed by call lowering. An aggregate-returning
/// function takes a prepended sret out-pointer and returns `void`; aggregate
/// parameters pass by pointer (the shared ABI's by-reference rule).
pub struct FnSignature {
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
    /// Whether the result is the value itself or a reference to it.
    pub result_form: ResultForm,
}

/// What a compiled function hands back: its result value, or one pointer to
/// caller-owned storage holding it (`func.returns_reference`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResultForm {
    Value,
    Reference,
}

/// The `{ tag: u32, ok: T, err: MjError }` outcome of a raising function,
/// laid out by the ordinary aggregate rules (`native::layout::outcome_layout`;
/// the tag sits at offset 0).
#[derive(Clone)]
pub struct OutcomeAbi {
    pub layout: Layout,
    pub ok_offset: u64,
    pub err_offset: u64,
    /// The lowered ok payload (`ZeroSized` for a `None` return).
    pub ok: LowerTy,
    /// Whether the ok payload is a place pointer rather than the value — the
    /// shape of every raising reference-returning callee, called directly
    /// or through a callable value. A raising call defines its destination
    /// as that handle.
    pub ok_is_reference: bool,
}

/// The physical ABI of an indirect call, derived from the callee register's
/// checked `TyKind::Func` contract by the same classification rules
/// [`declare_function`] applies to a compiled callee. The `invoke` thunk a
/// callable value carries has exactly this signature with the environment
/// pointer prepended after the out-pointer: `[outcome*|sret*], env*,
/// params...` (never both an sret and an outcome — the inherited invariant).
pub struct ContractAbi {
    /// The `invoke` call type, including the `env*` parameter.
    pub func_ty: TypedHandle<FuncType>,
    pub returns_value: bool,
    /// Whether the contract returns the referent's address (`ref_return`),
    /// as a compiled reference-returning callee does.
    pub returns_reference: bool,
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
/// `mjrt_trap`) and the interned per-target `invoke` thunks of retained
/// callables (`mjthunk_<n>`). The `mjrt_` and `mjthunk_` prefixes, like
/// `main`, are outside the injective `mj_` mangle image (see `mangle`).
pub struct ModuleShared {
    module: ModuleOp,
    rt_types: HashMap<&'static str, TypedHandle<FuncType>>,
    /// The libc callees `external_call` has declared so far (see
    /// `externs.rs`); like `rt_types`, declared once per module on demand.
    extern_types: HashMap<&'static str, TypedHandle<FuncType>>,
    strings: HashMap<Vec<u8>, Identifier>,
    /// Interned callable thunks, keyed by (mangled target, capture-mode
    /// string — `r`/`c`/`m` per leading capture parameter). One lifted body
    /// gets distinct thunks per mode vector because declaration sites
    /// capture with real modes while in-body forwarding sites re-capture
    /// everything by reference.
    thunks: HashMap<(String, String), Identifier>,
    /// Interned capture-record teardown thunks (`mjdrop_<n>`), keyed like
    /// [`Self::thunks`] by (target, capture-mode string).
    drop_thunks: HashMap<(String, String), Identifier>,
    /// Interned parameter-list element arrays (`mjplist_<n>`), keyed by
    /// their bytes and alignment.
    param_lists: HashMap<(Vec<u8>, u32), Identifier>,
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
mod externs;
mod instr;
mod iter;
mod methods;
mod module_env;
mod places;
mod pointers;
mod print;
mod regions;
mod release;
mod simd;
mod subscripts;
mod support;
mod term;
mod try_flow;
mod types;
mod variants;
mod vars;

pub use module_env::*;
pub use support::*;
pub use types::*;

use regions::{Carrier, OpenBlock, RegionEnd, RegionTarget};
use release::TempRelease;

/// The module-wide read-only lowering environment shared by every function
/// body: compiled signatures, call-binding declarations, and the span
/// locator.
pub struct LowerEnv<'a> {
    pub signatures: &'a HashMap<String, FnSignature>,
    pub declarations: &'a HashMap<String, MirFunctionDeclaration>,
    pub struct_decls: &'a HashMap<&'a str, &'a MirStructDeclaration>,
    /// The program's MIR declarations, which answer whether a copy of a
    /// value is the value ([`MirDeclarations::copy_is_identity`]).
    pub mir_declarations: &'a MirDeclarations,
    pub layout: LayoutCx<'a>,
    pub locator: &'a Locator,
    /// Emit `mjrt_trace` lifecycle events (test lane only).
    pub trace_lifecycle: bool,
}

/// Lower `func`'s body into the declared `func_op`.
pub fn lower_body(
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
        mir_declarations: env.mir_declarations,
        layout: env.layout,
        locator: env.locator,
        shared,
        trace_lifecycle: env.trace_lifecycle,
        reg_values: HashMap::new(),
        pending_literals: HashMap::new(),
        int_consts: HashMap::new(),
        pack_positions: HashMap::new(),
        str_consts: HashMap::new(),
        str_runtime: HashMap::new(),
        owned_temps: HashMap::new(),
        last_uses: HashMap::new(),
        temp_releases: HashMap::new(),
        open_blocks: Vec::new(),
        lazy_values: Vec::new(),
        erased: HashSet::new(),
        print_sink: None,
        partially_moved: HashSet::new(),
        ended_inside: HashSet::new(),
        leaf_flags: HashMap::new(),
        aliased_load_regs: collect_aliased_load_regs(func, env.declarations, env.mir_declarations),
        loaded_places: collect_loaded_places(&func.blocks),
        storage_reads: HashSet::new(),
        pointer_slot_refs: HashSet::new(),
        made_ref_places: HashMap::new(),
        reference_places: HashMap::new(),
        consumed_receivers: HashSet::new(),
        var_slots: Vec::new(),
        carriers: Vec::new(),
        region_targets: Vec::new(),
        try_frames: Vec::new(),
        trap_blocks: HashMap::new(),
        region: None,
        entry: None,
        scratch: None,
        sret_ptr: None,
        outcome_ptr: None,
        err_slot: None,
        propagate_block: None,
        edge_errors: edge_errors(&func.blocks),
        open_edge: None,
        current: None,
    };
    lowering.run(ctx, func_op)
}

/// Resolves MIR source spans to pliron [`Location`]s against the compilation's
/// registered sources.
pub struct Locator {
    sources: Vec<(String, pliron::location::Source, Vec<usize>)>,
}

/// The scalar value types the backend lowers. `Int` and `UInt` share the
/// signless i64 representation and differ only in operator selection; `Ptr`
/// is one opaque target pointer (checked `Pointer` values, origins erased);
/// `Dtype` is a `DType` value's one-byte upstream code; `Sized` is a width-1
/// SIMD scalar alias at its lane width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScalarTy {
    Int,
    UInt,
    Float64,
    Bool,
    Ptr,
    Dtype,
    /// A sized scalar alias (`Int8`…`Int64`, `UInt8`…`UInt64`, `Float16`,
    /// `Float32`).
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
pub enum LowerTy {
    Scalar(ScalarTy),
    Aggregate { ty: Ty, layout: Layout },
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

/// One enclosing `try` body during lowering: its label, which a raise
/// naming it carries, the landing carrier such a raise enters with the
/// handler's arguments, whose block binds the error for the handler, and
/// the blocks open at the op, which such a raise stays inside.
struct TryFrame {
    label: TryLabel,
    landing: usize,
    open: usize,
}

struct FnLowering<'a> {
    name: &'a str,
    func: &'a MirFunction,
    signatures: &'a HashMap<String, FnSignature>,
    declarations: &'a HashMap<String, MirFunctionDeclaration>,
    struct_decls: &'a HashMap<&'a str, &'a MirStructDeclaration>,
    /// The program's MIR declarations, which answer whether a value is
    /// register-passable trivial ([`MirDeclarations::copy_is_identity`]).
    mir_declarations: &'a MirDeclarations,
    layout: LayoutCx<'a>,
    locator: &'a Locator,
    shared: &'a mut ModuleShared,
    /// Emit `mjrt_trace` lifecycle events (test lane only).
    trace_lifecycle: bool,
    /// Materialized SSA value of each register.
    reg_values: HashMap<u32, Value>,
    /// Registers holding a not-yet-materialized literal.
    pending_literals: HashMap<u32, PendingLiteral>,
    /// Registers holding an `Int` constant: an index the elaborator folded
    /// (an unrolled `comptime for`'s), which projects pack storage
    /// statically as a literal does.
    int_consts: HashMap<u32, i64>,
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
    /// Each register's final read, as the chain of `(block, instruction)`
    /// positions from the body block down to the reading position: a read
    /// inside a region is a read by the region op holding it at every
    /// enclosing level. A terminator read is at `usize::MAX`.
    last_uses: HashMap<u32, Vec<(BlockKey, usize)>>,
    /// Where each owned temporary is released: its defining block, and the
    /// position there of its final read.
    temp_releases: HashMap<u32, TempRelease>,
    /// The blocks being lowered, outermost first, each with the position of
    /// the instruction being lowered in it.
    open_blocks: Vec<OpenBlock>,
    /// Registers whose value a read materialized on demand (a pending
    /// literal, a string descriptor) at the position being lowered, in
    /// order, each with the value it replaced: a region's own revert when
    /// it ends, since its block does not dominate its siblings.
    lazy_values: Vec<(u32, Option<Value>)>,
    /// Registers written by semantically erased instructions (analysis
    /// markers, void call results). Reading one is an internal invariant
    /// violation surfaced as a diagnostic, never a silent miscompile.
    erased: HashSet<u32>,
    /// The C `int` descriptor a `print(file=)` currently routes its bytes
    /// through (`None` outside such a call: `mjrt_write_stdout`).
    print_sink: Option<Value>,
    /// Variables a `MovePlace` moved a projection out of. Dropping such a
    /// variable must skip the moved parts (the VM tombstones them); when the
    /// drop would emit destructor work, lowering rejects instead of guessing.
    partially_moved: HashSet<u32>,
    /// Flagged top-level leaves a part inside of which an end (a
    /// `ConsumePlace` or `MarkDestroyed`) left missing, until the leaf
    /// itself ends: an inlined destructor's body ending its receiver's
    /// fields in the caller's place, before the caller consumes the place.
    /// A variable with one is partially moved.
    ended_inside: HashSet<(u32, usize)>,
    /// Per-variable `i1` presence flags for top-level leaves (struct fields
    /// or pack elements) that a depth-1 `MovePlace` moves out somewhere in
    /// the body — the VM's `Value::Moved` tombstones. Drops and consumption
    /// destroy exactly the surviving leaves and suppress the whole-value
    /// destructor when any tracked leaf is absent.
    leaf_flags: HashMap<u32, std::collections::BTreeMap<usize, Value>>,
    /// `LoadPlace` results whose every consumer borrows them: the register
    /// holds its place's address and no copy lifecycle runs (MIR's aggregate
    /// load is a shallow read; `CopyValue` is the copy boundary).
    aliased_load_regs: HashSet<u32>,
    /// Aggregate registers read out of storage (a load, a field, an
    /// element, a payload): byte snapshots sharing their storage's buffers,
    /// which the storage's drop may free while the register is still read.
    storage_reads: HashSet<u32>,
    /// Original places behind every `LoadPlace`. Immutable aggregate calls
    /// can borrow these directly even when the checker did not retain an
    /// explicit `arg_place` on the call contract.
    loaded_places: HashMap<u32, MirPlace>,
    /// `MakeRef` results that address pointer-typed storage: a value access
    /// through such a handle first loads the stored pointer (the VM's
    /// reference-pointer boundary). A plain pointer value (a loaded pointer
    /// field) needs no extra dereference, so the distinction is per
    /// register, not per type.
    pointer_slot_refs: HashSet<u32>,
    /// The place each `MakeRef` result addresses, typed as what the handle
    /// designates (the referent, where the handle forwards a stored one).
    made_ref_places: HashMap<u32, MirPlace>,
    /// The place whose address each `ref` binding's slot holds (the
    /// `MakeRef` its `DefVar` stores): a place accessed through that binding
    /// addresses from the stored handle with only the projections beyond
    /// that place.
    reference_places: HashMap<u32, MirPlace>,
    /// The slots a call's aliased `deinit` receiver consumed whole, as
    /// upstream's call consumes its `deinit` operand (`memConsume`): the
    /// `consume.var` MIR still emits after the call ends nothing more. The
    /// VM stores `Value::Moved` into the place before such a call, so its
    /// `consume.var` logs nothing either.
    consumed_receivers: HashSet<u32>,
    /// Alloca (pointer) value of each variable slot; aggregate parameter
    /// slots alias the incoming pointer (write-through), but an owned one of
    /// a register-passable trivial type, which the callee copies.
    var_slots: Vec<Value>,
    /// The blocks region values enter (arena — region targets and region
    /// ends refer by index).
    carriers: Vec<Carrier>,
    /// The region ops enclosing the block being lowered that a `break`,
    /// `continue`, `match.next`, or `match.complete` may leave, innermost
    /// last.
    region_targets: Vec<RegionTarget>,
    /// Enclosing `try` bodies, innermost last: a raise naming one jumps to
    /// its landing block.
    try_frames: Vec<TryFrame>,
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
    /// The entry-block `MjError` staging slot: a `raise` writes the
    /// in-flight error here before jumping to its `try`'s landing block or
    /// the propagate block. Created lazily.
    err_slot: Option<Value>,
    /// The per-function propagate block of a raising function: copies the
    /// staged error into the outcome's error slot, tags it, releases the
    /// heap buffers of still-initialized releasable locals (no user
    /// destructor — the VM abandons raising frames), and returns. Created
    /// lazily.
    propagate_block: Option<Ptr<BasicBlock>>,
    /// The error registers of the function's raise edges, each with the
    /// results its instruction defines, which only its normal path sees: a
    /// `raise` of one hands on, whole, the error its instruction raised,
    /// traces no new raise, and releases none of those results.
    edge_errors: HashMap<u32, Vec<Reg>>,
    /// The raise edge of the instruction being lowered ([`OpenEdge`]).
    open_edge: Option<OpenEdge>,
    current: Option<Ptr<BasicBlock>>,
}

/// The raise edge of a raising instruction while it lowers: its flag and
/// error registers, the storage its error lands in, and, once its call has
/// branched on the callee's outcome, the block the error path continues
/// in.
struct OpenEdge {
    raises: Raises,
    storage: Value,
    error_block: Option<Ptr<BasicBlock>>,
}

/// The error register of every raise edge in `blocks`, at any depth, with
/// the results of its instruction.
fn edge_errors(blocks: &[MirBlock]) -> HashMap<u32, Vec<Reg>> {
    let mut errors = HashMap::new();
    let mut pending: Vec<&[MirBlock]> = vec![blocks];
    while let Some(blocks) = pending.pop() {
        for instruction in blocks.iter().flat_map(|block| &block.instrs) {
            if let Some(raises) = instruction.raise_edge() {
                let mut results = Vec::new();
                mojito_mir::mir::verify::instruction_result_regs(instruction, &mut results);
                errors.insert(raises.error.0, results);
            }
            pending.extend(
                mojito_mir::mir::instruction_regions(instruction)
                    .into_iter()
                    .map(Vec::as_slice),
            );
        }
    }
    errors
}
