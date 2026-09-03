//! Checked semantic handoff between the frontend and lowering.

use mojito_ast::ast::Stmt;
use mojito_ast::ast::{Expr, ExprKind, PrefixOp};
use mojito_common::token::{SourceSpan, Span};
use mojito_types::types::Ty;
use std::collections::HashMap;
use std::collections::HashSet;

/// Stable identity within one checked program. Unlike a source span this remains
/// unique for synthesized nodes and for multiple semantic nodes at one location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckedNodeId(pub u32);

/// How an expression participates in evaluation. New categories can be added
/// without changing source syntax; in particular, future patterns and suspended
/// computations can refer to nodes rather than impersonating ordinary values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueCategory {
    Value,
    Place,
    Type,
    CompileTime,
}

/// Extensible control/effect summary. Suspension is deliberately independent
/// from raising: future coroutines, generators, and delimited continuations can
/// introduce resume edges without being encoded as exceptions or calls.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectFacts {
    pub raises: Option<Ty>,
    pub may_suspend: bool,
    pub diverges: bool,
}

/// One source argument after ordinary call binding selected a concrete method
/// parameter.  Keeping the source slot and effective convention together lets
/// HIR/MIR retain caller storage without rebuilding keyword/default binding or
/// origin-dependent `ref` mutability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCallArgument {
    pub source: CheckedCallArgumentSource,
    pub parameter_ty: Ty,
    /// The declared ABI requires a caller place even when origin solving turns
    /// an immutable `ref` into an effective read for conflict analysis.
    pub requires_place: bool,
    /// Effective access after parametric reference mutability is solved.
    pub convention: Option<mojito_ast::ast::ArgConvention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedCallArgumentSource {
    Positional(usize),
    Keyword(usize),
    Default,
}

/// One source-ordered compile-time method argument. Type arguments occupy a
/// declaration slot with no runtime value; value arguments name the checked
/// expression occurrence whose already-evaluated register lowering must reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCallParameterArgument {
    pub name: Option<String>,
    pub value_source: Option<SourceSpan>,
}

/// One checker-selected adaptation of an already-evaluated call argument.
/// These are deliberately call-local: augmented subscripts may send the same
/// source value through different getter and setter parameter contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedCallValueAdjustment {
    ResolveCallable { target: String },
    ImplicitConversion { target: String },
    IndexNormalization { target: String },
    MaterializeLiteral { target: Box<Ty> },
}

/// Call-boundary facts for one supplied argument. `value_source` identifies the
/// source occurrence which MIR evaluates once. Lowering must derive a separate
/// per-call register by applying `adjustments`, retain the original caller place
/// when the corresponding [`CheckedCallArgument`] requires it, and emit the
/// invalidations immediately at this call rather than at source evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCallArgumentBoundary {
    pub source: CheckedCallArgumentSource,
    pub value_source: SourceSpan,
    pub adjustments: Vec<CheckedCallValueAdjustment>,
    pub invalidations: Vec<InteriorInvalidation>,
}

/// Effects and value adaptations whose semantic location is one selected call.
/// Keeping this on the contract makes two calls over shared syntax independent:
/// the source operands are still evaluated once, but conversions and mutation
/// effects occur according to each callee's own signature.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckedCallBoundary {
    pub arguments: Vec<CheckedCallArgumentBoundary>,
    /// Receiver/call-site generation changes, emitted at the call boundary.
    pub invalidations: Vec<InteriorInvalidation>,
}

/// A checker-proven adaptation applied at an abstract call boundary.  This is
/// deliberately not inferred from the runtime value: a value-returning method
/// may itself produce a reference-valued element.  Execution consults the
/// retargeted declaration's ABI and performs this adapter only when that
/// declaration returns through the reference ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedResultAdapter {
    /// Mojo permits `Iterator.__next__ -> Element` to be implemented as
    /// `__next__ -> ref[o] Element` when `Element: Copyable`.  An abstract call
    /// observes the promised value result, so it must read and lifecycle-copy
    /// the concrete reference return.
    CopyIteratorReference,
}

/// Canonical checker-to-lowering contract for one selected method-like call.
/// Nominal subscripts use this exact record rather than a reduced parallel
/// resolver.  Future syntactic call forms can share it without teaching MIR
/// overload resolution, origin solving, or effect inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCallContract {
    pub target: String,
    pub raises: Option<Ty>,
    /// Executable result type of the selected call. Reference-returning calls
    /// carry the full instantiated `ref` type here; surface expression typing
    /// still exposes the referent as a place-like value.
    pub result_ty: Ty,
    /// Explicit adaptation needed after retargeting an abstract call to its
    /// concrete implementation. Concrete calls never carry an adapter.
    pub result_adapter: Option<CheckedResultAdapter>,
    pub receiver_requires_place: bool,
    /// Effective receiver access; `receiver_requires_place` independently
    /// retains the declared ABI handle requirement.
    pub receiver_convention: Option<mojito_ast::ast::ArgConvention>,
    pub arguments: Vec<CheckedCallArgument>,
    pub captures: Vec<mojito_types::origin::CaptureOrigin>,
    pub reference_result: Option<mojito_types::origin::RefTy>,
    pub parameter_arguments: Vec<CheckedCallParameterArgument>,
    pub param_decls: Vec<mojito_types::types::ParamDecl>,
    pub boundary: CheckedCallBoundary,
}

/// The operation selected for `receiver[index] OP= rhs`. A value-returning
/// getter is followed by a setter, while a mutable-reference getter writes the
/// computed result directly through that handle and therefore has no setter.
/// In either case the receiver and indices are evaluated only once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedAugmentedSubscript {
    pub getter: CheckedCallContract,
    /// Absent exactly when `getter.reference_result` is mutable and lowering
    /// must finish with `WriteRef` instead of invoking `__setitem__`.
    pub setter: Option<CheckedCallContract>,
    /// In-place dunder selected on the element type when it is a user-defined
    /// value (`c[i] += v` → `element.__iadd__(v)`). Lowering materializes the
    /// element into a mutable temporary, applies this `mut self` call, and sends
    /// the result through the setter or reference handle. `None` for a native
    /// scalar element, whose operator step stays a `BinOp` read-modify-write.
    pub inplace: Option<CheckedCallContract>,
    /// Ordinary value type read from the getter (the referent for `ref` results).
    pub operand_ty: Ty,
    pub result_ty: Ty,
    /// Synthetic checker-only source used to bind the computed result into the
    /// selected setter, including inferred compile-time value arguments.
    pub value_source: Option<SourceSpan>,
}

/// The two-call plan for a bare element call `value[i](args)` (also member
/// bases and multi-index brackets): subscript the indexable runtime value,
/// then dispatch the element as a callable. The getter contract lives here
/// rather than in `SelectedCall` so node-level consumers (reference-result
/// diversion, `resolved_callable`, `checked_raises`) see the element call,
/// not the subscript read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedElementInvocation {
    /// The selected `__getitem__` contract for the subscript read.
    pub getter: CheckedCallContract,
    /// The element's declared callable contract governing the invocation.
    pub callable: Ty,
    /// Exact nominal `Struct.__call__` target for a callable-struct element.
    pub target: Option<String>,
    /// The `__call__` contract's checked error type; the getter's raising
    /// effect stays on its own contract.
    pub raises: Option<Ty>,
}

/// Ownership mode selected for a checked iteration expression.  This is a
/// semantic distinction, not a runtime guess: owned iteration must dispatch to
/// an `__iter__(var self)` implementation, while ordinary iteration uses a
/// borrowed receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterationMode {
    Borrowed,
    Owned,
}

/// Checker-proven adaptation from one raw `__next__` result into the loop
/// target's storage convention.  Source ownership is deliberately absent:
/// [`IterationMode`] selects borrowed versus consuming `__iter__`, while this
/// enum describes only what the target spelling does with each yielded item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterationBindingAction {
    /// Move a freshly yielded value into an explicit `var` target that owns it
    /// and may transfer it onward.
    MoveValue,
    /// Bind a freshly yielded rvalue into an immutable or `ref` target's own
    /// per-iteration storage, dropped at each iteration's end rather than moved
    /// onward. The value is stored directly (not aliased) because it has no
    /// external referent to borrow.
    BorrowValue,
    /// Read and lifecycle-copy a yielded reference into an owned `var` target.
    CopyReference,
    /// Preserve a yielded reference, possibly attenuating it to immutable for
    /// an implicit loop target.
    BorrowReference,
}

/// Complete checked plan for one loop/comprehension target. `yielded_ty` is the
/// exact result type of `__next__`; `binding_ty` is the user-visible storage
/// type. `mutable` is the loop variable's declared mutability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedIterationBinding {
    pub mode: mojito_ast::ast::LoopBindingMode,
    pub action: IterationBindingAction,
    pub yielded_ty: Ty,
    pub binding_ty: Ty,
    pub mutable: bool,
}

/// Exact checked contract for one iterator `__next__` call.  The result type is
/// the type produced by the call itself, so a reference-yielding iterator keeps
/// its `Ty::Ref` handle type rather than collapsing to the referent.  The
/// parallel reference fact carries the executable origin/capability contract
/// needed by lowering, while `raises` records the selected exhaustion effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedIteratorCall {
    pub target: String,
    pub result_ty: Ty,
    pub reference_result: Option<mojito_types::origin::RefTy>,
    pub raises: Option<Ty>,
    pub result_adapter: Option<CheckedResultAdapter>,
}

/// One resolved generic free-function application at a call site: the source
/// callee name and the compile-time arguments, in declaration order, exactly
/// as `resolve_use_params` returned them.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericInstantiation {
    pub callee: String,
    pub arguments: Vec<mojito_types::types::TyArg>,
}

/// Fully resolved iterator protocol retained across the checked boundary.
/// `prepare` contains the exact `__iter__` symbols needed to normalize a user
/// iterable; builtin ranges/collections leave it empty.  User iterators carry
/// exact iterator-operation symbols so the VM never performs name/arity
/// overload reconstruction. `exhaustion` is the checked typed error caught by
/// a raising `__next__`; when absent, `has_next` selects the legacy bounded
/// `__len__` protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IterationProtocol {
    pub mode: IterationMode,
    /// Target convention and result-category adaptation, filled once the
    /// checker has combined this source protocol with its loop target. Boxed to
    /// keep `SemanticAdjustment::Iterate` compact.
    pub binding: Option<Box<CheckedIterationBinding>>,
    /// Concrete borrowed collection storage retained by this iteration.  The
    /// checker records the canonical owner identity and interior generation;
    /// HIR/MIR use it both to avoid an accidental value copy and to keep the
    /// owner live while rejecting a use of an iterator after structural
    /// invalidation.  Generic `Iterable` remains `None`: its abstract
    /// `__iterator_dispatch` contract yields `Element` values, so a `ref`
    /// loop target over a generic bound is rejected outright.
    pub borrowed_origin: Option<mojito_types::origin::OriginPlace>,
    /// The declared interior projection of the yielded reference relative to
    /// the source origin (`__next__ -> ref[o._get_owned_interior["element"]]`
    /// leaves `[Interior("element")]`). Appended to the attached borrowed
    /// origin so the source loan carries the iterator's declared granularity;
    /// empty when the iterator declares no projection (whole-place loan) or
    /// the projection already resolved onto a concrete origin.
    pub yield_interior: Vec<mojito_types::origin::OriginSeg>,
    pub prepare: Vec<String>,
    pub has_next: Option<String>,
    pub next: Option<Box<CheckedIteratorCall>>,
    pub exhaustion: Option<Ty>,
}

/// A checked mutation of an origin that owns interior storage. `except` names
/// the reference through which the mutation occurs, when there is one: writing
/// through `element` keeps that reference's generation valid while still
/// invalidating interiors nested below the element. `include_base_generation`
/// distinguishes defining a fresh named owned-interior generation (which
/// replaces that exact generation) from an ordinary write through its storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteriorInvalidation {
    pub base: mojito_types::origin::OriginPlace,
    pub except: Option<mojito_types::origin::OwnerId>,
    pub include_base_generation: bool,
}

/// Checker decisions which lowering must apply explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SemanticAdjustment {
    ResolveCallable(String),
    /// Construct the concrete runtime type bound to a checked type parameter.
    ConstructTypeParam {
        param: String,
    },
    /// A supplied compile-time type argument binding a runtime-constructible
    /// type parameter (`hash[Fnv1a](x)`): MIR reifies it as the bound
    /// struct's name so `ConstructTypeParam` dispatch constructs the supplied
    /// type rather than the declaration default.
    ReifyTypeArgument {
        name: String,
    },
    /// Complete selected method/subscript contract.  `ResolveCallable` remains
    /// as a compatibility projection while downstream consumers migrate; this
    /// record is authoritative whenever both are present.
    SelectedCall(Box<CheckedCallContract>),
    /// Complete two-call contract for an augmented nominal subscript. The
    /// coexisting `SelectedCall` is the setter for consumers that only inspect
    /// assignment syntax; MIR uses this record as the authoritative pair.
    AugmentedSubscript(Box<CheckedAugmentedSubscript>),
    /// In-place dunder selected for `place OP= rhs` on a user-defined value
    /// (`counter += 2` → `counter.__iadd__(2)`). The contract carries the
    /// `mut self` receiver, argument conventions/conversions, effects, and raising
    /// type; MIR emits a receiver-committing method call instead of a `BinOp`
    /// read-modify-write. Absent for native scalar operands, which keep the
    /// builtin operator path.
    AugmentedInPlace(Box<CheckedCallContract>),
    /// Normalize an `Indexer` expression through the exact checked
    /// `__mlir_index__() -> Int` method before a concrete indexing operation.
    /// MIR evaluates the source expression once and emits this call explicitly;
    /// backends receive an ordinary `Int` index and never guess by runtime type.
    IndexNormalization {
        target: String,
    },
    /// Direct invocation of a parameterized method written
    /// `receiver.method[parameters](arguments)`. The source parser represents
    /// this as `Invoke(Member(..))`; retaining the selected generic declaration
    /// here lets HIR/MIR lower an ordinary method call without treating a bound
    /// method as an escapable first-class value.
    ParameterizedMethodCall {
        param_decls: Vec<mojito_types::types::ParamDecl>,
    },
    /// A method-call-shaped expression (`holder.callback(1)`) dispatching
    /// through a callable-typed FIELD rather than a declared method. MIR
    /// loads the field value and emits an indirect call whose callee place
    /// is the field's, so a stored closure environment stays reachable.
    FieldInvocation {
        /// The field's checked callable type.
        callable: Ty,
    },
    /// A call-shaped expression (`objs[0](3)`, `a.b[i](x)`, `grid[i, j](x)`)
    /// whose brackets are a runtime subscript of an indexable value rather
    /// than compile-time parameter application. MIR reads the element through
    /// the recorded getter contract and emits an indirect call on the result.
    ElementInvocation(Box<CheckedElementInvocation>),
    /// Monomorphic callable contract selected after applying explicit
    /// compile-time arguments to a generic callable value. Indirect-call MIR
    /// retains this typed fact so verification never has to recover a value
    /// parameter by scanning preceding constant instructions.
    InstantiatedCallableContract {
        contract: Ty,
        /// Complete checker-resolved generic arguments in declaration order,
        /// including defaults. These are the substitution witness for the
        /// monomorphic contract; executable MIR must not rediscover them from
        /// the instructions which happened to materialize parameter registers.
        arguments: Vec<mojito_types::types::TyArg>,
    },
    ImplicitConversion(String),
    /// Materialize an exact, compile-time-only numeric literal expression into
    /// its checked runtime scalar type.  Keeping this distinct from a user
    /// `@implicit` constructor preserves the language boundary: literal
    /// arithmetic is unbounded, while the resulting scalar has fixed-width
    /// runtime semantics.
    MaterializeLiteral(Ty),
    BorrowShared,
    BorrowMutable,
    /// Constructor arguments bound to origin-solving `ref` parameters: the
    /// constructed aggregate borrows each listed argument's place, so the
    /// binding's loans keep the source alive (a view constructor's whole
    /// contract). Entries are (positional argument index, loan mutability).
    BorrowRefArguments {
        arguments: Vec<(usize, bool)>,
    },
    /// A temporary (non-place) expression bound to a `ref [origin]` parameter
    /// or `ref` binding: the checker materializes it as an anonymous owned
    /// binding with this identity, and lowering stores the value in a hidden
    /// frame slot registered under the same owner so the borrow has a real
    /// place (upstream's temporary lifetime extension — the temporary lives as
    /// long as its borrower).
    MaterializeBorrowSource {
        owner: mojito_types::origin::OwnerId,
    },
    /// An `@implicit` conversion whose selected constructor borrows its single
    /// argument through a `ref [origin]` parameter (a view construction): the
    /// conversion result borrows the source expression's place, refining the
    /// temporary's origin to its source (temporary-origin inference). Pairs
    /// with the `ImplicitConversion` fact at the same site and mirrors the
    /// explicit construction path's `BorrowRefArguments` loan.
    BorrowConversionSource {
        mutable: bool,
    },
    /// A subscript result that is itself a borrowed view (a Span
    /// sub-slice): the result binding inherits its receiver's loans.
    BorrowViewResult,
    /// A place expression occurs in an ownership-producing context (binding,
    /// assignment, return, or another consuming slot) and was proven Copyable
    /// in the checker's active generic environment. MIR must materialize an
    /// independent value rather than leave a projected-place load as a shallow
    /// handle alias.
    CopyPlaceValue,
    /// A call produced a reference handle, while ordinary expression typing
    /// reads through that handle to the referent. Reference-valued contexts
    /// retain the handle; MIR emits an explicit `ReadRef` everywhere else.
    ReferenceResult {
        reference: mojito_types::origin::RefTy,
    },
    /// Exact per-element extraction selected for one tuple-unpacking RHS. The
    /// unpack syntax has no source `Index` child nodes, so checking carries the
    /// synthetic result type, concrete accessor, and reference-return contract
    /// explicitly instead of asking MIR to reconstruct them from a nominal
    /// type name.
    TupleUnpack {
        elements: Vec<CheckedTupleUnpackElement>,
    },
    /// This actual argument must retain its caller place through the call
    /// because the selected parameter uses `mut`/`ref` handle or write-back
    /// semantics. Ordinary copied arguments intentionally omit this marker so
    /// ASAP destruction may still occur after argument evaluation.
    RetainCallPlace,
    /// The read-convention sibling of `RetainCallPlace`: this actual argument
    /// is a shared read of a place no exclusive access overlaps within the
    /// call, so lowering may bind it as a shallow place read instead of the
    /// implicit `__copyinit__` copy. Arguments overlapping a `mut`/`ref`/`^`
    /// access in the same call intentionally omit this marker — their implicit
    /// copy is what makes the call's exclusivity check pass.
    BorrowReadArgument,
    /// Concrete owner accesses performed by a callable environment during this
    /// call. The checker derives these from the selected callable and any
    /// non-escaping callable arguments; MIR translates stable owner identities
    /// to function-local slots and loan analysis treats them as call effects.
    CallableCaptureAccesses(Vec<mojito_types::origin::CaptureOrigin>),
    /// This checked compile-time argument is semantic-only. MIR retains its
    /// source position for declaration alignment but must not evaluate it into
    /// a runtime register (notably an explicitly supplied `Origin`).
    EraseCompileTimeArgument,
    /// A consuming method receiver is a source place, but the selected type is
    /// `ImplicitlyCopyable`, so the call consumes a copied value rather than
    /// tombstoning the caller's place. This coexists with parameterized-method
    /// metadata and is consumed directly by MIR lowering.
    ImplicitlyCopyConsumingReceiver,
    Move,
    ExplicitDestroy,
    Iterate(IterationProtocol),
    ConstructSimd {
        dtype: mojito_ast::ast::Dtype,
        width: i64,
    },
    /// `size_of[T]()` resolved to one concrete checked type. MIR retains the
    /// type so every executable backend consults the shared native layout.
    SizeOf {
        ty: Ty,
    },
    /// `v.cast[DType.target]()` — elementwise dtype conversion of a SIMD
    /// value. The target dtype and lane width are resolved at checking; MIR
    /// carries them so the VM never derives semantics from a runtime value.
    SimdCast {
        dtype: mojito_ast::ast::Dtype,
        width: i64,
    },
    /// `v.shuffle[*mask]()` — lane gather by checker-resolved compile-time
    /// indices, each within the receiver's width; the result takes the
    /// mask's (power-of-two) width.
    SimdShuffle {
        mask: Vec<usize>,
    },
    /// Construct the selected alternative of a checked `Variant` type.
    ConstructVariant {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// A collection display/comprehension resolved to a nominal constructor and
    /// insertion protocol. MIR lowers this as ordinary construction plus exact
    /// method calls; it must not reconstruct a native collection from syntax.
    ConstructCollection {
        target: Ty,
        insert: Option<String>,
    },
    /// A fixed-size list display resolved to `Array`'s variadic literal
    /// constructor. MIR lowers this as one nominal constructor call carrying
    /// the element registers and the concrete `length` parameter argument;
    /// `constructor` is the exact lowered overload symbol.
    ConstructArrayLiteral {
        target: Ty,
        constructor: String,
    },
    /// Test the active runtime tag (`value.isa[T]()`).
    VariantIs {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// Compile-time membership query (`value.is_type_supported[T]()`).
    VariantTypeSupported {
        supported: bool,
    },
    /// Checked typed projection (`value[T]`).
    VariantProject {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// Replace the active alternative (`value.set[T](new_value)`).
    VariantSet {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// Consume a variant and move out the selected payload. `checked` controls
    /// whether execution validates the active tag (`take` versus `unsafe_take`).
    VariantTake {
        alternatives: Vec<Ty>,
        index: usize,
        checked: bool,
    },
    /// Atomically install `input_index` and move out `output_index`.
    VariantReplace {
        alternatives: Vec<Ty>,
        input_index: usize,
        output_index: usize,
        checked: bool,
    },
    /// Construct an origin-bearing pointer to existing checked storage
    /// (`UnsafePointer(to=place)`). The checked pointer type retains the
    /// inferred place origin; execution represents the value as a frame/slot
    /// handle and erases the origin.
    PointerToPlace {
        mutable: bool,
    },
    /// Move one initialized element out of compiler-private raw collection
    /// storage, leaving that heap slot uninitialized. Only bundled collection
    /// sources may acquire this adjustment.
    PointerStorageTake {
        element: Ty,
    },
    /// Destroy one initialized element in compiler-private raw collection
    /// storage, leaving that heap slot uninitialized. Only bundled collection
    /// sources may acquire this adjustment.
    PointerStorageDestroy {
        element: Ty,
    },
    /// `pointer.unsafe_offset(i)`: provenance-preserving element arithmetic,
    /// lowered as the ordinary pointer `+` operation.
    PointerOffset,
    /// `pointer.unsafe_origin_cast[...]()`: a checked provenance rebind. The runtime
    /// value is unchanged, so lowering forwards the receiver register; the
    /// resolved target origin is retained so aggregate-origin bookkeeping can
    /// carry the rebound loan without re-resolving the parameter argument.
    PointerOriginCast {
        origin: mojito_types::origin::PointerOrigin,
    },
    /// Construct compiler-private inline uninit storage (`__UninitStorage[T]()`
    /// or `__UninitStorage[T](value^)`). Only bundled standard-library sources
    /// may acquire this adjustment.
    UninitStorageMake {
        element: Ty,
        init: bool,
    },
    /// `storage.unsafe_write(value)`: initialize-or-overwrite the inline
    /// payload without destroying a previous value (it leaks by design).
    UninitStorageWrite {
        element: Ty,
    },
    /// `variant.set[T](init_with=factory)`: in-place placement replacement —
    /// the zero-parameter factory's result becomes the new payload and the
    /// previous payload is destroyed (all alternatives checked `Deinitable`).
    VariantSetInitWith {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// `variant.deinit_with(handler)`: consume the variant, handing the
    /// active payload to the single-parameter consuming handler. The handler
    /// is monomorphic for exactly one alternative (`index`); a runtime tag
    /// mismatch aborts, matching upstream's contract.
    VariantDeinitWith {
        alternatives: Vec<Ty>,
        index: usize,
    },
    /// `storage^.take()`: move the payload out of consumed inline uninit
    /// storage; traps at the VM if the storage is uninitialized.
    UninitStorageTake {
        element: Ty,
    },
    /// `storage^.destroy()`: run the payload's destructor and consume the
    /// inline storage; traps at the VM if the storage is uninitialized.
    UninitStorageDestroy {
        element: Ty,
    },
    /// `pointer.unsafe_write(value)` / `unsafe_write(copy=v)`: initialize the
    /// pointee at offset 0 with a moved (or copied) value, lowered onto the
    /// same store family as `pointer[] = value`.
    PointerWrite {
        element: Ty,
        copy: bool,
    },
    /// Descriptor types selected for a subscript's arguments. `None` denotes an
    /// ordinary index; `Some` denotes a source slice and records whether overload
    /// selection chose the contiguous, strided, or general Slice protocol.
    SliceDescriptors {
        descriptors: Vec<Option<mojito_types::types::SliceKind>>,
        /// A variadic `__setitem__` receives the assignment value through its
        /// required keyword-only `value` slot. Reads and fixed-arity writes leave
        /// this false.
        set_value_keyword: bool,
    },
    /// This place expression produces a reference into a named storage region
    /// owned by a container. The full stable path is a checker fact; MIR must
    /// not reconstruct it from source indexing syntax.
    InteriorReference {
        origin: mojito_types::origin::OriginPlace,
    },
    /// Mutating this expression may invalidate existing interior-reference
    /// generations rooted below one or more checked base origins.
    InvalidateInteriors {
        invalidations: Vec<InteriorInvalidation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedTupleUnpackElement {
    pub ty: Ty,
    pub accessor: Option<String>,
    pub reference: Option<mojito_types::origin::RefTy>,
}

/// One expression in the typed semantic arena. `syntax` is retained for
/// diagnostics and incremental migration only; identity, type, category, edges,
/// and adjustments no longer have to be reconstructed from it.
#[derive(Debug, Clone)]
pub struct CheckedExpr {
    pub id: CheckedNodeId,
    pub syntax: Expr,
    pub ty: Option<Ty>,
    /// Type physically stored at this place. For a reference-valued field this
    /// is `Ty::Ref`, while `ty` is the referent produced by an ordinary read.
    pub place_ty: Option<Ty>,
    /// Resolved destination type when this expression initializes an annotated
    /// or inferred binding. Lowering must not re-resolve its source annotation.
    pub binding_ty: Option<Ty>,
    pub category: ValueCategory,
    pub binding: Option<mojito_types::origin::OwnerId>,
    /// Checker-resolved base type name for a `$contextual` leading-dot
    /// sentinel at this node; HIR substitutes it during syntax renaming.
    pub contextual_base: Option<String>,
    pub effects: EffectFacts,
    pub adjustments: Vec<SemanticAdjustment>,
    pub children: Vec<CheckedNodeId>,
    /// Stable identities and storage types for generator binders introduced by
    /// a collection comprehension. Entries are in source `for`-clause order.
    /// Keeping these on the checked node prevents MIR from accidentally
    /// interning a binder by its spelling and aliasing an outer local.
    pub comprehension_bindings: Vec<CheckedComprehensionBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedComprehensionBinding {
    pub name: String,
    pub owner: mojito_types::origin::OwnerId,
    pub ty: Ty,
    pub plan: CheckedIterationBinding,
    /// Whether this binding's storage is droppable in the checked constraint
    /// environment at its introduction site. Conditional generic conformances
    /// cannot be reconstructed from the nominal type after checking.
    pub deinitable: bool,
}

pub use mojito_types::types::CheckedDeclId;

/// Declaration identity is independent of declaration spelling. Future classes,
/// pattern binders, coroutine state declarations, and generated declarations can
/// add variants without changing consumers of the common metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CheckedDeclKind {
    Function,
    Struct,
    Trait,
    Binding,
    CompileTime,
}

#[derive(Debug, Clone)]
pub struct CheckedDeclaration {
    pub id: CheckedDeclId,
    pub kind: CheckedDeclKind,
    pub name: String,
    pub location: SourceSpan,
    /// Stable value-binding identity for declarations which introduce a runtime
    /// name. This is independent of spelling and therefore survives shadowing.
    pub binding: Option<mojito_types::origin::OwnerId>,
    /// Explicit closure captures resolved in the declaration's enclosing scope.
    /// Default captures are discovered from checked expression bindings.
    pub captures: Vec<CheckedCapture>,
    pub ty: Option<Ty>,
    pub children: Vec<CheckedDeclId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCapture {
    pub name: String,
    pub binding: mojito_types::origin::OwnerId,
    /// Exact storage type at the captured binding. This is retained even when
    /// the capture is explicit but unused by the closure body, so forwarding
    /// through an intermediate lifted function remains fully typed.
    pub ty: Ty,
    pub kind: mojito_ast::ast::CaptureKind,
    /// Canonical outer-owner effects retained by this environment entry. Owned
    /// copy/move captures contribute only loans already stored in their value;
    /// read/ref/mut captures also contribute their source place.
    pub origins: Vec<mojito_types::origin::CaptureOrigin>,
}

/// A successfully checked program plus semantic facts that downstream phases
/// previously recomputed from AST syntax or checker-private side tables.
#[derive(Debug, Clone)]
pub struct CheckedProgram {
    statements: Vec<Stmt>,
    compatibility_overload_targets: HashMap<SourceSpan, String>,
    generic_instantiations: HashMap<SourceSpan, GenericInstantiation>,
    /// Caller-substituted loan transfers per call occurrence, keyed by the
    /// call expression's span; MIR lowering installs the implied loans.
    call_transfers: HashMap<SourceSpan, Vec<CheckedCallTransfer>>,
    compatibility_implicit_conversions: HashMap<SourceSpan, String>,
    checked_types: HashMap<AnnotationSite, Ty>,
    generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    expressions: Vec<CheckedExpr>,
    expression_index: HashMap<SourceSpan, Vec<CheckedNodeId>>,
    declarations: Vec<CheckedDeclaration>,
    explicit_destroy_types: HashMap<String, ExplicitDestroyInfo>,
    declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
}

#[derive(Debug, Clone)]
pub struct ExplicitDestroyInfo {
    pub message: String,
    pub destructors: HashMap<String, bool>,
    /// Direct fields whose types carry their own explicit-destroy obligation.
    pub fields: HashMap<String, String>,
}

/// The declaration-owned location of a source annotation. Unlike `SourceType`
/// syntax itself, this identity preserves the scope in which syntax such as a
/// bare `T` was resolved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnnotationSite {
    FunctionParam {
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
        param: usize,
    },
    /// Struct-owned sites are identified by the struct's unique name, not its
    /// span: compile-time struct specializations share their template's span
    /// (correct provenance), so spans cannot distinguish the declarations.
    StructField {
        module: Option<String>,
        declaration: String,
        field: usize,
    },
    MethodParam {
        module: Option<String>,
        declaration: String,
        method: usize,
        param: usize,
    },
    /// The checked return type of a free function declaration.
    FunctionReturn {
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    /// The checked callable type of a free function declaration — the exact
    /// `Func`/`GenericFunc` type the checker binds the name to. Lowering uses
    /// it to type closure values without reconstructing signatures.
    FunctionType {
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    /// The checked return type of a struct method declaration.
    MethodReturn {
        module: Option<String>,
        declaration: String,
        method: usize,
    },
}

/// Declaration-owned identity for a checked compile-time parameter list.
/// Runtime reification consumes these already-resolved declarations instead of
/// reclassifying source bounds in MIR or the VM.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericSite {
    Function {
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    Struct {
        module: Option<String>,
        declaration: String,
    },
    Method {
        module: Option<String>,
        declaration: String,
        method: usize,
    },
}

pub use mojito_types::types::{TransferEffect, TransferSet};

/// A higher-order transfer residue: the callable's body calls through one of
/// its own callable parameters, whose transfer effects are unknowable in the
/// body (a `def(...)` annotation cannot spell effects). Each call site —
/// which knows the concrete callable — translates that callable's effects
/// through the recorded argument mapping into effects of THIS callable and
/// replays them against its own actuals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallThroughEffect {
    pub callee: CallThroughCallee,
    /// Signature abstraction of each inner-call argument slot.
    pub args: Vec<CallThroughArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallThroughCallee {
    /// An ordinary runtime parameter with a function type (its slot index).
    RuntimeParam(usize),
    /// A compile-time callable value parameter (its declaration name).
    ValueParam(String),
}

/// How one inner-call actual maps to the enclosing signature.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallThroughArg {
    /// The actual's own place, when rooted at the enclosing signature.
    pub place: Option<mojito_types::origin::SigOrigin>,
    /// Origins carried by the actual that abstract to the signature.
    pub carried: Vec<mojito_types::origin::SigOrigin>,
    /// Whether the actual's place or carried loans root at frame-local
    /// storage — an escape if the inner callee stores them outward.
    pub local: bool,
}

/// One caller-substituted transfer at a call site: which actual receives
/// loans rooted at which caller origins. MIR lowering installs the loans.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedCallTransfer {
    pub dest: CheckedTransferDest,
    /// Interior path below the destination actual's root that receives the
    /// loans (empty = the whole root). Lowering turns a non-empty path into
    /// the `EstablishLoans` destination domain.
    pub dest_path: Vec<mojito_types::origin::OriginSeg>,
    pub sources: Vec<mojito_types::origin::Origin>,
    pub mutable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedTransferDest {
    Receiver,
    Argument(usize),
    /// A concrete captured owner (a `Bound` destination): lowering resolves
    /// it through its owner-variable map, skipping owners that live in an
    /// ancestor frame — the verbatim-propagated effect installs those where
    /// the storage lives.
    Owner(mojito_types::origin::OwnerId),
}

/// the declaration returns a reference. Recorded per callable so lowering
/// never re-reads source `raises`/return annotations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationEffect {
    pub raises: bool,
    pub error: Option<Ty>,
    pub returns_reference: bool,
}

#[derive(Debug, Clone)]
/// Literal value retained after semantic checking for declaration metadata.
pub enum CheckedConst {
    Int(mojito_common::literal::IntLiteral),
    Float(mojito_common::literal::FloatLiteral),
    Bool(bool),
    String(String),
    None,
    /// A default that is an `@implicit` conversion of `arg`: the omitted-arg
    /// slot materializes by running the converting constructor `target` on the
    /// folded literal (e.g. a `None` default for an `Optional[T]` parameter runs
    /// the empty-Optional constructor). Produced only when the checker recorded
    /// an implicit conversion at the default expression's span.
    Construct {
        target: String,
        arg: Box<CheckedConst>,
    },
}

impl CheckedConst {
    /// Convert an expression that is already a literal constant.
    pub fn from_expr(expr: &Expr) -> Option<Self> {
        match &expr.kind {
            ExprKind::Int(value) => Some(Self::Int(value.clone())),
            ExprKind::Float(value) => Some(Self::Float(value.clone())),
            ExprKind::Bool(value) => Some(Self::Bool(*value)),
            ExprKind::Str(value) => Some(Self::String(value.clone())),
            ExprKind::None => Some(Self::None),
            ExprKind::Prefix(PrefixOp::Neg, inner) => match Self::from_expr(inner)? {
                Self::Int(value) => Some(Self::Int(value.neg())),
                Self::Float(value) => Some(Self::Float(value.neg())),
                _ => None,
            },
            _ => None,
        }
    }
}

impl CheckedProgram {
    // This is the single checker-to-checked-boundary assembly point. Keeping the
    // phase-owned fact tables explicit makes accidental omission visible.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        statements: Vec<Stmt>,
        overload_targets: HashMap<SourceSpan, String>,
        contextual_bases: HashMap<SourceSpan, String>,
        generic_instantiations: HashMap<SourceSpan, GenericInstantiation>,
        call_transfers: HashMap<SourceSpan, Vec<CheckedCallTransfer>>,
        implicit_conversions: HashMap<SourceSpan, String>,
        conversion_source_borrows: HashMap<SourceSpan, bool>,
        checked_types: HashMap<AnnotationSite, Ty>,
        generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
        expression_types: HashMap<SourceSpan, Ty>,
        expression_bindings: HashMap<SourceSpan, mojito_types::origin::OwnerId>,
        statement_bindings: HashMap<SourceSpan, mojito_types::origin::OwnerId>,
        declaration_captures: HashMap<SourceSpan, Vec<CheckedCapture>>,
        comprehension_bindings: HashMap<SourceSpan, Vec<CheckedComprehensionBinding>>,
        expression_place_types: HashMap<SourceSpan, Ty>,
        binding_types: HashMap<SourceSpan, Ty>,
        expression_effects: HashMap<SourceSpan, EffectFacts>,
        selected_calls: HashMap<SourceSpan, CheckedCallContract>,
        subscript_descriptors: HashMap<
            SourceSpan,
            (Vec<Option<mojito_types::types::SliceKind>>, bool),
        >,
        iteration_protocols: HashMap<SourceSpan, IterationProtocol>,
        simd_constructions: HashMap<SourceSpan, (mojito_ast::ast::Dtype, i64)>,
        operation_adjustments: HashMap<SourceSpan, SemanticAdjustment>,
        tuple_unpack_plans: HashMap<SourceSpan, Vec<CheckedTupleUnpackElement>>,
        interior_references: HashMap<SourceSpan, mojito_types::origin::OriginPlace>,
        interior_invalidations: HashMap<SourceSpan, Vec<InteriorInvalidation>>,
        explicit_destroy_types: HashMap<String, ExplicitDestroyInfo>,
        explicit_destroy_calls: HashSet<SourceSpan>,
        reference_value_uses: HashMap<SourceSpan, bool>,
        copy_place_value_uses: HashSet<SourceSpan>,
        call_place_uses: HashSet<SourceSpan>,
        borrowed_read_call_places: HashSet<SourceSpan>,
        implicitly_copied_consuming_receivers: HashSet<SourceSpan>,
        declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
    ) -> Self {
        let (expressions, expression_index) = build_checked_expressions(
            &statements,
            &contextual_bases,
            &expression_types,
            &expression_bindings,
            &comprehension_bindings,
            &expression_place_types,
            &binding_types,
            &expression_effects,
            &selected_calls,
            &subscript_descriptors,
            &iteration_protocols,
            &simd_constructions,
            &operation_adjustments,
            &tuple_unpack_plans,
            &interior_references,
            &interior_invalidations,
            &overload_targets,
            &implicit_conversions,
            &conversion_source_borrows,
            &explicit_destroy_calls,
            &reference_value_uses,
            &copy_place_value_uses,
            &call_place_uses,
            &borrowed_read_call_places,
            &implicitly_copied_consuming_receivers,
        );
        let declarations = build_checked_declarations(
            &statements,
            &checked_types,
            &statement_bindings,
            &declaration_captures,
            &binding_types,
        );
        Self {
            statements,
            compatibility_overload_targets: overload_targets,
            generic_instantiations,
            call_transfers,
            compatibility_implicit_conversions: implicit_conversions,
            checked_types,
            generic_parameters,
            expressions,
            expression_index,
            declarations,
            explicit_destroy_types,
            declaration_effects,
        }
    }

    /// Elaborated statements accepted by semantic checking.
    pub fn statements(&self) -> &[Stmt] {
        &self.statements
    }

    /// Checker-selected lowered callable name at each resolved call site.
    pub fn overload_targets(&self) -> &HashMap<SourceSpan, String> {
        &self.compatibility_overload_targets
    }

    /// The resolved generic application at each bound-generic call site: the
    /// source callee and the exact compile-time arguments `resolve_use_params`
    /// selected (types, values, and origins, symbolic entries included).
    /// Recorded so instantiation discovery can monomorphize call sites the
    /// elaborator cannot re-infer on its own.
    pub fn generic_instantiations(&self) -> &HashMap<SourceSpan, GenericInstantiation> {
        &self.generic_instantiations
    }

    /// The caller-substituted loan transfers per call occurrence.
    pub fn call_transfers(&self) -> &HashMap<SourceSpan, Vec<CheckedCallTransfer>> {
        &self.call_transfers
    }

    /// Checker-selected converting constructor for an expression used in a
    /// context that permits a user-defined implicit conversion.
    pub fn implicit_conversions(&self) -> &HashMap<SourceSpan, String> {
        &self.compatibility_implicit_conversions
    }

    pub fn checked_type_at(&self, site: &AnnotationSite) -> Option<&Ty> {
        self.checked_types.get(site)
    }

    pub fn generic_parameters_at(
        &self,
        site: &GenericSite,
    ) -> Option<&[mojito_types::types::ParamDecl]> {
        self.generic_parameters.get(site).map(Vec::as_slice)
    }

    /// Checked raising contract and reference-return fact for a callable
    /// declaration site.
    pub fn declaration_effect_at(&self, site: &AnnotationSite) -> Option<&DeclarationEffect> {
        self.declaration_effects.get(site)
    }

    pub fn expressions(&self) -> &[CheckedExpr] {
        &self.expressions
    }

    pub fn declarations(&self) -> &[CheckedDeclaration] {
        &self.declarations
    }

    pub fn declaration(&self, id: CheckedDeclId) -> Option<&CheckedDeclaration> {
        self.declarations.get(id.0 as usize)
    }

    pub fn expression(&self, id: CheckedNodeId) -> Option<&CheckedExpr> {
        self.expressions.get(id.0 as usize)
    }

    /// Compatibility lookup while HIR migrates from syntax to checked ids.
    /// Multiple nodes may share a source location, so callers must never use the
    /// span itself as semantic identity.
    pub fn expression_ids_at(&self, span: &SourceSpan) -> &[CheckedNodeId] {
        self.expression_index
            .get(span)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn explicit_destroy_types(&self) -> &HashMap<String, ExplicitDestroyInfo> {
        &self.explicit_destroy_types
    }

    /// Module identity attached to a declaration or expression location.
    pub fn declaration_module<'a>(&self, location: &'a SourceSpan) -> Option<&'a str> {
        location.source.as_deref()
    }
}

// Mirrors `CheckedProgram::new` while the fact tables are folded into stable
// checked nodes. A named input record can replace this once the old maps retire.
#[allow(clippy::too_many_arguments)]
fn build_checked_expressions(
    statements: &[Stmt],
    contextual_bases: &HashMap<SourceSpan, String>,
    types: &HashMap<SourceSpan, Ty>,
    bindings: &HashMap<SourceSpan, mojito_types::origin::OwnerId>,
    comprehension_bindings: &HashMap<SourceSpan, Vec<CheckedComprehensionBinding>>,
    place_types: &HashMap<SourceSpan, Ty>,
    binding_types: &HashMap<SourceSpan, Ty>,
    effects: &HashMap<SourceSpan, EffectFacts>,
    selected_calls: &HashMap<SourceSpan, CheckedCallContract>,
    subscript_descriptors: &HashMap<
        SourceSpan,
        (Vec<Option<mojito_types::types::SliceKind>>, bool),
    >,
    iteration_protocols: &HashMap<SourceSpan, IterationProtocol>,
    simd_constructions: &HashMap<SourceSpan, (mojito_ast::ast::Dtype, i64)>,
    operation_adjustments: &HashMap<SourceSpan, SemanticAdjustment>,
    tuple_unpack_plans: &HashMap<SourceSpan, Vec<CheckedTupleUnpackElement>>,
    interior_references: &HashMap<SourceSpan, mojito_types::origin::OriginPlace>,
    interior_invalidations: &HashMap<SourceSpan, Vec<InteriorInvalidation>>,
    calls: &HashMap<SourceSpan, String>,
    conversions: &HashMap<SourceSpan, String>,
    conversion_source_borrows: &HashMap<SourceSpan, bool>,
    explicit_destroy: &HashSet<SourceSpan>,
    reference_value_uses: &HashMap<SourceSpan, bool>,
    copy_place_value_uses: &HashSet<SourceSpan>,
    call_place_uses: &HashSet<SourceSpan>,
    borrowed_read_call_places: &HashSet<SourceSpan>,
    implicitly_copied_consuming_receivers: &HashSet<SourceSpan>,
) -> (Vec<CheckedExpr>, HashMap<SourceSpan, Vec<CheckedNodeId>>) {
    struct Builder<'a> {
        nodes: Vec<CheckedExpr>,
        index: HashMap<SourceSpan, Vec<CheckedNodeId>>,
        contextual_bases: &'a HashMap<SourceSpan, String>,
        types: &'a HashMap<SourceSpan, Ty>,
        bindings: &'a HashMap<SourceSpan, mojito_types::origin::OwnerId>,
        comprehension_bindings: &'a HashMap<SourceSpan, Vec<CheckedComprehensionBinding>>,
        place_types: &'a HashMap<SourceSpan, Ty>,
        binding_types: &'a HashMap<SourceSpan, Ty>,
        effects: &'a HashMap<SourceSpan, EffectFacts>,
        selected_calls: &'a HashMap<SourceSpan, CheckedCallContract>,
        subscript_descriptors:
            &'a HashMap<SourceSpan, (Vec<Option<mojito_types::types::SliceKind>>, bool)>,
        iteration_protocols: &'a HashMap<SourceSpan, IterationProtocol>,
        simd_constructions: &'a HashMap<SourceSpan, (mojito_ast::ast::Dtype, i64)>,
        operation_adjustments: &'a HashMap<SourceSpan, SemanticAdjustment>,
        tuple_unpack_plans: &'a HashMap<SourceSpan, Vec<CheckedTupleUnpackElement>>,
        interior_references: &'a HashMap<SourceSpan, mojito_types::origin::OriginPlace>,
        interior_invalidations: &'a HashMap<SourceSpan, Vec<InteriorInvalidation>>,
        calls: &'a HashMap<SourceSpan, String>,
        conversions: &'a HashMap<SourceSpan, String>,
        conversion_source_borrows: &'a HashMap<SourceSpan, bool>,
        explicit_destroy: &'a HashSet<SourceSpan>,
        reference_value_uses: &'a HashMap<SourceSpan, bool>,
        copy_place_value_uses: &'a HashSet<SourceSpan>,
        call_place_uses: &'a HashSet<SourceSpan>,
        borrowed_read_call_places: &'a HashSet<SourceSpan>,
        implicitly_copied_consuming_receivers: &'a HashSet<SourceSpan>,
    }
    impl Builder<'_> {
        fn expr(&mut self, expression: &Expr) -> CheckedNodeId {
            use ExprKind::*;
            let mut children = Vec::new();
            let mut add = |this: &mut Self, child: &Expr| children.push(this.expr(child));
            match &expression.kind {
                Prefix(_, value) | Transfer(value) | Spread(value) | Named { value, .. } => {
                    add(self, value)
                }
                Infix(_, left, right)
                | Index {
                    object: left,
                    index: right,
                } => {
                    add(self, left);
                    add(self, right);
                }
                Call {
                    param_args,
                    args,
                    kwargs,
                    ..
                } => {
                    for argument in param_args {
                        match argument {
                            mojito_ast::ast::ParamArg::Value(value) => add(self, value),
                            mojito_ast::ast::ParamArg::Named { value, .. } => {
                                if let mojito_ast::ast::ParamArg::Value(value) = &**value {
                                    add(self, value);
                                }
                            }
                            mojito_ast::ast::ParamArg::Type(_) => {}
                        }
                    }
                    for value in args {
                        add(self, value);
                    }
                    for value in kwargs {
                        add(self, &value.value);
                    }
                }
                Invoke {
                    callee,
                    param_args,
                    args,
                    kwargs,
                } => {
                    add(self, callee);
                    for argument in param_args {
                        match argument {
                            mojito_ast::ast::ParamArg::Value(value) => add(self, value),
                            mojito_ast::ast::ParamArg::Named { value, .. } => {
                                if let mojito_ast::ast::ParamArg::Value(value) = &**value {
                                    add(self, value);
                                }
                            }
                            mojito_ast::ast::ParamArg::Type(_) => {}
                        }
                    }
                    for value in args {
                        add(self, value);
                    }
                    for value in kwargs {
                        add(self, &value.value);
                    }
                }
                Member { object, .. } => add(self, object),
                MethodCall {
                    object,
                    args,
                    kwargs,
                    ..
                } => {
                    add(self, object);
                    for value in args {
                        add(self, value);
                    }
                    for value in kwargs {
                        add(self, &value.value);
                    }
                }
                ListLit(values) | TupleLit(values) => {
                    for value in values {
                        add(self, value);
                    }
                }
                BraceLit(values) => {
                    for (key, value) in values {
                        add(self, key);
                        if let Some(value) = value {
                            add(self, value);
                        }
                    }
                }
                Comprehension {
                    key,
                    value,
                    clauses,
                    ..
                } => {
                    for clause in clauses {
                        match clause {
                            mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                                add(self, iter)
                            }
                            mojito_ast::ast::ComprehensionClause::If(condition) => {
                                add(self, condition)
                            }
                        }
                    }
                    if let Some(key) = key {
                        add(self, key);
                    }
                    add(self, value);
                }
                IfExpr {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    add(self, cond);
                    add(self, then_branch);
                    add(self, else_branch);
                }
                Compare { first, rest } => {
                    add(self, first);
                    for (_, value) in rest {
                        add(self, value);
                    }
                }
                Slice {
                    object,
                    lower,
                    upper,
                    step,
                    ..
                } => {
                    add(self, object);
                    for value in [lower, upper, step].into_iter().flatten() {
                        add(self, value);
                    }
                }
                MultiIndex { object, args } => {
                    add(self, object);
                    for argument in args {
                        match argument {
                            mojito_ast::ast::SubscriptArg::Index(value)
                            | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                                add(self, value)
                            }
                            mojito_ast::ast::SubscriptArg::Slice {
                                lower, upper, step, ..
                            }
                            | mojito_ast::ast::SubscriptArg::KeywordSlice {
                                lower,
                                upper,
                                step,
                                ..
                            } => {
                                for value in [lower, upper, step].into_iter().flatten() {
                                    add(self, value);
                                }
                            }
                        }
                    }
                }
                TString { parts, .. } => {
                    for part in parts {
                        if let mojito_ast::ast::TStringPart::Expr(value) = part {
                            add(self, value);
                        }
                    }
                }
                // A lambda expression is a leaf here: its hidden definition's
                // body belongs to that definition's own checked declaration,
                // not to the enclosing expression tree.
                Int(_) | Float(_) | Bool(_) | Str(_) | None | Uninitialized | EmptySubscript
                | Identifier(_) | TypeValue(_) => {}
                // A lambda expression node has no expression children of its
                // own, but its hidden definition's body must be built exactly
                // like a nested `def` statement's body so the body's checked
                // facts become checked nodes.
                Lambda { def } => self.block(std::slice::from_ref(def)),
                TypeApply { args, .. } => {
                    for argument in args {
                        match argument {
                            mojito_ast::ast::ParamArg::Value(value) => add(self, value),
                            mojito_ast::ast::ParamArg::Named { value, .. } => {
                                if let mojito_ast::ast::ParamArg::Value(value) = &**value {
                                    add(self, value);
                                }
                            }
                            mojito_ast::ast::ParamArg::Type(_) => {}
                        }
                    }
                }
            }
            let span = expression.source_span();
            let mut adjustments = Vec::new();
            if let Some(target) = self.calls.get(&span) {
                adjustments.push(SemanticAdjustment::ResolveCallable(target.clone()));
            }
            if let Some(call) = self.selected_calls.get(&span) {
                adjustments.push(SemanticAdjustment::SelectedCall(Box::new(call.clone())));
            }
            if let Some((descriptors, set_value_keyword)) = self.subscript_descriptors.get(&span) {
                adjustments.push(SemanticAdjustment::SliceDescriptors {
                    descriptors: descriptors.clone(),
                    set_value_keyword: *set_value_keyword,
                });
            }
            if let Some(target) = self.conversions.get(&span) {
                if mojito_symbol::symbol::is_index_normalization_symbol(target) {
                    adjustments.push(SemanticAdjustment::IndexNormalization {
                        target: target.clone(),
                    });
                } else {
                    adjustments.push(SemanticAdjustment::ImplicitConversion(target.clone()));
                    if let Some(mutable) = self.conversion_source_borrows.get(&span) {
                        adjustments
                            .push(SemanticAdjustment::BorrowConversionSource { mutable: *mutable });
                    }
                }
            }
            if matches!(expression.kind, Transfer(_)) {
                adjustments.push(SemanticAdjustment::Move);
            }
            if self.explicit_destroy.contains(&span) {
                adjustments.push(SemanticAdjustment::ExplicitDestroy);
            }
            if let Some(protocol) = self.iteration_protocols.get(&span) {
                adjustments.push(SemanticAdjustment::Iterate(protocol.clone()));
            }
            if let Some(writable) = self.reference_value_uses.get(&span) {
                adjustments.push(if *writable {
                    SemanticAdjustment::BorrowMutable
                } else {
                    SemanticAdjustment::BorrowShared
                });
            }
            if self.copy_place_value_uses.contains(&span) {
                adjustments.push(SemanticAdjustment::CopyPlaceValue);
            }
            if self.call_place_uses.contains(&span) {
                adjustments.push(SemanticAdjustment::RetainCallPlace);
            }
            if self.borrowed_read_call_places.contains(&span) {
                adjustments.push(SemanticAdjustment::BorrowReadArgument);
            }
            if self.implicitly_copied_consuming_receivers.contains(&span) {
                adjustments.push(SemanticAdjustment::ImplicitlyCopyConsumingReceiver);
            }
            if let Some((dtype, width)) = self.simd_constructions.get(&span) {
                adjustments.push(SemanticAdjustment::ConstructSimd {
                    dtype: *dtype,
                    width: *width,
                });
            }
            if let Some(operation) = self.operation_adjustments.get(&span) {
                adjustments.push(operation.clone());
            }
            if let Some(elements) = self.tuple_unpack_plans.get(&span) {
                adjustments.push(SemanticAdjustment::TupleUnpack {
                    elements: elements.clone(),
                });
            }
            if let Some(origin) = self.interior_references.get(&span) {
                adjustments.push(SemanticAdjustment::InteriorReference {
                    origin: origin.clone(),
                });
            }
            if let Some(invalidations) = self.interior_invalidations.get(&span) {
                adjustments.push(SemanticAdjustment::InvalidateInteriors {
                    invalidations: invalidations.clone(),
                });
            }
            let variant_projection =
                self.operation_adjustments
                    .get(&span)
                    .is_some_and(|operation| {
                        matches!(operation, SemanticAdjustment::VariantProject { .. })
                    });
            let resolved_callable_value = self.calls.contains_key(&span);
            let ty = self.types.get(&span).cloned();
            let place_ty = self.place_types.get(&span).cloned();
            let binding_ty = self.binding_types.get(&span).cloned();
            let binding = self.bindings.get(&span).copied();
            let category = match expression.kind {
                TypeApply { .. } if variant_projection => ValueCategory::Place,
                TypeApply { .. } if resolved_callable_value => ValueCategory::Value,
                TypeApply { .. } | TypeValue(_) => ValueCategory::Type,
                // Expressions retained only inside constraints, reflection, and
                // other erased parameter metadata are deliberately absent from
                // the runtime type/place maps. They still cross the checked
                // boundary as explicit compile-time nodes rather than being
                // mislabeled as untyped runtime places.
                _ if ty.is_none() && place_ty.is_none() && binding.is_none() => {
                    ValueCategory::CompileTime
                }
                Identifier(_) | Member { .. } | Index { .. } => ValueCategory::Place,
                _ => ValueCategory::Value,
            };
            let id = CheckedNodeId(self.nodes.len() as u32);
            self.nodes.push(CheckedExpr {
                id,
                syntax: expression.clone(),
                ty,
                place_ty,
                binding_ty,
                category,
                binding,
                contextual_base: self.contextual_bases.get(&span).cloned(),
                effects: self.effects.get(&span).cloned().unwrap_or_default(),
                adjustments,
                children,
                comprehension_bindings: self
                    .comprehension_bindings
                    .get(&span)
                    .cloned()
                    .unwrap_or_default(),
            });
            self.index.entry(span).or_default().push(id);
            id
        }

        fn block(&mut self, statements: &[Stmt]) {
            use mojito_ast::ast::StmtKind::*;
            for statement in statements {
                match &statement.kind {
                    VarDecl { value, .. }
                    | RefDecl { value, .. }
                    | Assign { value, .. }
                    | Raise(value)
                    | Expr(value) => {
                        self.expr(value);
                    }
                    Comptime {
                        where_clauses,
                        value,
                        ..
                    } => {
                        for constraint in where_clauses {
                            self.expr(constraint);
                        }
                        self.expr(value);
                    }
                    AugAssign { place, value, .. } | SetPlace { place, value } => {
                        self.expr(place);
                        self.expr(value);
                    }
                    Unpack { targets, value, .. } => {
                        for target in targets {
                            self.expr(target);
                        }
                        self.expr(value);
                    }
                    Def {
                        params,
                        where_clauses,
                        body,
                        ..
                    } => {
                        for param in params {
                            if let Some(value) = &param.default {
                                self.expr(value);
                            }
                        }
                        for value in where_clauses {
                            self.expr(value);
                        }
                        self.block(body);
                    }
                    Struct {
                        conformance_conditions,
                        where_clauses,
                        associated,
                        methods,
                        ..
                    } => {
                        for (_, value) in conformance_conditions {
                            self.expr(value);
                        }
                        for value in where_clauses {
                            self.expr(value);
                        }
                        for value in associated {
                            for constraint in &value.where_clauses {
                                self.expr(constraint);
                            }
                            self.expr(&value.value);
                        }
                        for method in methods {
                            for param in &method.params {
                                if let Some(value) = &param.default {
                                    self.expr(value);
                                }
                            }
                            for value in &method.where_clauses {
                                self.expr(value);
                            }
                            self.block(&method.body);
                        }
                    }
                    Trait {
                        methods,
                        comptime_members,
                        ..
                    } => {
                        for method in methods {
                            for param in &method.params {
                                if let Some(value) = &param.default {
                                    self.expr(value);
                                }
                            }
                            for value in &method.where_clauses {
                                self.expr(value);
                            }
                            if let Some(body) = &method.default_body {
                                self.block(body);
                            }
                        }
                        for member in comptime_members {
                            for constraint in &member.where_clauses {
                                self.expr(constraint);
                            }
                        }
                    }
                    ComptimeIf { branches, orelse } | If { branches, orelse } => {
                        for (condition, body) in branches {
                            self.expr(condition);
                            self.block(body);
                        }
                        if let Some(body) = orelse {
                            self.block(body);
                        }
                    }
                    ComptimeFor { iter, body, .. } => {
                        self.expr(iter);
                        self.block(body);
                    }
                    While { cond, body, orelse } => {
                        self.expr(cond);
                        self.block(body);
                        if let Some(body) = orelse {
                            self.block(body);
                        }
                    }
                    For {
                        iter, body, orelse, ..
                    } => {
                        self.expr(iter);
                        self.block(body);
                        if let Some(body) = orelse {
                            self.block(body);
                        }
                    }
                    Return(value) => {
                        if let Some(value) = value {
                            self.expr(value);
                        }
                    }
                    With { items, body } => {
                        for item in items {
                            self.expr(&item.context);
                        }
                        self.block(body);
                    }
                    Try {
                        body,
                        except,
                        orelse,
                        finalbody,
                    } => {
                        self.block(body);
                        if let Some((_, body)) = except {
                            self.block(body);
                        }
                        if let Some(body) = orelse {
                            self.block(body);
                        }
                        if let Some(body) = finalbody {
                            self.block(body);
                        }
                    }
                    Import { .. } | FromImport { .. } | Pass | Break | Continue => {}
                }
            }
        }
    }
    let mut builder = Builder {
        nodes: Vec::new(),
        index: HashMap::new(),
        contextual_bases,
        types,
        bindings,
        comprehension_bindings,
        place_types,
        binding_types,
        effects,
        selected_calls,
        subscript_descriptors,
        iteration_protocols,
        simd_constructions,
        operation_adjustments,
        tuple_unpack_plans,
        interior_references,
        interior_invalidations,
        calls,
        conversions,
        conversion_source_borrows,
        explicit_destroy,
        reference_value_uses,
        copy_place_value_uses,
        call_place_uses,
        borrowed_read_call_places,
        implicitly_copied_consuming_receivers,
    };
    builder.block(statements);
    (builder.nodes, builder.index)
}

fn build_checked_declarations(
    statements: &[Stmt],
    annotation_types: &HashMap<AnnotationSite, Ty>,
    statement_bindings: &HashMap<SourceSpan, mojito_types::origin::OwnerId>,
    declaration_captures: &HashMap<SourceSpan, Vec<CheckedCapture>>,
    binding_types: &HashMap<SourceSpan, Ty>,
) -> Vec<CheckedDeclaration> {
    fn block(
        statements: &[Stmt],
        out: &mut Vec<CheckedDeclaration>,
        annotation_types: &HashMap<AnnotationSite, Ty>,
        statement_bindings: &HashMap<SourceSpan, mojito_types::origin::OwnerId>,
        declaration_captures: &HashMap<SourceSpan, Vec<CheckedCapture>>,
        binding_types: &HashMap<SourceSpan, Ty>,
    ) -> Vec<CheckedDeclId> {
        use mojito_ast::ast::StmtKind;
        let mut ids = Vec::new();
        for statement in statements {
            // Lambda expressions in this statement's own expressions declare
            // hidden nested functions in this block's scope.
            let mut lambda_nodes = Vec::new();
            mojito_ast::ast::lambdas_in_stmt(statement, &mut lambda_nodes);
            for lambda in lambda_nodes {
                let mojito_ast::ast::ExprKind::Lambda { def } = &lambda.kind else {
                    continue;
                };
                let StmtKind::Def {
                    name,
                    body: def_body,
                    ..
                } = &def.kind
                else {
                    continue;
                };
                let id = CheckedDeclId(out.len() as u32);
                let location = def.source_span();
                out.push(CheckedDeclaration {
                    id,
                    kind: CheckedDeclKind::Function,
                    name: name.clone(),
                    binding: statement_bindings.get(&location).copied(),
                    captures: declaration_captures
                        .get(&location)
                        .cloned()
                        .unwrap_or_default(),
                    location,
                    ty: annotation_types
                        .get(&AnnotationSite::FunctionType {
                            module: def.module.clone(),
                            declaration: def.span,
                            syntax: def.syntax_id,
                        })
                        .cloned(),
                    children: Vec::new(),
                });
                let children = block(
                    def_body,
                    out,
                    annotation_types,
                    statement_bindings,
                    declaration_captures,
                    binding_types,
                );
                out[id.0 as usize].children = children;
                ids.push(id);
            }
            let (kind, name) = match &statement.kind {
                StmtKind::Def { name, .. } => (CheckedDeclKind::Function, name.clone()),
                StmtKind::Struct { name, .. } => (CheckedDeclKind::Struct, name.clone()),
                StmtKind::Trait { name, .. } => (CheckedDeclKind::Trait, name.clone()),
                StmtKind::VarDecl { name, .. } | StmtKind::RefDecl { name, .. } => {
                    (CheckedDeclKind::Binding, name.clone())
                }
                StmtKind::For { var, .. } => (CheckedDeclKind::Binding, var.clone()),
                StmtKind::Try {
                    except: Some((Some(name), _)),
                    ..
                } => (CheckedDeclKind::Binding, name.clone()),
                StmtKind::Comptime { name, .. } => (CheckedDeclKind::CompileTime, name.clone()),
                _ => {
                    // Control-flow declarations are discovered recursively below;
                    // no fake declaration node is introduced for the container.
                    let nested: Vec<&[Stmt]> = match &statement.kind {
                        StmtKind::If { branches, orelse }
                        | StmtKind::ComptimeIf { branches, orelse } => branches
                            .iter()
                            .map(|(_, body)| body.as_slice())
                            .chain(orelse.iter().map(Vec::as_slice))
                            .collect(),
                        StmtKind::While { body, orelse, .. }
                        | StmtKind::For { body, orelse, .. } => std::iter::once(body.as_slice())
                            .chain(orelse.iter().map(Vec::as_slice))
                            .collect(),
                        StmtKind::Try {
                            body,
                            except,
                            orelse,
                            finalbody,
                        } => std::iter::once(body.as_slice())
                            .chain(except.iter().map(|(_, body)| body.as_slice()))
                            .chain(orelse.iter().map(Vec::as_slice))
                            .chain(finalbody.iter().map(Vec::as_slice))
                            .collect(),
                        StmtKind::With { body, .. } | StmtKind::ComptimeFor { body, .. } => {
                            vec![body]
                        }
                        _ => Vec::new(),
                    };
                    for body in nested {
                        ids.extend(block(
                            body,
                            out,
                            annotation_types,
                            statement_bindings,
                            declaration_captures,
                            binding_types,
                        ));
                    }
                    continue;
                }
            };
            let id = CheckedDeclId(out.len() as u32);
            let location = statement.source_span();
            let ty = match kind {
                CheckedDeclKind::Function => annotation_types
                    .get(&AnnotationSite::FunctionType {
                        module: statement.module.clone(),
                        declaration: statement.span,
                        syntax: statement.syntax_id,
                    })
                    .cloned(),
                CheckedDeclKind::Binding | CheckedDeclKind::CompileTime => {
                    binding_types.get(&location).cloned()
                }
                CheckedDeclKind::Struct | CheckedDeclKind::Trait => None,
            };
            out.push(CheckedDeclaration {
                id,
                kind,
                name,
                binding: statement_bindings.get(&location).copied(),
                captures: declaration_captures
                    .get(&location)
                    .cloned()
                    .unwrap_or_default(),
                location,
                ty,
                children: Vec::new(),
            });
            let children = match &statement.kind {
                StmtKind::Def { body, .. } => block(
                    body,
                    out,
                    annotation_types,
                    statement_bindings,
                    declaration_captures,
                    binding_types,
                ),
                StmtKind::Struct { methods, .. } => methods
                    .iter()
                    .flat_map(|method| {
                        block(
                            &method.body,
                            out,
                            annotation_types,
                            statement_bindings,
                            declaration_captures,
                            binding_types,
                        )
                    })
                    .collect(),
                StmtKind::For { body, orelse, .. } => block(
                    body,
                    out,
                    annotation_types,
                    statement_bindings,
                    declaration_captures,
                    binding_types,
                )
                .into_iter()
                .chain(orelse.iter().flat_map(|body| {
                    block(
                        body,
                        out,
                        annotation_types,
                        statement_bindings,
                        declaration_captures,
                        binding_types,
                    )
                }))
                .collect(),
                StmtKind::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } => std::iter::once(body.as_slice())
                    .chain(except.iter().map(|(_, body)| body.as_slice()))
                    .chain(orelse.iter().map(Vec::as_slice))
                    .chain(finalbody.iter().map(Vec::as_slice))
                    .flat_map(|body| {
                        block(
                            body,
                            out,
                            annotation_types,
                            statement_bindings,
                            declaration_captures,
                            binding_types,
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            out[id.0 as usize].children = children;
            ids.push(id);
        }
        ids
    }
    let mut declarations = Vec::new();
    let _ = block(
        statements,
        &mut declarations,
        annotation_types,
        statement_bindings,
        declaration_captures,
        binding_types,
    );
    declarations
}

#[cfg(test)]
mod transfer_set_tests {
    use super::*;

    #[test]
    fn transfer_sets_are_transparent_to_type_identity() {
        // Two otherwise-equal function types must never differ by their
        // inferred effects; acceptance and coercion do not consult them.
        let effect = TransferEffect {
            dest: mojito_types::origin::SigOrigin::Self_,
            src: mojito_types::origin::SigOrigin::Param(0),
            src_is_place: false,
            mutable: true,
        };
        assert_eq!(TransferSet(vec![effect]), TransferSet(Vec::new()));
    }
}
