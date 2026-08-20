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
use pliron::builtin::attributes::{BytesAttr, FPDoubleAttr, IntegerAttr, StringAttr};
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
    FCmpPredicateAttr, FastmathFlagsAttr, ICmpPredicateAttr, IntegerOverflowFlagsAttr, LinkageAttr,
};
use pliron_llvm::op_interfaces::{
    AlignableOpInterface, BinArithOp, CastOpInterface, CastOpWithNNegInterface, FastMathFlags,
    FloatBinArithOpWithFastMathFlags, IntBinArithOpWithOverflowFlag,
};
use pliron_llvm::ops::{
    AShrOp, AddOp, AddressOfOp, AllocaOp, AndOp, BrOp, CallIntrinsicOp, CallOp, CondBrOp,
    ConstantOp, FAddOp, FCmpOp, FDivOp, FMulOp, FNegOp, FSubOp, FuncOp, GepIndex, GetElementPtrOp,
    GlobalOp, ICmpOp, LShrOp, LoadOp, MulOp, OrOp, ReturnOp, SDivOp, SIToFPOp, SRemOp, SelectOp,
    ShlOp, StoreOp, SubOp, UDivOp, UIToFPOp, URemOp, UnreachableOp, XorOp, ZExtOp,
};
use pliron_llvm::types::{ArrayType, FuncType, PointerType, VoidType};

use crate::ast::{InfixOp, PrefixOp};
use crate::call::{ArgSlot, CallVariadics, match_call_slots};
use crate::checked::CheckedConst;
use crate::literal::{FloatLiteral, IntLiteral};
use crate::mir::{
    Const as MirConst, MirBlockId, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirStructDeclaration, MirTerm, Proj, Reg, UseMode,
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
    /// Whether each parameter is consuming — the callee takes ownership and
    /// destroys the value, so passing an owned temporary transfers it.
    pub owned_params: Vec<bool>,
    /// Whether the receiver (parameter 0) is a `deinit` destructor receiver —
    /// its final state writes back to the caller's receiver place, exactly
    /// like a `mut` receiver.
    pub deinit_receiver: bool,
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
    let (result, returns_value, ret, sret) = match lower_ty(name, ret_ty, layout, None)? {
        LowerTy::ZeroSized => (VoidType::get(ctx).to_handle(), false, RetKind::Void, None),
        LowerTy::Scalar(scalar) => (scalar.handle(ctx), true, scalar.ret_kind(), None),
        LowerTy::Aggregate { layout, .. } => (
            VoidType::get(ctx).to_handle(),
            false,
            RetKind::Void,
            Some(layout),
        ),
    };
    let mut params = Vec::with_capacity(func.param_types.len());
    let mut param_handles = Vec::with_capacity(func.param_types.len() + 1);
    if sret.is_some() {
        param_handles.push(PointerType::get(ctx, 0).into());
    }
    for ty in &func.param_types {
        let lowered = lower_ty(name, ty, layout, None)?;
        match &lowered {
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
            owned_params: func.owned_params.clone(),
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
        reg_values: HashMap::new(),
        pending_literals: HashMap::new(),
        str_consts: HashMap::new(),
        str_runtime: HashMap::new(),
        owned_temps: HashMap::new(),
        last_uses: HashMap::new(),
        position: (0, 0),
        erased: HashSet::new(),
        partially_moved: HashSet::new(),
        var_slots: Vec::new(),
        blocks: Vec::new(),
        trap_blocks: HashMap::new(),
        region: None,
        entry: None,
        scratch: None,
        sret_ptr: None,
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
    callees: &[String],
) -> Result<(), PlironError> {
    let void = VoidType::get(ctx).to_handle();
    let i32_ty: TypeHandle = IntegerType::get(ctx, 32, Signedness::Signless).into();
    let version_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let version = FuncOp::new(
        ctx,
        "mjrt_version".try_into().expect("valid identifier"),
        version_ty,
    );
    module.append_operation(ctx, version.get_operation(), 0);
    let wrapper_ty = FuncType::get(ctx, i32_ty, vec![], false);
    let wrapper = FuncOp::new(
        ctx,
        "main".try_into().expect("valid identifier"),
        wrapper_ty,
    );
    module.append_operation(ctx, wrapper.get_operation(), 0);
    let entry = wrapper.get_or_create_entry_block(ctx);

    let version_call = CallOp::new(
        ctx,
        CallOpCallable::Direct("mjrt_version".try_into().expect("valid identifier")),
        version_ty,
        vec![],
    );
    version_call.get_operation().insert_at_back(entry, ctx);
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
/// is one opaque target pointer (checked `Pointer` values, origins erased).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScalarTy {
    Int,
    UInt,
    Float64,
    Bool,
    Ptr,
}

impl ScalarTy {
    fn handle(self, ctx: &mut Context) -> TypeHandle {
        match self {
            ScalarTy::Int | ScalarTy::UInt => {
                IntegerType::get(ctx, 64, Signedness::Signless).into()
            }
            ScalarTy::Float64 => FP64Type::get(ctx).into(),
            ScalarTy::Bool => IntegerType::get(ctx, 1, Signedness::Signless).into(),
            ScalarTy::Ptr => PointerType::get(ctx, 0).into(),
        }
    }

    fn ret_kind(self) -> RetKind {
        match self {
            ScalarTy::Int => RetKind::I64,
            ScalarTy::UInt => RetKind::U64,
            ScalarTy::Float64 => RetKind::F64,
            ScalarTy::Bool => RetKind::Bool,
            ScalarTy::Ptr => RetKind::Ptr,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ScalarTy::Int => "Int",
            ScalarTy::UInt => "UInt",
            ScalarTy::Float64 => "Float64",
            ScalarTy::Bool => "Bool",
            ScalarTy::Ptr => "Pointer",
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

/// Classify a checked type for lowering: the four scalars stay SSA; `None`
/// is zero-sized; struct and tuple aggregates take their shared-engine
/// layout; everything else (pointers, variants, errors, references, SIMD,
/// literal types) stays outside the supported subset with a contextual
/// rejection.
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
        // Origins and ownership facts erase after validation; a pointer is
        // one opaque target pointer regardless of its element type.
        Ty::Pointer { .. } => Ok(LowerTy::Scalar(ScalarTy::Ptr)),
        Ty::None => Ok(LowerTy::ZeroSized),
        Ty::Struct(..) | Ty::Tuple(_) | Ty::RuntimePack(_) => match layout.layout_of(ty) {
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

struct FnLowering<'a> {
    name: &'a str,
    func: &'a MirFunction,
    signatures: &'a HashMap<String, FnSignature>,
    declarations: &'a HashMap<String, MirFunctionDeclaration>,
    struct_decls: &'a HashMap<&'a str, &'a MirStructDeclaration>,
    layout: LayoutCx<'a>,
    locator: &'a Locator,
    shared: &'a mut ModuleShared,
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
    /// Alloca (pointer) value of each variable slot; aggregate parameter
    /// slots alias the incoming pointer (write-through).
    var_slots: Vec<Value>,
    /// Pliron block for each MIR block id.
    blocks: Vec<Ptr<BasicBlock>>,
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

        // Entry: one alloca per variable slot, parameter stores, then a jump
        // to MIR block 0. An aggregate-returning function receives its sret
        // out-pointer as argument 0, shifting every parameter right by one;
        // aggregate parameter slots alias the incoming pointer directly
        // (write-through — `out`/`mut` receivers mutate caller storage), so
        // they allocate nothing.
        self.current = Some(entry);
        let signature = &self.signatures[self.name];
        let arg_offset = usize::from(signature.sret.is_some());
        if signature.sret.is_some() {
            self.sret_ptr = Some(entry.deref(ctx).get_argument(0));
        }
        let param_tys: Vec<Option<LowerTy>> = (0..self.func.n_vars)
            .map(|var| {
                (var < self.func.n_params).then(|| self.signatures[self.name].params[var].clone())
            })
            .collect();
        let one = self.int_constant(ctx, 1);
        for (var, param_ty) in param_tys.iter().enumerate() {
            match param_ty {
                Some(LowerTy::Aggregate { .. }) => {
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
            if matches!(param_ty, Some(LowerTy::Aggregate { .. })) {
                continue;
            }
            let value = entry.deref(ctx).get_argument(arg_offset + param);
            let store = StoreOp::new(ctx, value, self.var_slots[param]);
            self.append(ctx, store.get_operation(), None);
        }
        let jump = BrOp::new(ctx, self.blocks[0], vec![]);
        self.append(ctx, jump.get_operation(), None);

        // Final operand appearances drive the owned-temporary releases.
        for (id, block) in self.func.blocks.iter().enumerate() {
            for (index, instr) in block.instrs.iter().enumerate() {
                for reg in operand_regs(instr) {
                    self.last_uses.insert(reg.0, (id, index));
                }
            }
            for reg in terminator_regs(&block.term) {
                self.last_uses.insert(reg.0, (id, usize::MAX));
            }
        }

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
                self.store_to(ctx, address, &ty, *src)
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
                // A raising method compiles with an unchanged signature: no
                // `try` lowering exists before Stage 4, so a runtime raise
                // exits the process inside the callee.
                let _ = raises;
                if reference_result.is_some() || result_adapter.is_some() {
                    return Err(
                        self.unsupported_reg("reference-result method contract".into(), *dest)
                    );
                }
                // Erased type-parameter slots (`value: None`) carry no
                // runtime data and are permitted.
                if arg_places.iter().any(Option::is_some)
                    || kwarg_places.iter().any(Option::is_some)
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
                // A raising callee compiles with an unchanged signature: no
                // `try` lowering exists before Stage 4, so a runtime raise
                // exits the process inside the callee.
                let _ = raises;
                // Erased type-parameter slots (`value: None`) carry no
                // runtime data and are permitted.
                if arg_places.iter().any(Option::is_some)
                    || kwarg_places.iter().any(Option::is_some)
                    || !capture_accesses.is_empty()
                    || param_arg_regs.iter().any(|arg| arg.value.is_some())
                {
                    return Err(self.unsupported_reg(
                        format!("non-positional call contract for `{}`", func.0),
                        *dest,
                    ));
                }
                self.lower_call(ctx, *dest, &func.0, args, kwargs)
            }
            MirInstr::DropVar { var } => self.lower_drop_var(ctx, *var),
            MirInstr::ConsumeVar { var } => self.lower_consume_var(*var),
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
            // Everything below is outside the supported subset. Every variant
            // is named so that new instructions force a decision here.
            MirInstr::MakeRef { dest, .. }
            | MirInstr::ReadRef { dest, .. }
            | MirInstr::MakeClosure { dest, .. }
            | MirInstr::CallIndirect { dest, .. }
            | MirInstr::Index { dest, .. }
            | MirInstr::Slice { dest, .. }
            | MirInstr::MultiIndex { dest, .. }
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
            | MirInstr::HasNext { dest, .. }
            | MirInstr::Next { dest, .. }
            | MirInstr::TryNext { dest, .. } => {
                Err(self.unsupported_reg(format!("instruction `{}`", instr_name(instr)), *dest))
            }
            MirInstr::Raise { src } => self.lower_raise(ctx, *src),
            MirInstr::WriteRef { .. }
            | MirInstr::GetIter { .. }
            | MirInstr::VariantSet { .. }
            | MirInstr::VariantSetInitWith { .. }
            | MirInstr::VariantDeinitWith { .. }
            | MirInstr::MultiSet { .. }
            | MirInstr::StoreRef { .. }
            | MirInstr::PointerStorageDestroy { .. }
            | MirInstr::UninitStorageDestroy { .. }
            | MirInstr::Try { .. }
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
        let lowered_args = if kwargs.is_empty() && args.len() == params.len() {
            let mut lowered = Vec::with_capacity(args.len());
            for (i, (arg, expected)) in args.iter().zip(&params).enumerate() {
                let owned = owned.get(i).copied().unwrap_or(false);
                lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
            }
            lowered
        } else {
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
            (ScalarTy::Ptr, _) | (_, ScalarTy::Ptr) => {
                Err(self
                    .unsupported_reg(format!("conversion `{name}` over a Pointer operand"), dest))
            }
        }
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
            return Err(self.unsupported_reg(format!("variable use mode `{mode:?}`"), dest));
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
                let value = self.reg_value(ctx, src, expected)?;
                let store = StoreOp::new(ctx, value, self.var_slots[var as usize]);
                self.append(ctx, store.get_operation(), None);
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let ptr = self.reg_ptr(src)?;
                let slot = self.var_slots[var as usize];
                self.mem_copy(ctx, slot, ptr, layout.size, src);
                // The variable owns the value now; the temporary transfers.
                self.owned_temps.remove(&src.0);
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
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
                let src = self.reg_ptr(value)?;
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
        if place.through.is_some() {
            return Err(self.unsupported_reg("place access through a reference".into(), dest));
        }
        let mut ty = self
            .func
            .var_tys
            .get(&place.root)
            .cloned()
            .or_else(|| place.root_ty.clone())
            .ok_or_else(|| {
                self.unsupported_reg(format!("untyped place root ${}", place.root), dest)
            })?;
        let mut address = self
            .var_slots
            .get(place.root as usize)
            .copied()
            .ok_or_else(|| {
                self.unsupported_reg(format!("place root ${} out of range", place.root), dest)
            })?;
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
        let base_ptr = self.reg_ptr(base)?;
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
    fn lower_method_call(
        &mut self,
        ctx: &mut Context,
        dest: Reg,
        recv: Reg,
        method: &str,
        resolved: Option<&str>,
        args: &[Reg],
        kwargs: &[(String, Reg)],
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
        let deinit_receiver = signature.deinit_receiver;
        if params.is_empty() {
            return Err(self.unsupported_reg(
                format!("method `{resolved}` without a receiver parameter"),
                dest,
            ));
        }
        let recv_owned = owned.first().copied().unwrap_or(false);
        let recv_value = self.arg_value(ctx, recv, &params[0], recv_owned, dest)?;
        let rest = &params[1..];
        let rest_owned = if owned.len() > 1 { &owned[1..] } else { &[] };
        let mut lowered = vec![recv_value];
        if kwargs.is_empty() && args.len() == rest.len() {
            for (i, (arg, expected)) in args.iter().zip(rest).enumerate() {
                let owned = rest_owned.get(i).copied().unwrap_or(false);
                lowered.push(self.arg_value(ctx, *arg, expected, owned, dest)?);
            }
        } else {
            lowered
                .extend(self.bind_call_slots(ctx, dest, resolved, rest, rest_owned, args, kwargs)?);
        }
        self.emit_bound_call(ctx, dest, resolved, lowered)?;
        // `mut self` (the struct's mut_self_methods set — keyed by either the
        // overload-qualified or the source method name) and named destructors
        // write the receiver back; a missing place means a discarded
        // temporary receiver.
        let write_back = match self.func.reg_types.get(&recv.0) {
            Some(Ty::Struct(struct_name, _)) => {
                let is_mut = self
                    .struct_decls
                    .get(struct_name.as_str())
                    .is_some_and(|d| {
                        d.mut_self_methods.contains(resolved) || d.mut_self_methods.contains(method)
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
            let recv_ptr = self.reg_ptr(recv)?;
            let (address, _) = self.place_address(ctx, place, dest)?;
            self.mem_copy(ctx, address, recv_ptr, size, dest);
        }
        Ok(())
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
            let src = self.reg_ptr(kwargs[0].1)?;
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
            let ptr = self.reg_ptr(arg)?;
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
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
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

    /// `Raise`: report the raised message through `mjrt_unhandled_error`
    /// (which exits 64 + 5) — the pre-Stage-4 contract where no `try`
    /// lowering exists, so every runtime raise is dynamically unhandled.
    /// Lowering continues into a fresh unreachable block for the dead
    /// remainder of the MIR block.
    fn lower_raise(&mut self, ctx: &mut Context, src: Reg) -> Result<(), PlironError> {
        let (data, len) = self.string_bytes(ctx, src, src)?;
        let report_ty = self.shared.ensure_rt(ctx, "mjrt_unhandled_error");
        let call = CallOp::new(
            ctx,
            CallOpCallable::Direct("mjrt_unhandled_error".try_into().expect("valid identifier")),
            report_ty,
            vec![data, len],
        );
        self.append(ctx, call.get_operation(), Some(src));
        let unreachable = UnreachableOp::new(ctx);
        self.append(ctx, unreachable.get_operation(), None);
        let region = self.region.expect("lowering is inside a function region");
        let dead = BasicBlock::new(ctx, None, vec![]);
        dead.insert_at_back(region, ctx);
        self.current = Some(dead);
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
        if let Some(Ty::Struct(name, _)) = self.func.reg_types.get(&reg.0)
            && crate::symbol::is_stdlib_string_struct(name)
        {
            let ptr = self.reg_ptr(reg)?;
            return Ok(self.string_parts(ctx, ptr, dest));
        }
        Err(self.unsupported_reg(format!("string value in register %r{}", reg.0), dest))
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
                self.emit_drop_value(ctx, ptr, &ty, false)
            }
        }
    }

    /// `ConsumeVar`: skip the whole-value destructor but destroy residual
    /// fields — a no-op unless fields carry their own destructor work.
    fn lower_consume_var(&mut self, var: u32) -> Result<(), PlironError> {
        match self.var_lower_ty(var)? {
            LowerTy::Scalar(_) | LowerTy::ZeroSized => Ok(()),
            LowerTy::Aggregate { ty, .. } => {
                if self.fields_need_drop(&ty) {
                    return Err(
                        self.unsupported("variable consumption with droppable fields".into(), None)
                    );
                }
                Ok(())
            }
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
    /// `__deinit__`, or any transitive field/element that does.
    fn needs_drop(&self, ty: &Ty) -> bool {
        self.has_lifecycle_method(ty, "__deinit__") || self.fields_need_drop(ty)
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
            LowerTy::Aggregate { .. } => self.reg_ptr(reg),
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
                let value = self.reg_value(ctx, src, scalar)?;
                let store = StoreOp::new(ctx, value, address);
                self.append(ctx, store.get_operation(), Some(src));
                Ok(())
            }
            LowerTy::Aggregate { layout, .. } => {
                let ptr = self.reg_ptr(src)?;
                self.mem_copy(ctx, address, ptr, layout.size, src);
                // The designated storage owns the value now.
                self.owned_temps.remove(&src.0);
                Ok(())
            }
            LowerTy::ZeroSized => Ok(()),
        }
    }

    /// The storage pointer of an aggregate-valued register.
    fn reg_ptr(&self, reg: Reg) -> Result<Value, PlironError> {
        self.reg_values.get(&reg.0).copied().ok_or_else(|| {
            self.unsupported(
                format!("read of undefined aggregate register %r{}", reg.0),
                self.reg_span(reg),
            )
        })
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
            let ptr = self.reg_ptr(arg)?;
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
        let ty = match self.concrete_scalar_ty(arg)? {
            Some(ty) => ty,
            // A bare literal argument materializes at the VM's default kind.
            None => match self.func.reg_types.get(&arg.0) {
                Some(Ty::FloatLiteral) => ScalarTy::Float64,
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
        let symbol = match ty {
            ScalarTy::Int => "mjrt_fmt_i64",
            ScalarTy::UInt => "mjrt_fmt_u64",
            ScalarTy::Float64 => "mjrt_fmt_f64",
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
            ScalarTy::Ptr => {
                Err(self.unsupported_reg(format!("operator `{op:?}` on Pointer operands"), dest))
            }
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
            ScalarTy::Bool | ScalarTy::Ptr => unreachable!("rejected above"),
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
            MirTerm::Return(value) => {
                // A value-less return inside a value-returning function is
                // checker-guaranteed unreachable fall-off scaffolding.
                let ret_lower = match self.func.ret_ty.as_ref() {
                    Some(Ty::None) | None => None,
                    Some(other) => Some(lower_ty(self.name, other, &self.layout, None)?),
                };
                let lowered = match (value, ret_lower) {
                    (Some(reg), Some(LowerTy::Aggregate { layout, .. })) => {
                        // Copy the returned aggregate into the sret
                        // out-pointer and return void; the caller owns it.
                        let sret = self
                            .sret_ptr
                            .expect("aggregate-returning functions receive an sret pointer");
                        let ptr = self.reg_ptr(*reg)?;
                        self.mem_copy(ctx, sret, ptr, layout.size, *reg);
                        self.owned_temps.remove(&reg.0);
                        None
                    }
                    (Some(reg), Some(LowerTy::Scalar(expected))) => {
                        Some(self.reg_value(ctx, *reg, expected)?)
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
