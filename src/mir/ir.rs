//! Flattened MIR data model shared by lowering, analysis, and execution.

use super::*;

/// Whether the callee is responsible for running a parameter's `__deinit__` when
/// it reaches its last use still initialized — i.e. a plain consuming `var`
/// parameter (the caller transferred ownership; the callee must destroy it).
///
/// A `deinit` parameter is deliberately excluded: it is the destructor/
/// move-source convention (`__deinit__(deinit self)`, `__moveinit__(out self,
/// deinit other)`). Such a value is *consumed* — the function body transfers
/// its resources (move-only fields with `^`, copyable fields by copy) — so
/// auto-running its whole-value `__deinit__` at function end would destroy a value
/// whose resources already moved elsewhere, double-freeing it.
pub(super) fn is_owned(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Var))
}

/// Whether an argument convention is `deinit` — the destructor/move-source
/// convention (`__deinit__(deinit self)`, `__moveinit__(out self, deinit other)`).
/// The callee is responsible for tearing the value down, but as a *consume*:
/// its residual fields are destroyed while its whole-value `__deinit__` is skipped
/// (its resources moved into the receiver), so drop elaboration emits
/// `ConsumeVar` rather than `DropVar` for such a parameter.
pub(super) fn is_deinit(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Deinit))
}

/// Whether a `try` region's statements contain a `break`/`continue` that **leaves**
/// the region — targeting a loop *outside* it. Such an escape would need to name the
/// outer loop's target block, which the self-contained mini-CFG region can't express
/// (unlike a `return`, which surfaces as a `Flow::Return` the block driver handles).
/// Nested loops absorb their own `break`/`continue` (tracked via `loop_depth`);
/// nested `def`/`struct` bodies have their own control flow and are not scanned.
pub(super) fn region_crosses_control(body: &[Stmt]) -> bool {
    fn walk(stmts: &[Stmt], loop_depth: usize) -> bool {
        stmts.iter().any(|s| match &s.kind {
            StmtKind::Break | StmtKind::Continue => loop_depth == 0,
            StmtKind::If { branches, orelse } => {
                branches.iter().any(|(_, b)| walk(b, loop_depth))
                    || orelse.as_ref().is_some_and(|b| walk(b, loop_depth))
            }
            StmtKind::While { body, .. } | StmtKind::For { body, .. } => walk(body, loop_depth + 1),
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                walk(body, loop_depth)
                    || except.as_ref().is_some_and(|(_, b)| walk(b, loop_depth))
                    || orelse.as_ref().is_some_and(|b| walk(b, loop_depth))
                    || finalbody.as_ref().is_some_and(|b| walk(b, loop_depth))
            }
            _ => false,
        })
    }
    walk(body, 0)
}

/// Whether an argument convention is a written-back reference (`mut`/`ref`).
pub(super) fn is_ref(c: &Option<ArgConvention>) -> bool {
    matches!(c, Some(ArgConvention::Mut | ArgConvention::Ref))
}
use crate::hir::VarId;
use std::collections::HashMap;

/// A virtual ("infinite") register — a fresh one per intermediate value (SSA-ish).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reg(pub u32);

/// One source `[...]` argument after lowering. `name` preserves keyword
/// binding (`callback=increment`); `value` is absent for an erased type
/// argument. Semantic Origin/OriginSet arguments do not enter this list.
///
/// Keeping binding identity separate from the optional runtime register lets
/// the VM normalize arguments to declaration order before applying defaults.
/// A bare `None` register cannot express the difference between an omitted
/// parameter and a supplied type argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirParamArg {
    pub name: Option<String>,
    pub value: Option<Reg>,
}

/// Index of a basic block within a [`MirFunction`]'s `blocks`.
pub type MirBlockId = usize;

/// A source byte range `(start, end)` — re-exported from [`crate::token`], the
/// canonical span type stamped by the parser onto every AST node.
pub use crate::token::Span;

/// How a variable is used at a given site (set from `^` and param conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseMode {
    Copy,
    Move,
    BorrowShared,
    BorrowMut,
}

/// One already-evaluated argument to a multi-dimensional subscript.
#[derive(Debug, Clone)]
pub enum MirSubscriptArg {
    Index(Reg),
    Slice {
        kind: crate::types::SliceKind,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
    },
}

/// Checked non-nominal dispatch for an index or slice instruction. A missing
/// [`MirSubscriptCall`] is never an invitation for the VM to inspect the runtime
/// value and guess semantics: lowering records the exact compiler/runtime
/// storage family selected by the checked base type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirIntrinsicSubscript {
    /// Compiler-private heterogeneous Tuple/RuntimePack storage, including a
    /// nominal Tuple result whose intrinsic producer uses `Value::Tuple` as its
    /// ABI (currently `Slice.indices`).
    TupleStorage,
    /// Compiler-private homogeneous `*args` storage.
    VariadicStorage,
    Simd,
    Pointer,
    ComptimeList,
}

/// A compile-time-known literal.
#[derive(Debug, Clone)]
pub enum Const {
    Int(i64),
    Float(f64),
    IntLiteral(crate::literal::IntLiteral),
    FloatLiteral(crate::literal::FloatLiteral),
    Bool(bool),
    Str(String),
    Function(String),
    None,
}

/// The callee of a `MirInstr::Call` — a function/struct-constructor/builtin name.
/// (Resolution to a concrete target happens in the backend's assembler.)
#[derive(Debug, Clone)]
pub struct FuncRef(pub String);
impl FuncRef {
    pub fn named(name: &str) -> FuncRef {
        FuncRef(name.to_string())
    }
}

/// One step of a **place** projection: a field of a struct, or a subscript. A
/// place is a *writable location* — a root variable followed by projections — as
/// opposed to an rvalue (a computed register). This mirrors `rustc` MIR's
/// `Place`/`Projection` split, and is what a write / read-modify-write targets.
#[derive(Debug, Clone)]
pub enum Proj {
    Field(String),
    Index(Reg), // the subscript index, flattened to a register (evaluated once)
    /// A statically selected element of compiler-private heterogeneous Tuple
    /// storage. Unlike [`Proj::Index`], distinct constant indices are disjoint
    /// ownership paths, so moving element 0 does not move element 1.
    ConstIndex(usize),
    /// Payload of a checked `Variant` alternative.  The tag is static; runtime
    /// navigation traps if the active alternative differs.
    Variant(usize),
    /// Payload of compiler-private inline uninit storage (`__UninitStorage`).
    /// A final-step write initializes-or-overwrites without dropping the old
    /// payload; reads trap while the storage is uninitialized.
    UninitPayload,
}

/// A writable location: a root variable plus a chain of projections
/// (`p.items[i].x` = root `p`, proj `[Field("items"), Index(i), Field("x")]`).
#[derive(Debug, Clone)]
pub struct MirPlace {
    pub root: VarId,
    /// Checked type of the root slot before projections. `None` is permitted only
    /// for compatibility HIR built without a `CheckedProgram`; production MIR
    /// verification rejects it.
    pub root_ty: Option<Ty>,
    pub proj: Vec<Proj>,
    /// Result type after each corresponding projection in `proj`.
    pub projection_tys: Vec<Ty>,
    /// Checked type of the designated storage after all projections.
    pub ty: Option<Ty>,
    /// The local reference through which this place is accessed. `None` means
    /// direct owner access. This is static metadata ignored by the VM.
    pub through: Option<VarId>,
}

impl MirPlace {
    pub fn root(root: VarId, ty: Option<Ty>) -> Self {
        Self {
            root,
            root_ty: ty.clone(),
            proj: Vec::new(),
            projection_tys: Vec::new(),
            ty,
            through: None,
        }
    }

    pub fn project(&mut self, projection: Proj, ty: Ty) {
        self.proj.push(projection);
        self.projection_tys.push(ty.clone());
        self.ty = Some(ty);
    }

    pub fn is_typed(&self) -> bool {
        self.root_ty.is_some() && self.ty.is_some() && self.proj.len() == self.projection_tys.len()
    }
}

/// Canonical, runtime-erased identity of an interior storage generation after
/// stable checker owners have been mapped to MIR slots. `Interior` path
/// segments are invalidation domains; ordinary field/index segments retain
/// field sensitivity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MirInteriorOrigin {
    pub root: VarId,
    pub path: Vec<crate::origin::OriginSeg>,
}

/// One owner dependency carried by a reference or reference-bearing value.
/// `place` is the executable target; `interior` is the distinct analytical
/// generation identity when the target lives behind container-owned storage.
#[derive(Debug, Clone)]
pub struct MirLoan {
    pub place: MirPlace,
    pub mutable: bool,
    pub interior: Option<MirInteriorOrigin>,
}

/// Complete checker-selected contract for a nominal subscript invocation.
/// Intrinsic pointer/SIMD/private-storage operations carry no such payload.
#[derive(Debug, Clone)]
pub struct MirSubscriptCall {
    pub target: String,
    pub raises: Option<Ty>,
    /// Checker-selected executable result type, including the instantiated
    /// origin and mutability of a reference result.
    pub result_ty: Ty,
    pub receiver_requires_place: bool,
    pub receiver_convention: Option<crate::ast::ArgConvention>,
    pub arguments: Vec<crate::checked::CheckedCallArgument>,
    pub capture_accesses: Vec<MirCaptureAccess>,
    pub reference_result: Option<crate::origin::RefTy>,
    pub param_arg_regs: Vec<MirParamArg>,
    pub param_decls: Vec<crate::types::ParamDecl>,
}

/// A single three-address instruction. Each value-producing instruction writes a
/// fresh `dest` register; control flow lives in the block's [`MirTerm`].
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum MirInstr {
    /// Establish one fresh generation containing every owner loan carried by a
    /// reference or aggregate binding. Grouping the loans makes rebinding reset
    /// the old generation atomically instead of merging historical loans for
    /// the same variable slot.
    EstablishLoans {
        reference: VarId,
        loans: Vec<MirLoan>,
        marker: Reg,
        /// Interior destination domain within `reference` that holds the
        /// loans (`None` ≡ the whole root). Rebinding a place that covers
        /// this domain releases the generation; sibling domains coexist.
        dest_interior: Option<MirInteriorOrigin>,
    },
    /// Invalidate established interior generations rooted below `base`.
    /// `include_base_generation` also replaces the exact named generation at
    /// `base`; `except` preserves the generation used to perform an ordinary
    /// mutation through an interior reference while still invalidating nested
    /// interiors.
    InvalidateInteriors {
        base: MirInteriorOrigin,
        except: Option<VarId>,
        include_base_generation: bool,
        marker: Reg,
    },
    /// Materialize a runtime reference handle to a verified place. If the root
    /// is already a reference parameter, its handle is forwarded and extended.
    MakeRef {
        dest: Reg,
        place: MirPlace,
    },
    ReadRef {
        dest: Reg,
        reference: Reg,
    },
    /// Materialize an owned copy of a register value.  Reference-returning
    /// expressions use this after `ReadRef` in ordinary value contexts; an
    /// explicit `ref` binding retains the handle and therefore never emits it.
    /// Keeping the operation explicit prevents the VM from guessing whether a
    /// handle read is a borrow, a forwarding read, or a lifecycle copy.
    CopyValue {
        dest: Reg,
        value: Reg,
    },
    WriteRef {
        reference: Reg,
        value: Reg,
    },
    /// Build a non-escaping closure value from a lifted function and its explicit
    /// environment. The capture mode fixes whether declaration evaluation stores
    /// a reference handle, an owned snapshot, or a transferred value.
    MakeClosure {
        dest: Reg,
        function: String,
        captures: Vec<MirClosureCapture>,
    },
    /// Extend an owner's MIR live range through a closure invocation without
    /// performing a value-level copy. This is erased by execution.
    KeepAlive {
        var: VarId,
    },
    Const {
        dest: Reg,
        k: Const,
    },
    /// Cross the compile-time literal boundary selected by the checker.  The
    /// operand is an exact `IntLiteral`/`FloatLiteral`; `target` is a concrete
    /// scalar (or width-one scalar alias).  Backends must implement this
    /// conversion explicitly rather than infer it from a later store.
    MaterializeLiteral {
        dest: Reg,
        value: Reg,
        target: Ty,
    },
    /// `x`, `x^`, `borrow x`, … — a use of a variable, tagged with how (`mode`).
    UseVar {
        dest: Reg,
        var: VarId,
        mode: UseMode,
    },
    /// A **partial move** `p.a^`, or a constant-index move from compiler-private
    /// Tuple storage — transfer one independently owned sub-place out of a
    /// variable, reading its value into `dest`. The ownership analysis tracks
    /// this at place granularity (moving `p.a` leaves `p.b` usable, and moving
    /// Tuple element 0 leaves element 1 usable); at runtime the slot is left a
    /// tombstone so a later aggregate drop skips it. Whole-variable moves stay
    /// `UseVar { mode: Move }`; user-facing indexed transfers remain restricted
    /// by checking to copyable value reads.
    MovePlace {
        dest: Reg,
        place: MirPlace,
    },
    /// `var := <register>` — (re)define a variable slot from a register (lowered
    /// from a HIR `Bind`). The write paired with `UseVar`; Stage 6 reads it as a
    /// dataflow *def* (transitions the var to `Owned`). `binding_ty` is the
    /// checker-resolved destination type; source annotation syntax never enters
    /// MIR. `None` on reassignment keeps the slot's existing runtime type.
    DefVar {
        var: VarId,
        src: Reg,
        binding_ty: Option<Ty>,
    },
    UnOp {
        op: PrefixOp,
        dest: Reg,
        a: Reg,
    },
    BinOp {
        op: InfixOp,
        dest: Reg,
        a: Reg,
        b: Reg,
        /// Checker-selected operator implementation when nominal overload
        /// resolution was required (notably a concrete Tuple membership
        /// overload). Backends must not reselect it by name/arity.
        resolved: Option<String>,
    },
    /// A free-function / constructor / builtin call. `args` are the flattened
    /// positional arguments; `kwargs` the keyword arguments (`name = value`). The
    /// backend matches them to the callee's parameter slots (filling defaults,
    /// collecting `*args`) via the phase-neutral call matcher.
    /// A free-function call. `arg_places[i]` is `Some` only when checking selected
    /// a `mut`/`ref` parameter and positional argument `i` is a simple place (a
    /// variable or field chain, no dynamic index). It retains the caller place
    /// for handle passing/write-back; ordinary copied arguments and unsupported
    /// place shapes are `None`.
    Call {
        dest: Reg,
        func: FuncRef,
        /// Checker-selected error contract for this call, if it may raise.
        raises: Option<Ty>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        arg_places: Vec<Option<MirPlace>>,
        /// Retained caller places aligned with `kwargs`. Keyword binding is
        /// reordered by the call ABI, so the backend resolves these by the
        /// selected parameter name rather than by positional index.
        kwarg_places: Vec<Option<MirPlace>>,
        /// Concrete captured-owner effects performed transitively during this
        /// call. Static verification consumes these; execution erases them.
        capture_accesses: Vec<MirCaptureAccess>,
        /// Supplied compile-time arguments in source order. Each entry retains
        /// an optional keyword name and an optional value register; type
        /// arguments have no register. Origin arguments are semantically erased
        /// before this list. Consumers bind these entries to `ParamDecl` order
        /// before applying defaults.
        param_arg_regs: Vec<MirParamArg>,
    },
    /// A call through a runtime function value. Callable parameters use this
    /// instruction instead of treating the parameter name as a global symbol.
    CallIndirect {
        dest: Reg,
        callee: Reg,
        /// Exact checker-selected nominal `__call__` target, or a
        /// signature-qualified abstract target for a `def(...)` value. The VM
        /// consults this only when `callee` is a nominal callable struct.
        resolved: Option<String>,
        /// Checker-selected error contract of the callable value.
        raises: Option<Ty>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        /// The callable value's caller place, when it has one. This is needed
        /// for a callable struct whose `__call__` receiver is `mut`/`ref`.
        callee_place: Option<MirPlace>,
        /// Like `Call::arg_places`: the retained caller place for each
        /// positional argument selected for a `mut`/`ref` parameter.
        arg_places: Vec<Option<MirPlace>>,
        /// Like `Call::kwarg_places`, aligned with `kwargs`.
        kwarg_places: Vec<Option<MirPlace>>,
        capture_accesses: Vec<MirCaptureAccess>,
        /// Compile-time arguments supplied while invoking a generic callable
        /// value, in the same source-order representation as `Call`.
        param_arg_regs: Vec<MirParamArg>,
        /// The generic callable contract selected by checking. Contract-side
        /// defaults govern omitted arguments even when the concrete function
        /// value declares different defaults.
        param_decls: Vec<ParamDecl>,
        /// Fully instantiated checker contract for this invocation, when
        /// explicit compile-time arguments resolve dependent parameter/result
        /// types. Generic callable storage may remain symbolic; executable
        /// operand verification uses this concrete view.
        instantiated_contract: Option<Ty>,
        /// Complete checker-resolved generic arguments in declaration order.
        /// This semantic-only substitution witness lets verification validate
        /// `instantiated_contract` without inspecting the instructions which
        /// materialized compile-time argument registers.
        instantiated_args: Vec<TyArg>,
    },
    /// A method call `recv.method(args)`. `recv_place` is `Some` when the receiver
    /// is a writable place (a variable / field-index chain), so a `mut self` method
    /// can write the updated receiver back; `None`
    /// for a temporary receiver (a call result), on which only read-only methods
    /// are valid (the checker guarantees this).
    MethodCall {
        dest: Reg,
        recv: Reg,
        method: String,
        resolved: Option<String>,
        /// Checker-selected concrete or trait-requirement error contract.
        raises: Option<Ty>,
        /// Reference-return ABI retained independently from the destination
        /// value type. A value-returning method may itself produce a
        /// reference-valued element, so the verifier must not infer ABI from
        /// `reg_types[dest]`.
        reference_result: Option<crate::origin::RefTy>,
        /// Checker-proven adaptation from a concrete implementation ABI to the
        /// abstract result promised at this call site.
        result_adapter: Option<crate::checked::CheckedResultAdapter>,
        args: Vec<Reg>,
        kwargs: Vec<(String, Reg)>,
        recv_place: Option<MirPlace>,
        /// Like `Call::arg_places`: `arg_places[i]` is `Some` only for a
        /// checker-selected `mut`/`ref` ordinary argument with a supported
        /// caller place.
        arg_places: Vec<Option<MirPlace>>,
        /// Like `Call::kwarg_places`, aligned with `kwargs`.
        kwarg_places: Vec<Option<MirPlace>>,
        capture_accesses: Vec<MirCaptureAccess>,
        /// Compile-time arguments supplied by direct parameterized-method
        /// syntax, in source order. Callable/scalar value parameters retain a
        /// runtime register; type parameters occupy an erased slot.
        param_arg_regs: Vec<MirParamArg>,
        /// Checker-selected generic method contract. This prevents lowering
        /// from reconstructing a bound-method type from the source member.
        param_decls: Vec<ParamDecl>,
    },
    /// Move an initialized element from compiler-private `UnsafePointer`
    /// collection storage. The source slot becomes uninitialized, so subsequent
    /// reads/takes/destroys are invalid until an explicit store initializes it.
    PointerStorageTake {
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: Ty,
    },
    /// Destroy an initialized element in compiler-private `UnsafePointer`
    /// collection storage and mark its slot uninitialized.
    PointerStorageDestroy {
        dest: Reg,
        pointer: Reg,
        index: Reg,
        element: Ty,
    },
    /// Construct compiler-private inline possibly-uninitialized storage
    /// (`UnsafeMaybeUninit`'s field): uninitialized when `init` is absent,
    /// holding the moved payload otherwise.
    UninitStorage {
        dest: Reg,
        init: Option<Reg>,
    },
    /// Move the payload out of consumed inline uninit storage. Traps if the
    /// storage is uninitialized (upstream UB, deterministic here).
    UninitStorageTake {
        dest: Reg,
        storage: Reg,
        element: Ty,
    },
    /// Destroy the payload of consumed inline uninit storage (runs the
    /// element's destructor). Traps if the storage is uninitialized.
    UninitStorageDestroy {
        dest: Reg,
        storage: Reg,
        element: Ty,
    },
    /// Struct/field *read* `base.field` inside an rvalue (name-based; the backend
    /// resolves layout). Field/index *writes* go through `Store`/a `MirPlace`.
    GetField {
        dest: Reg,
        base: Reg,
        field: String,
    },
    /// Subscript *read* `base[index]` inside an rvalue. Nominal collection
    /// subscripts carry their complete checker-selected invocation in `call`.
    /// This includes the exact `__getitem__` target, effects, parameter access,
    /// compile-time arguments, captures, and any reference-return contract; the
    /// backend never re-derives overload resolution or borrowing semantics.
    Index {
        dest: Reg,
        base: Reg,
        index: Reg,
        /// Stable receiver storage retained by checking/lowering. Nominal
        /// `__getitem__` dispatch uses this for a `ref self` handle and for any
        /// reference returned into the caller's frame.
        base_place: Option<MirPlace>,
        index_place: Option<MirPlace>,
        call: Option<MirSubscriptCall>,
        intrinsic: Option<MirIntrinsicSubscript>,
    },
    /// Slice `object[lower:upper:step]` → a new value. Each bound is
    /// optional (absent = a direction-aware default).
    Slice {
        dest: Reg,
        object: Reg,
        kind: crate::types::SliceKind,
        lower: Option<Reg>,
        upper: Option<Reg>,
        step: Option<Reg>,
        object_place: Option<MirPlace>,
        arg_places: Vec<Option<MirPlace>>,
        call: Option<MirSubscriptCall>,
        intrinsic: Option<MirIntrinsicSubscript>,
    },
    /// `object[a, b:c]`: variadic `__getitem__` dispatch with every slice
    /// descriptor selected by the checker and constructed explicitly by the VM.
    MultiIndex {
        dest: Reg,
        object: Reg,
        args: Vec<MirSubscriptArg>,
        object_place: Option<MirPlace>,
        arg_places: Vec<Option<MirPlace>>,
        /// Keyword subscript actuals — index values (`s[byte=i]`) or slice
        /// descriptors (`s[byte=a:b]`) — bound to keyword-only `__getitem__`
        /// parameters through the checked contract's keyword sources,
        /// parallel to `kwarg_places`.
        kwargs: Vec<(String, MirSubscriptArg)>,
        kwarg_places: Vec<Option<MirPlace>>,
        call: Option<MirSubscriptCall>,
    },
    /// `object[a, b:c] = value`: checked `__setitem__` dispatch. The receiver
    /// place is retained so a `mut self` implementation is written back after
    /// the call. Variadic setitem methods receive `value` in their keyword-only
    /// slot; fixed-arity methods receive it as the last positional argument.
    MultiSet {
        receiver: Reg,
        receiver_place: Option<MirPlace>,
        args: Vec<MirSubscriptArg>,
        arg_places: Vec<Option<MirPlace>>,
        value: Reg,
        value_place: Option<MirPlace>,
        value_keyword: bool,
        call: MirSubscriptCall,
    },
    /// `place = src` — a write through a place (`p.x = e`, `xs[i] = e`, nested).
    Store {
        place: MirPlace,
        src: Reg,
    },
    /// Initialize reference-valued storage with a reference handle.  Ordinary
    /// `Store` on the same typed place writes through the established handle.
    StoreRef {
        place: MirPlace,
        reference: Reg,
    },
    /// Read a place into a register — for a read-modify-write (`place OP= e`),
    /// where the place (and its indices) must be evaluated exactly once.
    LoadPlace {
        dest: Reg,
        place: MirPlace,
    },
    /// Construct the compiler-private heterogeneous pack-storage primitive.
    /// Source Tuple/List/Set/Dict values are nominal structs and therefore use
    /// ordinary `Call`/`MethodCall` instructions instead of aggregate opcodes.
    MakeTuple {
        dest: Reg,
        elems: Vec<Reg>,
        /// Resolved element types when available. Typed `Tuple[T, ...](...)`
        /// construction uses these to materialize each argument precisely.
        element_types: Option<Vec<Ty>>,
    },
    /// Construct a tagged union. Alternative order determines the runtime tag.
    MakeVariant {
        dest: Reg,
        alternatives: Vec<Ty>,
        index: usize,
        value: Reg,
    },
    /// Test a tag selected during semantic checking.
    VariantIs {
        dest: Reg,
        variant: Reg,
        index: usize,
    },
    /// Extract the active alternative, trapping on a tag mismatch.
    VariantGet {
        dest: Reg,
        variant: Reg,
        index: usize,
    },
    /// Replace a writable variant and destroy its previous payload.
    VariantSet {
        dest: Reg,
        place: MirPlace,
        index: usize,
        value: Reg,
    },
    /// Move a payload out of an already-consumed variant value.
    VariantTake {
        dest: Reg,
        variant: Reg,
        index: usize,
        checked: bool,
    },
    /// In-place placement replacement (`set(init_with=…)`): invoke the
    /// zero-parameter `factory` callable value, store its result as the
    /// payload selected by `index`, and destroy the previous payload.
    VariantSetInitWith {
        dest: Reg,
        place: MirPlace,
        index: usize,
        factory: Reg,
    },
    /// Consuming teardown (`deinit_with`): destructure the moved variant
    /// value and invoke the single-parameter consuming `handler` callable
    /// value with the active payload. The handler covers exactly the
    /// alternative at `index`; any other runtime tag aborts.
    VariantDeinitWith {
        dest: Reg,
        variant: Reg,
        handler: Reg,
        index: usize,
    },
    /// Replace the active payload without destroying it, returning ownership of
    /// that previous payload to the caller.
    VariantReplace {
        dest: Reg,
        place: MirPlace,
        input_index: usize,
        output_index: usize,
        value: Reg,
        checked: bool,
    },
    /// SIMD construction `SIMD[DType.<dt>, width](elems)` (or a scalar-alias like
    /// `Int32(x)`). The element `dtype`/`width` — compile-time parameters the MIR
    /// is otherwise untyped about — are resolved here at lowering; `elems` are the
    /// lane values (exactly `width`, or one to splat).
    MakeSimd {
        dest: Reg,
        dtype: Dtype,
        width: usize,
        elems: Vec<Reg>,
    },
    /// Elementwise dtype conversion `v.cast[DType.<dt>]()`. The target
    /// `dtype`/`width` are checker-resolved compile-time parameters, like
    /// [`MirInstr::MakeSimd`]'s.
    SimdCast {
        dest: Reg,
        value: Reg,
        dtype: Dtype,
        width: usize,
    },
    /// Lane gather `v.shuffle[*mask]()`: result lane `i` is `value`'s lane
    /// `mask[i]`. The mask is a checker-resolved compile-time parameter —
    /// every index is within the receiver's width and the mask length is a
    /// valid SIMD width.
    SimdShuffle {
        dest: Reg,
        value: Reg,
        mask: Vec<usize>,
    },
    /// `raise <src>` — raise an error value. Propagates as an exceptional outcome
    /// (the VM unwinds to the nearest enclosing [`MirInstr::Try`] handler).
    Raise {
        src: Reg,
    },
    /// A structurally lowered `try`/`except`/`else`/`finally` region. Each
    /// sub-part is a self-contained mini-CFG (a
    /// `Vec<MirBlock>` with local block ids, entry = block 0) that **shares this
    /// function's register and variable space** — so it addresses the same slots.
    /// `handler` is `Some((error_var, body))` when there is an `except` clause (the
    /// optional slot binds the caught error). `cleanup` lists the body-local
    /// variables to drop when the body unwinds (the exceptional-edge cleanup).
    Try {
        body: Vec<MirBlock>,
        handler: Option<(Option<VarId>, Vec<MirBlock>)>,
        orelse: Option<Vec<MirBlock>>,
        finalbody: Option<Vec<MirBlock>>,
        cleanup: Vec<VarId>,
    },
    /// An ASAP destructor on a register (reserved for the future Op/assembler VM).
    Drop {
        reg: Reg,
    },
    /// Drop the value in a variable slot — spliced in by the Stage 7 liveness pass
    /// at a variable's last use (ASAP destruction). Runs the value's `__deinit__` (and
    /// its fields', in reverse order) and leaves the slot empty. A no-op for values
    /// without a destructor, so it never changes observable behaviour except when a
    /// struct defines `__deinit__`.
    DropVar {
        var: VarId,
    },
    /// Consume a variable without running implicit destruction. Explicit-destroy
    /// calls emit this immediately after the call, so a raising call leaves the
    /// source live for an `except` fallback while a successful call consumes it.
    ConsumeVar {
        var: VarId,
    },
    /// Consume one projected subobject after its named explicit destructor
    /// succeeds, leaving the rest of the aggregate available.
    ConsumePlace {
        place: MirPlace,
        marker: Reg,
    },
    /// A construct the MIR/backends don't lower yet (a `try` with its exceptional
    /// edges, a nested declaration). Kept as an explicit node — rather than a
    /// lowering-time `panic!` — so a backend can report a clean error instead of
    /// crashing on an otherwise-valid program.
    Unsupported(String),
    /// Iterator protocol: normalize `source` through its checker-selected nominal
    /// `__iter__()` implementation, producing the iterator in `dest`. When
    /// `source == dest` the iterable is normalized in place; when they differ,
    /// `source` retains the live iterable in its own slot (dropped after the loop)
    /// so a borrowing iterator does not clobber its only owner.
    GetIter {
        source: VarId,
        dest: VarId,
        mode: crate::IterationMode,
        prepare: Vec<String>,
    },
    /// Iterator protocol (`for` loops): read whether the iterator variable `iter`
    /// yields another element into `dest` (a `Bool`) — a pure read.
    HasNext {
        dest: Reg,
        iter: VarId,
        method: Option<String>,
    },
    /// Iterator protocol: bind the current element into `dest` and advance the
    /// iterator variable `iter` in place (a mutating read).
    Next {
        dest: Reg,
        iter: VarId,
        /// Exact checked nominal operation; absent only for compiler-private
        /// iterator storage.
        call: Option<crate::checked::CheckedIteratorCall>,
    },
    /// Invoke a typed-raising iterator `__next__`. `yielded` is true when
    /// `dest` contains an element and false when the call raises exactly the
    /// checked `exhaustion` type; other raised values propagate.
    TryNext {
        dest: Reg,
        yielded: Reg,
        iter: VarId,
        /// Exact selected target, reference/value ABI, and raising effect.
        call: crate::checked::CheckedIteratorCall,
        exhaustion: Ty,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCaptureMode {
    Reference,
    Copy,
    Move,
}

#[derive(Debug, Clone)]
pub struct MirClosureCapture {
    pub place: MirPlace,
    pub mode: MirCaptureMode,
}

/// A static access to owner storage performed by a callable environment while
/// executing a call. Origin paths deliberately retain abstract indices; the VM
/// never interprets this metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirCaptureAccess {
    pub root: VarId,
    pub path: Vec<crate::origin::OriginSeg>,
    pub access: crate::origin::CaptureAccess,
}

/// How a basic block hands off control. Block targets are indices into
/// `MirFunction::blocks`; values are registers.
#[derive(Debug, Clone)]
pub enum MirTerm {
    Jump(MirBlockId),
    Branch {
        cond: Reg,
        then_b: MirBlockId,
        else_b: MirBlockId,
    },
    Return(Option<Reg>),
    /// Return after evaluating `value`, carrying structured loop-owned cleanup
    /// out through any enclosing `try/finally` regions. The VM performs these
    /// drops only after every pending `finally` has run.
    ReturnWithCleanup {
        value: Option<Reg>,
        cleanup: Vec<VarId>,
    },
    /// Normal fall-through end of a `try` sub-region (see [`hir::Terminator::FallOff`]).
    /// The VM's region runner reads it as "completed normally". Never appears in a
    /// function body's blocks.
    FallOff,
    /// A `break`/`continue` inside a `try` region that targets an enclosing
    /// **function** loop: `target` is that loop's exit/header block in the
    /// *enclosing function*'s `blocks` (not the region's). The VM propagates it out
    /// as a `Flow::Jump(target)` — running each `finally` on the way — until the
    /// function driver jumps there. `cleanup` lists the region-body-local variables
    /// to drop when this escape edge is taken (filled by drop elaboration).
    EscapeJump {
        target: MirBlockId,
        cleanup: Vec<VarId>,
    },
}

#[derive(Debug, Clone)]
pub struct MirBlock {
    pub instrs: Vec<MirInstr>,
    pub term: MirTerm,
}

#[derive(Debug)]
pub struct MirFunction {
    pub blocks: Vec<MirBlock>,
    pub n_regs: u32,
    /// Number of variable slots (the interner size). A frame allocates this many
    /// var cells; `UseVar`/`DefVar` index into them.
    pub n_vars: usize,
    /// The name of each variable slot (`var_names[id]` is `VarId` `id`'s source
    /// name; synthetic `$…` names for compiler temporaries). For diagnostics — the
    /// ownership analysis names the offending variable.
    pub var_names: Vec<String>,
    /// Number of leading vars that are parameters (`vars[0..n_params]`), bound from
    /// the call's arguments in declaration order — the VM call ABI.
    pub n_params: usize,
    /// The checker-resolved type of each parameter (same order/length as the
    /// params), used when binding arguments. Empty for `__toplevel__`.
    pub param_types: Vec<Ty>,
    /// Whether each parameter is consuming (the callee takes ownership, so it drops
    /// the value — unlike a borrowed `read`/`mut` parameter). Same order as the
    /// params; the caller transfers with `^`, so its own drop is skipped.
    pub owned_params: Vec<bool>,
    /// Whether each parameter is a `deinit` (destructor/move-source) parameter.
    /// Same order/length as the params. Such a parameter is *consumed* at its
    /// drop point — its residual fields are destroyed but its whole-value
    /// `__deinit__` is skipped (its resources moved into the receiver), so drop
    /// elaboration lowers its teardown to `ConsumeVar` instead of `DropVar`.
    pub deinit_params: Vec<bool>,
    /// Whether each parameter is a `mut`/`ref` **reference** (its final value is
    /// written back to the caller). `self` (handled via a method's `recv_place`) is
    /// always `false` here. Same order as the params.
    pub ref_params: Vec<bool>,
    pub returns_reference: bool,
    /// Checked type of each variable slot, as far as lowering recorded one.
    /// Parameters, bindings, and synthetic locals are covered on the checked
    /// path; instruction typing and verification read slot types from here.
    pub var_tys: HashMap<VarId, Ty>,
    /// Checked return type. `None` only on unchecked compatibility paths;
    /// production lowering always records it (`Ty::None` for no return).
    pub ret_ty: Option<Ty>,
    /// Checked raising contract (`raises Never` records as nonraising).
    pub raises: bool,
    /// Declared error type when `raises` is true and the contract is typed.
    pub error_ty: Option<Ty>,
    pub spans: SpanTable,
    /// Resolved type of registers originating in checked expressions. Synthetic
    /// control-flow registers are filled by instruction typing before verification.
    pub reg_types: HashMap<u32, Ty>,
}

/// Maps each generated register to its source span and (if it names one) the
/// origin variable — so borrow-checker diagnostics can point at real code.
#[derive(Debug, Default)]
pub struct SpanTable(pub HashMap<u32 /*reg*/, (SourceSpan, Option<VarId>)>);
