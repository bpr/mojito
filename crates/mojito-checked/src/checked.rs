//! Checked semantic handoff between the frontend and lowering.

use mojito_ast::ast::Stmt;
use mojito_ast::ast::{
    ArgConvention, BindingMode, CollectionKind, ComprehensionClause, Expr, ExprKind, ImportNames,
    InfixOp, KwArg, MethodProvenance, ParamArg, ParamKind, PrefixOp, SubscriptArg, TStringPart,
    TypeParam,
};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::{SourceSpan, Span, SyntaxId};
use mojito_types::types::{SimdDtype, SimdWidth, Ty};
use std::collections::HashMap;
use std::sync::Arc;

mod encoding;

/// Stable identity within one checked program. Unlike a source span this remains
/// unique for synthesized nodes and for multiple semantic nodes at one location.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct CheckedNodeId(pub u32);

/// How an expression participates in evaluation.
///
/// New categories can be added without changing source syntax; in particular,
/// future patterns and suspended computations can refer to nodes rather than
/// impersonating ordinary values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum ValueCategory {
    Value,
    Place,
    Type,
    CompileTime,
}

/// Extensible control/effect summary.
///
/// Suspension is deliberately independent from raising: future coroutines,
/// generators, and delimited continuations can introduce resume edges without
/// being encoded as exceptions or calls.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct EffectFacts {
    pub raises: Option<Ty>,
    pub may_suspend: bool,
    pub diverges: bool,
}

/// One source argument after ordinary call binding selected a concrete method
/// parameter.
///
/// Keeping the source slot and effective convention together lets HIR/MIR
/// retain caller storage without rebuilding keyword/default binding or
/// origin-dependent `ref` mutability.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallArgument {
    pub source: CheckedCallArgumentSource,
    pub parameter_ty: Ty,
    /// The declared ABI requires a caller place even when origin solving turns
    /// an immutable `ref` into an effective read for conflict analysis.
    pub requires_place: bool,
    /// Effective access after parametric reference mutability is solved.
    pub convention: Option<mojito_ast::ast::ArgConvention>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckedCallArgumentSource {
    Positional(usize),
    Keyword(usize),
    Default,
}

/// One source-ordered compile-time method argument.
///
/// Type arguments occupy a declaration slot with no runtime value; a value
/// argument names, by position, the call's explicit compile-time operand
/// whose already-evaluated register lowering reuses. A subscript accessor's
/// operands are its bracket's positional operands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallParameterArgument {
    pub name: Option<String>,
    pub operand: Option<usize>,
}

/// One checker-selected adaptation of an already-evaluated call argument.
///
/// These are deliberately call-local: augmented subscripts may send the same
/// source value through different getter and setter parameter contracts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckedCallValueAdjustment {
    ResolveCallable { target: String },
    ImplicitConversion { target: String },
    IndexNormalization { target: String },
    MaterializeLiteral { target: Ty },
    SplatScalar { target: Ty },
}

/// Call-boundary facts for one supplied argument.
///
/// `value_source` identifies the source occurrence which MIR evaluates once.
/// Lowering must derive a separate per-call register by applying
/// `adjustments`, retain the original caller place when the corresponding
/// [`CheckedCallArgument`] requires it, and emit the invalidations immediately
/// at this call rather than at source evaluation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallArgumentBoundary {
    pub source: CheckedCallArgumentSource,
    pub value_source: SourceSpan,
    pub adjustments: Vec<CheckedCallValueAdjustment>,
    pub invalidations: Vec<InteriorInvalidation>,
}

/// Effects and value adaptations whose semantic location is one selected call.
///
/// Keeping this on the contract makes two calls over shared syntax independent:
/// the source operands are still evaluated once, but conversions and mutation
/// effects occur according to each callee's own signature.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallBoundary {
    pub arguments: Vec<CheckedCallArgumentBoundary>,
    /// Receiver/call-site generation changes, emitted at the call boundary.
    pub invalidations: Vec<InteriorInvalidation>,
}

/// A checker-proven adaptation applied at an abstract call boundary.
///
/// This is deliberately not inferred from the runtime value: a value-returning
/// method may itself produce a reference-valued element.  Execution consults
/// the retargeted declaration's ABI and performs this adapter only when that
/// declaration returns through the reference ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckedResultAdapter {
    /// Mojo permits `Iterator.__next__ -> Element` to be implemented as
    /// `__next__ -> ref[o] Element` when `Element: Copyable`.  An abstract call
    /// observes the promised value result, so it must read and lifecycle-copy
    /// the concrete reference return.
    CopyIteratorReference,
}

/// Canonical checker-to-lowering contract for one selected method-like call.
///
/// Nominal subscripts use this exact record rather than a reduced parallel
/// resolver.  Future syntactic call forms can share it without teaching MIR
/// overload resolution, origin solving, or effect inference.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// The selected target declares no receiver: a `@staticmethod` reached
    /// through an instance (`value.static_method()`). The receiver is
    /// evaluated and discarded; the call passes only the arguments.
    pub receiver_elided: bool,
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

/// The operation selected for `receiver[index] OP= rhs`.
///
/// A value-returning getter is followed by a setter, while a mutable-reference
/// getter writes the computed result directly through that handle and
/// therefore has no setter. In either case the receiver and indices are
/// evaluated only once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
}

/// The two-call plan for a bare element call `value[i](args)` (also member
/// bases and multi-index brackets): subscript the indexable runtime value,
/// then dispatch the element as a callable.
///
/// The getter contract lives here rather than in `SelectedCall` so node-level
/// consumers (reference-result diversion, `resolved_callable`,
/// `checked_raises`) see the element call, not the subscript read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Ownership mode selected for a checked iteration expression.
///
/// This is a semantic distinction, not a runtime guess: owned iteration must
/// dispatch to an `__iter__(var self)` implementation, while ordinary
/// iteration uses a borrowed receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IterationMode {
    Borrowed,
    Owned,
}

/// Checker-proven adaptation from one raw `__next__` result into the loop
/// target's storage convention.
///
/// Source ownership is deliberately absent: [`IterationMode`] selects borrowed
/// versus consuming `__iter__`, while this enum describes only what the target
/// spelling does with each yielded item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Complete checked plan for one loop/comprehension target.
///
/// `yielded_ty` is the exact result type of `__next__`; `binding_ty` is the
/// user-visible storage type. `mutable` is the loop variable's declared
/// mutability.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedIterationBinding {
    pub mode: mojito_ast::ast::BindingMode,
    pub action: IterationBindingAction,
    pub yielded_ty: Ty,
    pub binding_ty: Ty,
    pub mutable: bool,
}

/// Exact checked contract for one iterator `__next__` call.
///
/// The result type is the type produced by the call itself, so a
/// reference-yielding iterator keeps its `TyKind::Ref` handle type rather than
/// collapsing to the referent.  The parallel reference fact carries the
/// executable origin/capability contract needed by lowering, while `raises`
/// records the selected exhaustion effect.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedIteratorCall {
    #[serde(serialize_with = "mojito_types::symbol_ref::serialize")]
    pub target: String,
    pub result_ty: Ty,
    pub reference_result: Option<mojito_types::origin::RefTy>,
    pub raises: Option<Ty>,
    pub result_adapter: Option<CheckedResultAdapter>,
}

/// One resolved generic free-function application at a call site: the source
/// callee name and the compile-time arguments, in declaration order, exactly
/// as `resolve_use_params` returned them.
///
/// A construction records one too, under the struct's name: the struct's
/// arguments, then the selected constructor's own, in the layout the
/// constructor's declaration restates them (`Counter[4](1)` calls
/// `Counter[4].__init__`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GenericInstantiation {
    pub callee: String,
    /// The selected overload's runtime parameter names, in declaration order,
    /// as the callee's own signature lists them (regular parameters, with an
    /// `out` named result excluded). This is how a specialization request
    /// names one overload of an overloaded template, exactly as
    /// [`MethodInstantiation::parameter_names`] does for a method. Overloads
    /// that differ only in their parameter *types* share a name list and are
    /// told apart by [`Self::parameter_types`].
    pub parameter_names: Vec<String>,
    /// The same parameters' declared types, mangled as
    /// `mojito_symbol::symbol::TypeKey` so they compare directly against the
    /// declaration's own annotations. This breaks a tie between overloads
    /// sharing a parameter-name list (`f[T](a: T)` beside `f(a: String)`); a
    /// family ambiguous under both lists is told apart by [`Self::variadic`].
    pub parameter_types: Vec<String>,
    /// The selected overload's `*args` collector, which neither list spells:
    /// two type-pack overloads can agree on every regular parameter and differ
    /// only in where the collector sits or in its pack's bounds.
    pub variadic: Option<mojito_symbol::symbol::VariadicKey>,
    pub arguments: Vec<mojito_types::types::TyArg>,
    /// The bracket arguments [`Self::arguments`] carries as closed values, by
    /// their positions among the call's bracket arguments as written
    /// ([`SemanticAdjustment::FoldedParameterArguments`]).
    pub folded_arguments: Vec<usize>,
}

/// One resolved application of a struct method that declares its own
/// compile-time parameters (`v.isa[Int]()`, `v.set(3)` inferring `T`).
///
/// The receiver struct as checked, the source method name, and the method's
/// declaration-order arguments from `resolve_use_params`.
///
/// The compiler's discovery loop replays closed recordings on specialized
/// variadic structs as per-call method clones.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MethodInstantiation {
    pub owner: String,
    /// The receiver instance's declaration-order type arguments when `owner`
    /// is an ordinary generic struct reached at a closed instance
    /// (`Optional[Int]`): the per-call clone then lives on the template with
    /// the instance's arguments baked before the call's. Empty for a
    /// specialized variadic struct or a non-generic struct.
    pub owner_arguments: Vec<mojito_types::types::TyArg>,
    pub method: String,
    /// The selected overload's runtime parameter names, in declaration order.
    pub parameter_names: Vec<String>,
    /// The selected overload's signature qualifier (`$ov$T$Int`) when the
    /// method is overloaded: two same-arity overloads that share their
    /// parameter names mint only the selected one's clone.
    pub overload: Option<String>,
    pub arguments: Vec<mojito_types::types::TyArg>,
    /// The bracket arguments carried as closed values, by position, as
    /// [`GenericInstantiation::folded_arguments`] records a `def`'s.
    pub folded_arguments: Vec<usize>,
}

/// One closed application of an ordinary generic struct the checker reached as
/// a constructor target or method-call receiver (`Optional[Int]`): the
/// template name and its declaration-order arguments.
///
/// The compiler's discovery loop replays closed recordings as
/// per-instantiation method clones appended to the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructInstantiation {
    pub template: String,
    pub arguments: Vec<mojito_types::types::TyArg>,
}

/// Fully resolved iterator protocol retained across the checked boundary.
///
/// `prepare` contains the exact `__iter__` symbols needed to normalize a user
/// iterable; builtin ranges/collections leave it empty.  User iterators carry
/// exact iterator-operation symbols so the VM never performs name/arity
/// overload reconstruction. `exhaustion` is the checked typed error caught by
/// a raising `__next__`; it is absent only for compiler-private iterator
/// storage, which advances without a nominal method.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Whether the normalized iterator may refer to its source: its type
    /// carries an origin or its `__next__` returns a reference. Only then does
    /// the loop keep a split source live (and loaned) until the loop exits;
    /// otherwise the source dies as soon as `__iter__` returns, as upstream's
    /// ASAP destruction does.
    pub source_retained: bool,
    pub prepare: Vec<String>,
    /// The type of the iterator the `prepare` chain produces: its last
    /// `__iter__` result, resolved at the loop site (`_ListIter[Int,
    /// origin_of(xs)]`, or `C.IteratorType[origin_of(self)]` over a generic
    /// `C: Iterable`). An empty chain's iterator is the iterable itself.
    pub iterator_ty: Ty,
    pub next: Option<Box<CheckedIteratorCall>>,
    pub exhaustion: Option<Ty>,
}

impl IterationProtocol {
    /// Whether the loop indexes a borrowed runtime pack in place, yielding a
    /// reference to each element, rather than advancing an iterator: the
    /// shape of upstream's `_VariadicListMemIter`, a cursor over the
    /// borrowed list.
    pub fn indexes_pack(&self) -> bool {
        self.mode == IterationMode::Borrowed
            && self.next.is_none()
            && self.borrowed_origin.is_some()
            && matches!(
                self.iterator_ty.kind(),
                mojito_types::types::TyKind::VariadicPack(_)
            )
    }
}

/// A checked mutation of an origin that owns interior storage.
///
/// `except` names the reference through which the mutation occurs, when there
/// is one: writing through `element` keeps that reference's generation valid
/// while still invalidating interiors nested below the element.
/// `include_base_generation` distinguishes defining a fresh named
/// owned-interior generation (which replaces that exact generation) from an
/// ordinary write through its storage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InteriorInvalidation {
    pub base: mojito_types::origin::OriginPlace,
    pub except: Option<mojito_types::origin::OwnerId>,
    pub include_base_generation: bool,
}

/// A `comptime for` by upstream's iterator protocol, recorded on its
/// iterable.
///
/// The check spelled the iterable as its `__iter__()` call, which MIR lifts
/// as a function over the binders in scope, and `binder` is the
/// `iterator`-typed iterator. `has_next` and `next_iter` are the
/// `paramfor_has_next`/`paramfor_next_iter` applications over the binder,
/// which the elaborator runs to step it; the loop variable is an alias of
/// `next_value`, the `paramfor_next_value` application over it. The body
/// is checked once with the binder symbolic; `native::mono` unrolls it
/// under each instance's bindings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComptimeIteration {
    pub binder: mojito_types::param_expr::ParamRef,
    pub iterator: mojito_types::param_expr::MetaTy,
    pub has_next: mojito_types::param_expr::ParamExpr,
    pub next_iter: mojito_types::param_expr::ParamExpr,
    pub next_value: mojito_types::param_expr::ParamExpr,
}

/// What a `comptime for` iterates, as upstream's `kgen.param.for` steps an
/// iterator.
///
/// From `initial`, the iterable's `__iter__()`, while `has_next` over the
/// binder holds, the binder takes `next_iter` over it. The binder is the
/// iterator, and `next_value` over it the element each copy's variable
/// aliases.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComptimeSequence {
    pub initial: mojito_types::param_expr::ParamExpr,
    pub has_next: mojito_types::param_expr::ParamExpr,
    pub next_iter: mojito_types::param_expr::ParamExpr,
    pub next_value: mojito_types::param_expr::ParamExpr,
}

impl ComptimeSequence {
    /// The meta-type of the loop's binder: the iterator.
    pub fn binder_meta(&self) -> mojito_types::param_expr::MetaTy {
        self.initial.meta().clone()
    }

    /// The loop's stop test, `has_next` over the binder equal to `True`:
    /// the condition of the `comptime if` its body opens with, as a
    /// parameter-valued `Bool` condition is spelled.
    pub fn stop_test(&self) -> mojito_types::types::GenericConstraint {
        use mojito_types::{
            ct::CtValue,
            types::{ConstraintOperand, GenericConstraint},
        };
        GenericConstraint::Eq(
            ConstraintOperand::Expr(self.has_next.clone()),
            ConstraintOperand::Value(CtValue::Bool(true)),
        )
    }

    /// Every parameter expression the sequence holds.
    pub fn expressions(&self) -> Vec<&mojito_types::param_expr::ParamExpr> {
        vec![
            &self.initial,
            &self.has_next,
            &self.next_iter,
            &self.next_value,
        ]
    }

    /// The sequence with each of its expressions mapped through `f`.
    pub fn try_map_exprs<E>(
        &self,
        f: &mut impl FnMut(
            &mojito_types::param_expr::ParamExpr,
        ) -> Result<mojito_types::param_expr::ParamExpr, E>,
    ) -> Result<Self, E> {
        Ok(Self {
            initial: f(&self.initial)?,
            has_next: f(&self.has_next)?,
            next_iter: f(&self.next_iter)?,
            next_value: f(&self.next_value)?,
        })
    }
}

impl std::fmt::Display for ComptimeSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            initial,
            has_next,
            next_iter,
            next_value,
        } = self;
        write!(f, "iter({initial}; {has_next}; {next_iter}; {next_value})")
    }
}

/// What one link of a chained comparison dispatched.
///
/// It is what a lone operator records: the selected dunder's lowered symbol
/// (`None` for a built-in comparison), and whether the link is Equatable's
/// default `!=`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCompareLink {
    pub target: Option<String>,
    pub negated_equality: bool,
}

/// Checker decisions which lowering must apply explicitly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum SemanticAdjustment {
    ResolveCallable(String),
    /// `rebind[dest](x)` over an operand `x` of type `operand`, which only
    /// a generator holds: MIR reads the operand as a value at `dest` through
    /// a value rebind, or retypes its place to `dest`, and the elaborator
    /// asserts the two types equal per instance.
    Rebind {
        operand: Ty,
        dest: Ty,
    },
    /// Construct the concrete runtime type bound to a checked type parameter.
    ConstructTypeParam {
        param: mojito_types::param_expr::ParamRef,
    },
    /// `Ts[i]()` / `Self.Ts[i]()` over a symbolic pack: construct the
    /// element the bracket's index selects from the pack `pack` binds, through
    /// its `Defaultable` initializer.
    ConstructPackElement {
        pack: mojito_types::param_expr::ParamRef,
    },
    /// `FT()` / `types[i]()` over `comptime types = reflect[T].field_types()`:
    /// construct the type the compile-time expression `ty` denotes, through
    /// the `Defaultable` initializer a `conforms_to` arm proved of it. The
    /// elaborator writes the closed type's default construction.
    ConstructType {
        ty: Ty,
    },
    /// `Int(copy=x)`: the `Copyable` initializer of a built-in value type,
    /// whose copy is the read of its source, so MIR lowers the call as its
    /// one keyword argument's value.
    BuiltinCopyConstruction,
    /// `UInt(7)`, `Float64(2.5)`: a built-in scalar constructed from a
    /// literal, upstream's builtin-inline `SIMD.__init__(IntLiteral)`, which
    /// is the literal's conversion. MIR lowers the call as its one argument,
    /// which carries the `MaterializeLiteral` to the constructed type.
    LiteralConstruction,
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
    /// builtin operator path. `place_reference` is the place's own
    /// `ReferenceResult` when the place is a module `def` call returning a
    /// reference (`text(s) += "d"`), which has no selected-call contract to
    /// carry it past this record at the same span.
    AugmentedInPlace {
        call: Box<CheckedCallContract>,
        place_reference: Option<mojito_types::origin::RefTy>,
    },
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
    /// The converted-to checked type of the `ImplicitConversion` at the same
    /// site (the constructed type with its arguments, e.g. `Optional[Int]`),
    /// so the emit-site register is not typed by the constructor's bare name.
    ConversionResultType(Ty),
    /// The error type raised by the `@implicit` constructor of the
    /// `ImplicitConversion` at the same site: the conversion is a raising call.
    ConversionRaises(Ty),
    /// The struct arguments the constructor of the `ImplicitConversion` at
    /// the same site takes, which its declaration restates first.
    ConversionInstantiatedArguments(Vec<mojito_types::types::TyArg>),
    /// Materialize an exact, compile-time-only numeric literal expression into
    /// its checked runtime scalar type.  Keeping this distinct from a user
    /// `@implicit` constructor preserves the language boundary: literal
    /// arithmetic is unbounded, while the resulting scalar has fixed-width
    /// runtime semantics.
    MaterializeLiteral(Ty),
    /// Splat a runtime one-lane value across the checked multi-lane vector
    /// type, upstream's `@implicit SIMD.__init__(Scalar[dtype])`.
    SplatScalar(Ty),
    BorrowShared,
    BorrowMutable,
    /// Constructor arguments bound to origin-solving `ref` parameters: the
    /// constructed aggregate borrows each listed argument's place, so the
    /// binding's loans keep the source alive (a view constructor's whole
    /// contract). Entries are (positional argument index, loan mutability).
    BorrowRefArguments {
        arguments: Vec<(usize, bool)>,
        /// The same call as a temporary receiver of a `ref[self]`-returning
        /// method (`FormatStruct(writer, "P").params(...)`): it is also a
        /// materialized borrow source (see `MaterializeBorrowSource`), and
        /// one span carries one adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
    },
    /// A temporary (non-place) expression bound to a `ref [origin]` parameter
    /// or `ref` binding: the checker materializes it as an anonymous owned
    /// binding with this identity, and lowering stores the value in a hidden
    /// frame slot registered under the same owner so the borrow has a real
    /// place (upstream's temporary lifetime extension — the temporary lives as
    /// long as its borrower).
    MaterializeBorrowSource {
        owner: mojito_types::origin::OwnerId,
        /// The capability the borrow that forced the materialization needs.
        /// Materializing must not change what the loan permits: a read
        /// receiver (`String("a").codepoints()`) lends the hidden slot
        /// immutably, so a second view of the same temporary coexists with
        /// the first exactly as it would over a named local.
        mutable: bool,
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
    /// A call or subscript result that is itself a borrowed view (a Span
    /// sub-slice, a view-returning method): the result binding inherits its
    /// receiver's and lending arguments' loans.
    BorrowViewResult {
        /// The same result as a temporary receiver of a `ref self` method
        /// (`it.peek_next().value()`): it is also a materialized borrow
        /// source (see `MaterializeBorrowSource`), and one span carries one
        /// adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
        /// The owned-interior tags the selected callee's return origin names
        /// below its receiver (`s.rstrip()` names `bytes`); empty when the
        /// result's origin is the receiver itself.
        interior: Vec<String>,
    },
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
    /// This actual argument (or receiver) is an owned temporary (a call
    /// result, a construction, a converted literal) bound to a
    /// read-convention parameter: the callee borrows it, so the caller destroys it once the
    /// call returns. A temporary at a `var`/`deinit` slot moves into the
    /// callee and omits this marker.
    ReadTemporaryArgument,
    /// The owned sibling of `ReadTemporaryArgument`: this actual argument
    /// (or receiver, or operator operand) binds a regular `var`/`deinit`
    /// parameter, which takes the caller's storage. A `^` transfer of a
    /// whole local hands its slot to the callee without the move
    /// initializer, as Mojo's owned argument convention does. A discarded
    /// `^` transfer (`_ = x^`, or `x^` as a statement) carries it too: the
    /// discard destroys the caller's storage without the move initializer.
    ForwardedArgument,
    /// Concrete owner accesses performed by a callable environment during this
    /// call. The checker derives these from the selected callable and any
    /// non-escaping callable arguments; MIR translates stable owner identities
    /// to function-local slots and loan analysis treats them as call effects.
    CallableCaptureAccesses(Vec<mojito_types::origin::CaptureOrigin>),
    /// Every compile-time argument a call of a generic `def` or a
    /// construction solved, in declaration order
    /// ([`GenericInstantiation::arguments`]), those it inferred from its
    /// arguments' types (`size(Counter[4](1))` binds `n = 4`) included: the
    /// call's MIR carries them as compile-time data, so the elaborator binds
    /// a parameter no runtime parameter or result spells and the call emits
    /// no code for one.
    InstantiatedArguments(Vec<mojito_types::types::TyArg>),
    /// The positions, among the call's bracket arguments as written, of those
    /// [`Self::InstantiatedArguments`] carries as closed values
    /// (`tup[(1, Tag(8, "x"))]()`, `s[1 + 2]()`). As Mojo's call carries a parameter
    /// attribute, such an argument has no run-time form at the call site:
    /// MIR lowers no code for it and leaves its slot's register absent.
    FoldedParameterArguments(Vec<usize>),
    /// A consuming method receiver is a source place, but the selected type is
    /// `ImplicitlyCopyable`, so the call consumes a copied value rather than
    /// tombstoning the caller's place. This coexists with parameterized-method
    /// metadata and is consumed directly by MIR lowering.
    ImplicitlyCopyConsumingReceiver,
    /// `a != b` on a struct that declares `__eq__` but no `__ne__`:
    /// Equatable's default `__ne__` is `not (a == b)`, so MIR dispatches
    /// `__eq__` and negates the result.
    NegatedEquality,
    /// `a OP b` answered by the right operand's reflected dunder (`1 + m`
    /// → `m.__radd__(1)`): MIR swaps the operands and `BinOp.resolved`
    /// names the reflected method.
    ReflectedOperator,
    /// The dunder each link of a chained comparison (`a < b < c`) selected,
    /// in link order: MIR lowers every link as its lone operator's call.
    CompareLinks(Vec<CheckedCompareLink>),
    /// `x.write_to(writer)` on a receiver with no `write_to` body of its own
    /// (a `Writable`-bounded type parameter or a built-in Writable value):
    /// receiver and argument swap, so MIR lowers the call as
    /// `writer.write(x)` — the same `Writer.write` shape both backends
    /// already format through the argument's `write_to` conformance.
    InvertedWrite,
    /// `x.write_repr_to(writer)` on a `Writable`-bounded receiver: format the
    /// receiver through `repr(x)`, then feed that text to `writer.write`.
    /// This is the representation twin of [`Self::InvertedWrite`].
    InvertedReprWrite,
    /// An instance method called through its type with the receiver as the
    /// first argument (`List[Int].__len__(xs)`, current Mojo's
    /// `T.method(value, …)` spelling): the checker typed it as
    /// `xs.__len__()`, so MIR lowers the first argument as the receiver and
    /// the rest as the arguments. `inner` keeps the operation adjustment the
    /// rewritten method call itself recorded at the same span (an inverted
    /// write, a parameterized clone selection), reported alongside.
    ReceiverFromFirstArgument {
        inner: Option<Box<Self>>,
    },
    /// `"…".format(args)` on a string literal, which declares no methods
    /// yet: the checker typed the static call of the bundled stand-in
    /// (`STDLIB_FORMAT_UTILS_STRUCT.FORMAT_LITERAL_METHOD`) with the
    /// receiver as its first argument, so MIR lowers that call.
    LiteralFormat,
    /// A condition (`if`/`while`/ternary/comprehension filter) whose value
    /// is a `Boolable` struct: MIR converts it through `Bool(x)`, i.e. the
    /// struct's `__bool__`.
    Truthiness,
    /// A conditional expression whose result type is proven
    /// register-passable: lowering yields it from its `if` as a value, as
    /// upstream's `mergeCValuesAcrossIfLikeOp` does. Any other result is
    /// stored into a result slot by each arm.
    RegisterPassableResult,
    /// A conditional expression whose condition is a parameter value:
    /// lowering builds upstream's `hlcf.comptime.if`, its condition the
    /// recorded `ComptimeCondition` or a thunk, so the elaborator keeps the
    /// live arm alone.
    ComptimeConditional,
    /// A `comptime if` condition the checker compiled as a constraint over
    /// the binders in scope: MIR carries it on the branch, and the elaborator
    /// decides it. A condition with no such record is lowered as a thunk.
    ComptimeCondition(mojito_types::types::GenericConstraint),
    /// A `comptime for` iterable the checker compiled as a range over the
    /// binders in scope, recorded on the iterable: MIR carries it on the
    /// loop header, and the elaborator unrolls it.
    ComptimeIteration(Box<ComptimeIteration>),
    /// The display a local `comptime` binding of a template body holds
    /// (`comptime L = [n, n + 1]`), typed and not closed by the check. The
    /// binding has no runtime form: MIR lifts the display as a function
    /// over the binders in scope at the binding, and binds it at the head of
    /// each function lifted for a compile-time expression that reads it.
    ///
    /// `construction` is the display's own checked construction, which this
    /// record stands in for on the display's span. `sequence` is what the
    /// binding denotes in the parameter domain, a list of its elements: the
    /// application of the function MIR lifts, by the name the check gave
    /// it, to the binders in scope. A loop over the name iterates it, and a
    /// type or a parameter argument reading an element or the length
    /// carries it.
    ComptimeDisplay {
        construction: Box<Self>,
        sequence: mojito_types::param_expr::ParamExpr,
    },
    /// A compile-time `Int` or `Bool` expression of a body that the check
    /// compiles to no parameter expression (`h(L[0])`, `n > 2`), written in
    /// a type or a parameter argument or bound by a local `comptime`. MIR
    /// lifts it as a function over the binders it reads, by the name the
    /// check gave it, and `application` is what the expression denotes in
    /// the parameter domain: that function applied to those binders. Two
    /// spellings of one expression over the same bindings carry one
    /// application, as the pin identifies them by structure.
    ///
    /// `operation` is the expression's own checked operation, which this
    /// record stands beside on the expression's span.
    ComptimeApplication {
        operation: Option<Box<Self>>,
        application: mojito_types::param_expr::ParamExpr,
    },
    Move,
    ExplicitDestroy,
    Iterate(IterationProtocol),
    /// `SIMD[dt, width](elems)`, a `Scalar[dt](x)`, or a scalar alias:
    /// the construction's lane dtype and width, each a known slot or the
    /// parameter expression a generator names, which the elaborator closes
    /// per instance.
    ConstructSimd {
        dtype: SimdDtype,
        width: SimdWidth,
    },
    /// `size_of[T]()` resolved to one concrete checked type. MIR retains the
    /// type so every executable backend consults the shared native layout.
    SizeOf {
        ty: Ty,
    },
    /// `__param_list_address[*values]()` in the bundled `ParameterList`: the
    /// static address of the parameter list `values`' elements, which MIR
    /// keeps as `MirInstr::ParamListAddress` for the elaborator to close.
    ParamListAddress {
        values: mojito_types::param_expr::ParamExpr,
    },
    /// `_unqualified_type_name[T]()` resolved to upstream's unqualified
    /// spelling of the checked type (`SIMD[DType.int, 1]`, `Optional[String]`);
    /// MIR lowers `text` to a string constant. The type itself is kept beside
    /// the spelling so an instance of a checked template re-renders it from
    /// the substituted type rather than inheriting the template's wording.
    TypeName {
        text: String,
        ty: Ty,
    },
    /// `v.cast[DType.target]()` — elementwise dtype conversion of a SIMD
    /// value. The target dtype and lane width are the checker's slots, known
    /// or a generator's parameter expressions; MIR carries them so the VM
    /// never derives semantics from a runtime value.
    SimdCast {
        dtype: SimdDtype,
        width: SimdWidth,
    },
    /// `v.to_bits[DType.target]()` — lane-wise bit reinterpretation of a
    /// SIMD value as the unsigned `dtype` (at least as wide as the source
    /// lane; narrower sources zero-extend). Resolved at checking like
    /// `SimdCast`.
    SimdToBits {
        dtype: SimdDtype,
        width: SimdWidth,
    },
    /// `v.length` on a SIMD value — the lane count, lowered as an `Int`
    /// constant when known and as the evaluation of its parameter
    /// expression in a generator.
    SimdLength {
        width: SimdWidth,
    },
    /// `DType.<name>` read as a runtime value, or the `dtype` of a `SIMD` type
    /// or value (`Int32.dtype`, `v.dtype`) — the dtype, lowered as a dtype
    /// constant when known and as a read of its binder in a generator. A
    /// value receiver still lowers for its effects.
    DtypeConstant {
        dtype: SimdDtype,
    },
    /// A compile-time query read as a runtime value — `Ts.length`,
    /// `len(Ts)`, `Ts.contains[X]()` of a `def`'s own pack, or a `DType`
    /// float-format query (`DType.mantissa_width[dt]()`) — lowered as a
    /// constant of the parameter expression, which the elaborator folds per
    /// instance. A field chain of a compile-time binding (`q.a` in
    /// `def f[q: Q]()`) is one too: upstream's struct extract on the
    /// parameter attribute, never a read of the whole struct.
    ParamValue {
        value: mojito_types::param_expr::ParamExpr,
        /// The same read as a borrowed temporary (`show(q.i)`): it is also a
        /// materialized borrow source (see `MaterializeBorrowSource`), and
        /// one span carries one adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
    },
    /// `v.shuffle[*mask]()`, `v.slice[width, offset=o]()`, and `v.join(w)` —
    /// lane gathers by a compile-time mask; the result takes the mask's
    /// (power-of-two) width. A shuffle's mask has one index per receiver
    /// lane. A `joined` gather indexes the receiver's lanes followed by the
    /// argument's. The mask is known where the receiver width and the
    /// gather's arguments are; a template's keeps its method's form, which
    /// each instance closes.
    SimdShuffle {
        mask: mojito_types::types::LaneMask,
        joined: bool,
    },
    /// Construct the selected alternative of a checked `Variant` type.
    ConstructVariant {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
    },
    /// Placement construction (`Variant[...](init_with=factory)`): the
    /// zero-parameter factory's result becomes the payload of the alternative
    /// selected by its return type (no `Movable` requirement).
    ConstructVariantInitWith {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
        /// The factory's result type, the selected alternative.
        payload: Ty,
    },
    /// A collection display/comprehension resolved to a nominal constructor and
    /// insertion protocol. MIR lowers this as ordinary construction plus exact
    /// method calls; it must not reconstruct a native collection from syntax.
    ConstructCollection {
        target: Ty,
        insert: Option<String>,
        /// The same display as a temporary receiver of a `ref self` method
        /// (`(1, 2)[0]`): it is also a materialized borrow source (see
        /// `MaterializeBorrowSource`), and one span carries one adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
        /// A non-empty dictionary display's literal constructor: the display
        /// is `Dict(keys=[k…], values=[v…], __dict_literal__=None)`, built
        /// from the key and value lists rather than by insertion.
        literal: Option<Box<DictLiteral>>,
    },
    /// A fixed-size list display resolved to `Array`'s variadic literal
    /// constructor. MIR lowers this as one nominal constructor call carrying
    /// the element registers and the concrete `length` parameter argument;
    /// `constructor` is the exact lowered overload symbol.
    ConstructArrayLiteral {
        target: Ty,
        constructor: String,
        /// The same display as a temporary receiver of a `ref self` method
        /// (`[1, 2][0]`): it is also a materialized borrow source (see
        /// `MaterializeBorrowSource`), and one span carries one adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
    },
    /// Test the active runtime tag (`value.isa[T]()`).
    VariantIs {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
    },
    /// Checked typed projection (`value[T]`).
    VariantProject {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
    },
    /// Replace the active alternative (`value.set[T](new_value)`).
    VariantSet {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
    },
    /// Consume a variant and move out the selected payload. `checked` controls
    /// whether execution validates the active tag (`take` versus `unsafe_take`).
    VariantTake {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
        checked: bool,
    },
    /// Atomically install `input_index` and move out `output_index`.
    VariantReplace {
        alternatives: Vec<Ty>,
        input_index: mojito_types::types::VariantIndex,
        output_index: mojito_types::types::VariantIndex,
        checked: bool,
    },
    /// The `__mlir_op` statement `lit.ownership.mark_initialized` over
    /// `__get_mvalue_as_litref(place)`: it marks a place initialized, after
    /// which its storage is written through pointers (`Tuple.__init__`). MIR
    /// carries it as `MarkInitialized` over the place.
    MarkInitialized,
    /// The `__mlir_op` statement `lit.ownership.mark_destroyed` over
    /// `__get_mvalue_as_litref(place)`: it ends a place's value without
    /// destroying it, after its storage was moved out through pointers
    /// (`Tuple.consume_elements`). MIR carries it as `MarkDestroyed` over
    /// the place.
    MarkDestroyed,
    /// A sugar written as the construction it stands for: an initializer
    /// list `{}` / `{a, b}` at the type its context expects (upstream's
    /// set-initializer literal, emitted as the construction `T(a, b)` of that
    /// type), or a template string `t"…"` (emitted as the call of the bundled
    /// `__make_tstring` over its segments and interpolations). The checker
    /// spells and checks the construction under an identity derived from
    /// the sugar's, so the sugar keeps the type its context sees and the
    /// construction its own facts; the checked arena hands HIR the
    /// construction in the sugar's place.
    SpelledConstruction {
        construction: SpelledConstruction,
    },
    /// Construct an origin-bearing pointer to existing checked storage
    /// (`UnsafePointer(to=place)`). The checked pointer type retains the
    /// inferred place origin; execution represents the value as a frame/slot
    /// handle and erases the origin.
    PointerToPlace {
        mutable: bool,
        /// The pointer as the holder of a dereference place
        /// (`Pointer(to=x)[]`): also a materialized borrow source (see
        /// `MaterializeBorrowSource`), and one span carries one adjustment.
        materialized: Option<mojito_types::origin::OwnerId>,
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
    PointerOffset {
        /// A materialized dereference holder, as on `PointerToPlace`.
        materialized: Option<mojito_types::origin::OwnerId>,
    },
    /// `pointer.unsafe_origin_cast[...]()`: a checked provenance rebind. The runtime
    /// value is unchanged, so lowering forwards the receiver register; the
    /// resolved target origin is retained so aggregate-origin bookkeeping can
    /// carry the rebound loan without re-resolving the parameter argument.
    PointerOriginCast {
        origin: mojito_types::origin::PointerOrigin,
        /// A materialized dereference holder, as on `PointerToPlace`.
        materialized: Option<mojito_types::origin::OwnerId>,
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
        index: mojito_types::types::VariantIndex,
    },
    /// `variant.deinit_with(handler)`: consume the variant, handing the
    /// active payload to the single-parameter consuming handler. The handler
    /// is monomorphic for exactly one alternative (`index`); a runtime tag
    /// mismatch aborts, matching upstream's contract.
    VariantDeinitWith {
        alternatives: Vec<Ty>,
        index: mojito_types::types::VariantIndex,
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

/// The constructors a dictionary display lowers through.
///
/// See [`SemanticAdjustment::ConstructCollection`]: the exact lowered symbols
/// of `Dict`'s `__dict_literal__` overload and of `List`'s `__list_literal__`
/// overload, which builds the key and value lists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DictLiteral {
    pub constructor: String,
    pub list_constructor: String,
}

/// The construction a sugar checked as.
///
/// See [`SemanticAdjustment::SpelledConstruction`]. Equality is the expression's
/// structural equality, an equivalence on parsed syntax (a literal is never
/// NaN).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpelledConstruction(pub Box<Expr>);

impl Eq for SpelledConstruction {}

/// One element of a tuple being destructured: how it is read from the
/// tuple, and, when the target nests a tuple at that position, the plan
/// for its own elements.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedTupleUnpackElement {
    pub ty: Ty,
    pub accessor: Option<String>,
    /// The accessor's own compile-time parameters, which the element's
    /// index binds (`Tuple.__getitem_param__[index: Int]`).
    pub param_decls: Vec<mojito_types::types::ParamDecl>,
    /// The owner of the hidden slot an unpacked temporary lives in while
    /// its elements are read through references into it.
    pub temporary: Option<mojito_types::origin::OwnerId>,
    pub reference: Option<mojito_types::origin::RefTy>,
    /// The element's storage carries owner loans (a view or other
    /// origin-bearing value), so its binding inherits the unpacked value's
    /// loans the way an ordinary binding of the whole value does.
    pub carries_loans: bool,
    /// The plan for the element's own elements when the target nests a
    /// tuple there; empty for a leaf target.
    #[serde(default)]
    pub elements: Vec<Self>,
}

/// One expression in the typed semantic arena.
///
/// A node holds its own operation, as an upstream op does: its opcode and
/// attributes, with each operand the node of a child expression, never a
/// copy of the child's syntax. Its location is its own occurrence.
///
/// It encodes as an MLIR bytecode op does: an encoding mask naming the
/// attributes it holds, then its operation and location, then those
/// attributes alone (see `encoding`).
#[derive(Debug, Clone)]
pub struct CheckedExpr {
    /// Its index in its tables, which an encoding leaves to its position.
    pub id: CheckedNodeId,
    pub op: CheckedOp,
    pub span: Span,
    pub source: Option<String>,
    pub syntax_id: SyntaxId,
    pub ty: Option<Ty>,
    /// Type physically stored at this place. For a reference-valued field this
    /// is `TyKind::Ref`, while `ty` is the referent produced by an ordinary read.
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
    /// Stable identities and storage types for generator binders introduced by
    /// a collection comprehension. Entries are in source `for`-clause order.
    /// Keeping these on the checked node prevents MIR from accidentally
    /// interning a binder by its spelling and aliasing an outer local.
    pub comprehension_bindings: Vec<CheckedComprehensionBinding>,
    /// The caller-substituted loan transfers of the call this node is.
    pub call_transfers: Vec<CheckedCallTransfer>,
}

impl CheckedExpr {
    /// The occurrence this node was checked at.
    pub fn location(&self) -> SourceSpan {
        SourceSpan::syntax(self.source.clone(), self.span, self.syntax_id)
    }
}

/// A checked expression's own operation.
///
/// It is the node's [`ExprKind`], with each child expression replaced by
/// the child's node. A type operand is the check's resolved attribute
/// ([`CheckedParamArg::Attribute`]), never the annotation as written. A
/// lambda names its hidden `def`'s statement node, as upstream's closure
/// names the function it closes over; the node's `binding` is that
/// function's identity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedOp {
    Int(IntLiteral),
    Float(FloatLiteral),
    Bool(bool),
    Str(String),
    None,
    Uninitialized,
    EmptySubscript,
    Identifier(String),
    /// A type in value position, as the check resolved it (`None` where
    /// the check resolved none), with the nodes of the applications the
    /// check lifted out of it ([`CheckedArenaBuilder::type_applications`]).
    TypeValue {
        ty: Option<Ty>,
        applications: Vec<CheckedNodeId>,
    },
    Prefix(PrefixOp, CheckedNodeId),
    Infix(InfixOp, CheckedNodeId, CheckedNodeId),
    Call {
        name: String,
        param_args: Vec<CheckedParamArg>,
        args: Vec<CheckedNodeId>,
        kwargs: Vec<CheckedKwArg>,
    },
    Invoke {
        callee: CheckedNodeId,
        param_args: Vec<CheckedParamArg>,
        args: Vec<CheckedNodeId>,
        kwargs: Vec<CheckedKwArg>,
    },
    Member {
        object: CheckedNodeId,
        field: String,
    },
    MethodCall {
        object: CheckedNodeId,
        method: String,
        args: Vec<CheckedNodeId>,
        kwargs: Vec<CheckedKwArg>,
    },
    Index {
        object: CheckedNodeId,
        index: CheckedNodeId,
    },
    TypeApply {
        name: String,
        args: Vec<CheckedParamArg>,
    },
    Spread(CheckedNodeId),
    Transfer(CheckedNodeId),
    ListLit(Vec<CheckedNodeId>),
    BraceLit(Vec<(CheckedNodeId, Option<CheckedNodeId>)>),
    Comprehension {
        kind: CollectionKind,
        key: Option<CheckedNodeId>,
        value: CheckedNodeId,
        clauses: Vec<CheckedComprehensionClause>,
    },
    Lambda {
        def: CheckedStmtId,
    },
    TupleLit(Vec<CheckedNodeId>),
    Named {
        name: String,
        value: CheckedNodeId,
    },
    IfExpr {
        cond: CheckedNodeId,
        then_branch: CheckedNodeId,
        else_branch: CheckedNodeId,
    },
    Compare {
        first: CheckedNodeId,
        rest: Vec<(InfixOp, CheckedNodeId)>,
    },
    Slice {
        object: CheckedNodeId,
        lower: Option<CheckedNodeId>,
        upper: Option<CheckedNodeId>,
        step: Option<CheckedNodeId>,
    },
    MultiIndex {
        object: CheckedNodeId,
        args: Vec<CheckedSubscriptArg>,
    },
    TString {
        parts: Vec<CheckedTStringPart>,
        raw: bool,
    },
}

impl CheckedOp {
    /// The operand nodes, in evaluation order: a comprehension's clauses
    /// before its key and value, a call's callee before its compile-time
    /// value arguments, its arguments, and its keyword arguments.
    pub fn operands(&self) -> Vec<CheckedNodeId> {
        let mut operands = Vec::new();
        let kwargs = |operands: &mut Vec<CheckedNodeId>, kwargs: &[CheckedKwArg]| {
            operands.extend(kwargs.iter().map(|kwarg| kwarg.value));
        };
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::Str(_)
            | Self::None
            | Self::Uninitialized
            | Self::EmptySubscript
            | Self::Identifier(_)
            | Self::TypeValue { .. }
            | Self::Lambda { .. } => {}
            Self::Prefix(_, value)
            | Self::Transfer(value)
            | Self::Spread(value)
            | Self::Named { value, .. }
            | Self::Member { object: value, .. } => operands.push(*value),
            Self::Infix(_, left, right)
            | Self::Index {
                object: left,
                index: right,
            } => operands.extend([*left, *right]),
            Self::Call {
                param_args,
                args,
                kwargs: keywords,
                ..
            } => {
                for argument in param_args {
                    argument.operands(&mut operands);
                }
                operands.extend(args);
                kwargs(&mut operands, keywords);
            }
            Self::Invoke {
                callee,
                param_args,
                args,
                kwargs: keywords,
            } => {
                operands.push(*callee);
                for argument in param_args {
                    argument.operands(&mut operands);
                }
                operands.extend(args);
                kwargs(&mut operands, keywords);
            }
            Self::MethodCall {
                object,
                args,
                kwargs: keywords,
                ..
            } => {
                operands.push(*object);
                operands.extend(args);
                kwargs(&mut operands, keywords);
            }
            Self::TypeApply { args, .. } => {
                for argument in args {
                    argument.operands(&mut operands);
                }
            }
            Self::ListLit(values) | Self::TupleLit(values) => operands.extend(values),
            Self::BraceLit(entries) => {
                for (key, value) in entries {
                    operands.push(*key);
                    operands.extend(value);
                }
            }
            Self::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                operands.extend(clauses.iter().map(|clause| match clause {
                    CheckedComprehensionClause::For { iter, .. } => *iter,
                    CheckedComprehensionClause::If(condition) => *condition,
                }));
                operands.extend(key);
                operands.push(*value);
            }
            Self::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => operands.extend([*cond, *then_branch, *else_branch]),
            Self::Compare { first, rest } => {
                operands.push(*first);
                operands.extend(rest.iter().map(|(_, value)| *value));
            }
            Self::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                operands.push(*object);
                operands.extend([lower, upper, step].into_iter().flatten());
            }
            Self::MultiIndex { object, args } => {
                operands.push(*object);
                for argument in args {
                    match argument {
                        CheckedSubscriptArg::Index(value)
                        | CheckedSubscriptArg::Keyword { value, .. } => operands.push(*value),
                        CheckedSubscriptArg::Slice {
                            lower, upper, step, ..
                        }
                        | CheckedSubscriptArg::KeywordSlice {
                            lower, upper, step, ..
                        } => operands.extend([lower, upper, step].into_iter().flatten()),
                    }
                }
            }
            Self::TString { parts, .. } => {
                operands.extend(parts.iter().filter_map(|part| match part {
                    CheckedTStringPart::Expr(value) => Some(*value),
                    CheckedTStringPart::Literal(_) => None,
                }));
            }
        }
        operands
    }

    /// The nodes the op holds in source order: its operands, with the
    /// lifted applications of each type it spells where the type is written.
    fn reached(&self) -> Vec<CheckedNodeId> {
        let mut nodes = Vec::new();
        match self {
            Self::TypeValue { applications, .. } => nodes.extend(applications),
            Self::Call {
                param_args,
                args,
                kwargs,
                ..
            }
            | Self::Invoke {
                param_args,
                args,
                kwargs,
                ..
            } => {
                if let Self::Invoke { callee, .. } = self {
                    nodes.push(*callee);
                }
                for argument in param_args {
                    argument.reached(&mut nodes);
                }
                nodes.extend(args);
                nodes.extend(kwargs.iter().map(|kwarg| kwarg.value));
            }
            Self::TypeApply { args, .. } => {
                for argument in args {
                    argument.reached(&mut nodes);
                }
            }
            _ => nodes = self.operands(),
        }
        nodes
    }

    /// The op with each node it names replaced by `map`'s, visited in the
    /// order the check's nodes are built: operands first, then a lambda's
    /// hidden `def`.
    pub(crate) fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        let values = |map: &mut dyn NodeMap, values: &[CheckedNodeId]| -> Vec<CheckedNodeId> {
            values.iter().map(|value| map.expression(*value)).collect()
        };
        let optional = |map: &mut dyn NodeMap, value: &Option<CheckedNodeId>| {
            value.map(|value| map.expression(value))
        };
        let kwargs = |map: &mut dyn NodeMap, kwargs: &[CheckedKwArg]| {
            kwargs
                .iter()
                .map(|kwarg| CheckedKwArg {
                    name: kwarg.name.clone(),
                    value: map.expression(kwarg.value),
                })
                .collect()
        };
        let param_args = |map: &mut dyn NodeMap, arguments: &[CheckedParamArg]| {
            arguments
                .iter()
                .map(|argument| argument.map_operands(map))
                .collect()
        };
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool(_)
            | Self::Str(_)
            | Self::None
            | Self::Uninitialized
            | Self::EmptySubscript
            | Self::Identifier(_) => self.clone(),
            Self::TypeValue { ty, applications } => Self::TypeValue {
                ty: ty.clone(),
                applications: values(map, applications),
            },
            Self::Prefix(op, value) => Self::Prefix(*op, map.expression(*value)),
            Self::Transfer(value) => Self::Transfer(map.expression(*value)),
            Self::Spread(value) => Self::Spread(map.expression(*value)),
            Self::Named { name, value } => Self::Named {
                name: name.clone(),
                value: map.expression(*value),
            },
            Self::Infix(op, left, right) => {
                let left = map.expression(*left);
                Self::Infix(*op, left, map.expression(*right))
            }
            Self::Index { object, index } => Self::Index {
                object: map.expression(*object),
                index: map.expression(*index),
            },
            Self::Call {
                name,
                param_args: arguments,
                args,
                kwargs: keywords,
            } => Self::Call {
                name: name.clone(),
                param_args: param_args(map, arguments),
                args: values(map, args),
                kwargs: kwargs(map, keywords),
            },
            Self::Invoke {
                callee,
                param_args: arguments,
                args,
                kwargs: keywords,
            } => Self::Invoke {
                callee: map.expression(*callee),
                param_args: param_args(map, arguments),
                args: values(map, args),
                kwargs: kwargs(map, keywords),
            },
            Self::Member { object, field } => Self::Member {
                object: map.expression(*object),
                field: field.clone(),
            },
            Self::MethodCall {
                object,
                method,
                args,
                kwargs: keywords,
            } => Self::MethodCall {
                object: map.expression(*object),
                method: method.clone(),
                args: values(map, args),
                kwargs: kwargs(map, keywords),
            },
            Self::TypeApply { name, args } => Self::TypeApply {
                name: name.clone(),
                args: param_args(map, args),
            },
            Self::ListLit(items) => Self::ListLit(values(map, items)),
            Self::TupleLit(items) => Self::TupleLit(values(map, items)),
            Self::BraceLit(entries) => Self::BraceLit(
                entries
                    .iter()
                    .map(|(key, value)| (map.expression(*key), optional(map, value)))
                    .collect(),
            ),
            Self::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => {
                let clauses = clauses
                    .iter()
                    .map(|clause| match clause {
                        CheckedComprehensionClause::For { var, binding, iter } => {
                            CheckedComprehensionClause::For {
                                var: var.clone(),
                                binding: *binding,
                                iter: map.expression(*iter),
                            }
                        }
                        CheckedComprehensionClause::If(condition) => {
                            CheckedComprehensionClause::If(map.expression(*condition))
                        }
                    })
                    .collect();
                Self::Comprehension {
                    kind: *kind,
                    key: optional(map, key),
                    value: map.expression(*value),
                    clauses,
                }
            }
            Self::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => Self::IfExpr {
                cond: map.expression(*cond),
                then_branch: map.expression(*then_branch),
                else_branch: map.expression(*else_branch),
            },
            Self::Compare { first, rest } => Self::Compare {
                first: map.expression(*first),
                rest: rest
                    .iter()
                    .map(|(op, value)| (*op, map.expression(*value)))
                    .collect(),
            },
            Self::Slice {
                object,
                lower,
                upper,
                step,
            } => Self::Slice {
                object: map.expression(*object),
                lower: optional(map, lower),
                upper: optional(map, upper),
                step: optional(map, step),
            },
            Self::MultiIndex { object, args } => Self::MultiIndex {
                object: map.expression(*object),
                args: args
                    .iter()
                    .map(|argument| match argument {
                        CheckedSubscriptArg::Index(value) => {
                            CheckedSubscriptArg::Index(map.expression(*value))
                        }
                        CheckedSubscriptArg::Keyword { name, value } => {
                            CheckedSubscriptArg::Keyword {
                                name: name.clone(),
                                value: map.expression(*value),
                            }
                        }
                        CheckedSubscriptArg::Slice { lower, upper, step } => {
                            CheckedSubscriptArg::Slice {
                                lower: optional(map, lower),
                                upper: optional(map, upper),
                                step: optional(map, step),
                            }
                        }
                        CheckedSubscriptArg::KeywordSlice {
                            name,
                            lower,
                            upper,
                            step,
                        } => CheckedSubscriptArg::KeywordSlice {
                            name: name.clone(),
                            lower: optional(map, lower),
                            upper: optional(map, upper),
                            step: optional(map, step),
                        },
                    })
                    .collect(),
            },
            Self::TString { parts, raw } => Self::TString {
                parts: parts
                    .iter()
                    .map(|part| match part {
                        CheckedTStringPart::Literal(text) => {
                            CheckedTStringPart::Literal(text.clone())
                        }
                        CheckedTStringPart::Expr(value) => {
                            CheckedTStringPart::Expr(map.expression(*value))
                        }
                    })
                    .collect(),
                raw: *raw,
            },
            Self::Lambda { def } => Self::Lambda {
                def: map.statement(*def),
            },
        }
    }
}

/// A compile-time parameter argument of a checked call or type application.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedParamArg {
    /// A bracket argument written in type position, as the check resolved
    /// it: a type, a compile-time value the spelling names (`Self.n`, a
    /// value-pack spread, a display element `L[0]`), or an origin; `None`
    /// where the check resolved none. Or one written as a value and bound
    /// to a type or an origin (`List[String]`, `Pointer[Int, origin_of(x)]`).
    /// With the nodes of the applications the check lifted out of it.
    Attribute {
        value: Option<mojito_types::types::TyArg>,
        applications: Vec<CheckedNodeId>,
    },
    Value(CheckedNodeId),
    Named {
        name: String,
        value: Box<Self>,
    },
}

impl CheckedParamArg {
    fn operands(&self, operands: &mut Vec<CheckedNodeId>) {
        match self {
            Self::Attribute { .. } => {}
            Self::Value(value) => operands.push(*value),
            Self::Named { value, .. } => value.operands(operands),
        }
    }

    fn reached(&self, nodes: &mut Vec<CheckedNodeId>) {
        match self {
            Self::Attribute { applications, .. } => nodes.extend(applications),
            Self::Value(value) => nodes.push(*value),
            Self::Named { value, .. } => value.reached(nodes),
        }
    }

    fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        match self {
            Self::Attribute {
                value,
                applications,
            } => Self::Attribute {
                value: value.clone(),
                applications: applications
                    .iter()
                    .map(|application| map.expression(*application))
                    .collect(),
            },
            Self::Value(value) => Self::Value(map.expression(*value)),
            Self::Named { name, value } => Self::Named {
                name: name.clone(),
                value: Box::new(value.map_operands(map)),
            },
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedKwArg {
    pub name: String,
    pub value: CheckedNodeId,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedSubscriptArg {
    Index(CheckedNodeId),
    Keyword {
        name: String,
        value: CheckedNodeId,
    },
    Slice {
        lower: Option<CheckedNodeId>,
        upper: Option<CheckedNodeId>,
        step: Option<CheckedNodeId>,
    },
    KeywordSlice {
        name: String,
        lower: Option<CheckedNodeId>,
        upper: Option<CheckedNodeId>,
        step: Option<CheckedNodeId>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedComprehensionClause {
    For {
        var: String,
        binding: BindingMode,
        iter: CheckedNodeId,
    },
    If(CheckedNodeId),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedTStringPart {
    Literal(String),
    Expr(CheckedNodeId),
}

/// One statement in the typed semantic arena.
///
/// A statement is an op too: it holds its own operation, with each child
/// expression its node and each body a list of statement nodes, so control
/// flow stays structured as upstream's `hlcf` keeps it. Its location is its
/// own occurrence. It encodes as [`CheckedExpr`] does, behind an encoding
/// mask.
#[derive(Debug, Clone)]
pub struct CheckedStmt {
    /// Its index in its tables, which an encoding leaves to its position.
    pub id: CheckedStmtId,
    pub op: CheckedStmtOp,
    pub span: Span,
    pub module: Option<String>,
    pub syntax_id: SyntaxId,
    /// The binding the statement introduces, or the one an assignment to a
    /// name writes.
    pub binding: Option<mojito_types::origin::OwnerId>,
    /// The checked type of a binding the statement introduces.
    pub binding_ty: Option<Ty>,
    /// The declaration the statement makes.
    pub declaration: Option<CheckedDeclId>,
}

impl CheckedStmt {
    /// The occurrence this node was checked at.
    pub fn location(&self) -> SourceSpan {
        SourceSpan::syntax(self.module.clone(), self.span, self.syntax_id)
    }
}

/// Stable identity of a statement within one checked program.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct CheckedStmtId(pub u32);

/// A checked statement's own operation.
///
/// It is the statement's [`StmtKind`](mojito_ast::ast::StmtKind), with each
/// child expression replaced by its node and each child statement list by
/// its statements' nodes. A declaration holds what the check resolved for
/// it, as upstream's function op holds its symbol and its resolved
/// function type: its symbol and its [`CheckedSignature`], and a struct
/// its fields' checked types. A binding's type is the statement's
/// [`CheckedStmt::binding_ty`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedStmtOp {
    VarDecl {
        name: String,
        /// The nodes of the applications the check lifted out of `ty`.
        applications: Vec<CheckedNodeId>,
        value: CheckedNodeId,
    },
    RefDecl {
        name: String,
        value: CheckedNodeId,
    },
    Assign {
        name: String,
        value: CheckedNodeId,
    },
    AugAssign {
        place: CheckedNodeId,
        op: InfixOp,
        value: CheckedNodeId,
    },
    /// A destructuring assignment: the value's pieces go to the target's
    /// leaves. The value node carries the extraction plan
    /// ([`SemanticAdjustment::TupleUnpack`]).
    Destructure {
        target: CheckedTarget,
        value: CheckedNodeId,
    },
    SetPlace {
        place: CheckedNodeId,
        value: CheckedNodeId,
    },
    Def(Box<CheckedDef>),
    Struct(Box<CheckedStruct>),
    Trait(Box<CheckedTrait>),
    Comptime {
        name: String,
        where_clauses: Vec<CheckedNodeId>,
        value: CheckedNodeId,
        /// The application a local binding denotes, as upstream's alias
        /// attribute: the binding has no storage, and each read is a
        /// materialization of it.
        denotes: Option<mojito_types::param_expr::ParamExpr>,
    },
    ComptimeIf {
        branches: Vec<(CheckedCondition, Vec<CheckedStmtId>)>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    ComptimeFor {
        var: String,
        iter: CheckedNodeId,
        body: Vec<CheckedStmtId>,
    },
    /// `comptime assert cond[, message]`: the condition's node carries its
    /// [`SemanticAdjustment::ComptimeCondition`], judged per instance by the
    /// elaborator.
    ComptimeAssert {
        cond: CheckedNodeId,
        message: Option<String>,
    },
    /// `comptime match`: the subject's node, then each case's pattern
    /// nodes, guard, and body.
    ComptimeMatch {
        subject: CheckedNodeId,
        cases: Vec<CheckedMatchCase>,
    },
    If {
        branches: Vec<(CheckedCondition, Vec<CheckedStmtId>)>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    While {
        cond: CheckedCondition,
        body: Vec<CheckedStmtId>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    /// `match`: the statement anchoring the subject (`ref $match<id> =
    /// subject`, the check's own declaration, evaluated once), then each
    /// case's pattern nodes, commands, guard, and body. `None` when the
    /// check recorded no plan.
    Match {
        anchor: Option<CheckedStmtId>,
        cases: Vec<CheckedMatchCase>,
    },
    For {
        var: String,
        binding: BindingMode,
        iter: CheckedNodeId,
        body: Vec<CheckedStmtId>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    Return(Option<CheckedNodeId>),
    Raise(CheckedNodeId),
    Import {
        path: Vec<String>,
        alias: Option<String>,
    },
    FromImport {
        level: usize,
        path: Vec<String>,
        names: ImportNames,
    },
    With {
        items: Vec<CheckedWithItem>,
        body: Vec<CheckedStmtId>,
    },
    Try {
        body: Vec<CheckedStmtId>,
        except: Option<(Option<String>, Vec<CheckedStmtId>)>,
        orelse: Option<Vec<CheckedStmtId>>,
        finalbody: Option<Vec<CheckedStmtId>>,
    },
    Scope(Vec<CheckedStmtId>),
    Pass,
    Break,
    Continue,
    Expr(CheckedNodeId),
}

impl CheckedStmtOp {
    /// The statement's own expression operands, in checked order; a nested
    /// body's are its statements'.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        let params = |params: &[CheckedFnParam]| {
            params
                .iter()
                .filter_map(|param| param.default)
                .collect::<Vec<_>>()
        };
        match self {
            Self::VarDecl { value, .. }
            | Self::RefDecl { value, .. }
            | Self::Assign { value, .. }
            | Self::Raise(value)
            | Self::Expr(value) => vec![*value],
            Self::AugAssign { place, value, .. } | Self::SetPlace { place, value } => {
                vec![*place, *value]
            }
            Self::Destructure { target, value } => {
                target.expressions().into_iter().chain([*value]).collect()
            }
            Self::Match { cases, .. } => cases
                .iter()
                .flat_map(CheckedMatchCase::expressions)
                .collect(),
            Self::ComptimeMatch { subject, cases } => std::iter::once(*subject)
                .chain(cases.iter().flat_map(CheckedMatchCase::expressions))
                .collect(),
            Self::Comptime {
                where_clauses,
                value,
                ..
            } => where_clauses.iter().copied().chain([*value]).collect(),
            Self::Def(def) => {
                let mut operands = params(&def.params);
                operands.extend(&def.where_clauses);
                operands
            }
            Self::Struct(declaration) => {
                let mut operands: Vec<_> = declaration
                    .conformance_conditions
                    .iter()
                    .map(|(_, value)| *value)
                    .chain(declaration.where_clauses.iter().copied())
                    .collect();
                for member in &declaration.associated {
                    operands.extend(&member.where_clauses);
                    operands.push(member.value);
                }
                for method in &declaration.methods {
                    operands.extend(params(&method.params));
                    operands.extend(&method.where_clauses);
                }
                operands
            }
            Self::Trait(declaration) => {
                let mut operands = Vec::new();
                for method in &declaration.methods {
                    operands.extend(params(&method.params));
                    operands.extend(&method.where_clauses);
                }
                for member in &declaration.comptime_members {
                    operands.extend(&member.where_clauses);
                }
                operands
            }
            Self::ComptimeIf { branches, .. } | Self::If { branches, .. } => branches
                .iter()
                .flat_map(|(condition, _)| condition.expressions())
                .collect(),
            Self::ComptimeFor { iter, .. } | Self::For { iter, .. } => vec![*iter],
            Self::ComptimeAssert { cond, .. } => vec![*cond],
            Self::While { cond, .. } => cond.expressions(),
            Self::Return(value) => value.iter().copied().collect(),
            Self::With { items, .. } => items.iter().map(|item| item.context).collect(),
            Self::Import { .. }
            | Self::FromImport { .. }
            | Self::Try { .. }
            | Self::Scope(_)
            | Self::Pass
            | Self::Break
            | Self::Continue => Vec::new(),
        }
    }

    /// The statement's nested statement lists, in source order: a `def`'s
    /// body, a struct's method bodies, a trait's default members' `def`
    /// nodes, a branch, loop, `with`, `try`, or scope body.
    pub fn bodies(&self) -> Vec<&[CheckedStmtId]> {
        match self {
            Self::Def(def) => vec![def.body.as_slice()],
            Self::Struct(declaration) => declaration
                .methods
                .iter()
                .map(|method| method.body.as_slice())
                .collect(),
            Self::Trait(declaration) => declaration
                .methods
                .iter()
                .filter_map(|method| method.default.as_ref().map(std::slice::from_ref))
                .collect(),
            Self::ComptimeIf { branches, orelse } | Self::If { branches, orelse } => branches
                .iter()
                .flat_map(|(condition, body)| {
                    condition
                        .statements()
                        .into_iter()
                        .chain(std::iter::once(body.as_slice()))
                })
                .chain(orelse.as_deref())
                .collect(),
            Self::While { cond, body, orelse } => cond
                .statements()
                .into_iter()
                .chain(std::iter::once(body.as_slice()))
                .chain(orelse.as_deref())
                .collect(),
            Self::For { body, orelse, .. } => std::iter::once(body.as_slice())
                .chain(orelse.as_deref())
                .collect(),
            Self::Match { anchor, cases } => anchor
                .iter()
                .map(std::slice::from_ref)
                .chain(cases.iter().flat_map(|case| {
                    case.statements()
                        .into_iter()
                        .chain(std::iter::once(case.body.as_slice()))
                }))
                .collect(),
            Self::ComptimeMatch { cases, .. } => {
                cases.iter().map(|case| case.body.as_slice()).collect()
            }
            Self::Try {
                body,
                except,
                orelse,
                finalbody,
            } => std::iter::once(body.as_slice())
                .chain(except.as_ref().map(|(_, body)| body.as_slice()))
                .chain(orelse.as_deref())
                .chain(finalbody.as_deref())
                .collect(),
            Self::ComptimeFor { body, .. } | Self::With { body, .. } | Self::Scope(body) => {
                vec![body.as_slice()]
            }
            Self::VarDecl { .. }
            | Self::RefDecl { .. }
            | Self::Assign { .. }
            | Self::AugAssign { .. }
            | Self::Destructure { .. }
            | Self::SetPlace { .. }
            | Self::Comptime { .. }
            | Self::ComptimeAssert { .. }
            | Self::Return(_)
            | Self::Raise(_)
            | Self::Import { .. }
            | Self::FromImport { .. }
            | Self::Pass
            | Self::Break
            | Self::Continue
            | Self::Expr(_) => Vec::new(),
        }
    }

    /// The op with each node it names replaced by `map`'s, visited in the
    /// order the check's nodes are built: a statement's expressions before
    /// the bodies that follow them, each member in turn.
    pub(crate) fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        let values = |map: &mut dyn NodeMap, values: &[CheckedNodeId]| -> Vec<CheckedNodeId> {
            values.iter().map(|value| map.expression(*value)).collect()
        };
        let block = |map: &mut dyn NodeMap, body: &[CheckedStmtId]| -> Vec<CheckedStmtId> {
            body.iter()
                .map(|statement| map.statement(*statement))
                .collect()
        };
        let optional_block = |map: &mut dyn NodeMap, body: &Option<Vec<CheckedStmtId>>| {
            body.as_ref().map(|body| block(map, body))
        };
        let branches =
            |map: &mut dyn NodeMap, branches: &[(CheckedCondition, Vec<CheckedStmtId>)]| {
                branches
                    .iter()
                    .map(|(condition, body)| (condition.map_operands(map), block(map, body)))
                    .collect::<Vec<_>>()
            };
        let cases = |map: &mut dyn NodeMap, cases: &[CheckedMatchCase]| {
            cases
                .iter()
                .map(|case| CheckedMatchCase {
                    pattern: case.pattern.map_operands(map),
                    commands: case
                        .commands
                        .iter()
                        .map(|command| command.map_operands(map))
                        .collect(),
                    guard: case.guard.map(|guard| map.expression(guard)),
                    body: block(map, &case.body),
                })
                .collect::<Vec<_>>()
        };
        match self {
            Self::VarDecl {
                name,
                applications,
                value,
            } => Self::VarDecl {
                name: name.clone(),
                applications: values(map, applications),
                value: map.expression(*value),
            },
            Self::RefDecl { name, value } => Self::RefDecl {
                name: name.clone(),
                value: map.expression(*value),
            },
            Self::Assign { name, value } => Self::Assign {
                name: name.clone(),
                value: map.expression(*value),
            },
            Self::AugAssign { place, op, value } => Self::AugAssign {
                place: map.expression(*place),
                op: *op,
                value: map.expression(*value),
            },
            Self::Destructure { target, value } => Self::Destructure {
                target: target.map_operands(map),
                value: map.expression(*value),
            },
            Self::SetPlace { place, value } => Self::SetPlace {
                place: map.expression(*place),
                value: map.expression(*value),
            },
            Self::Def(def) => Self::Def(Box::new(CheckedDef {
                params: map_params(map, &def.params),
                where_clauses: values(map, &def.where_clauses),
                body: block(map, &def.body),
                ..(**def).clone()
            })),
            Self::Struct(declaration) => {
                let conformance_conditions = declaration
                    .conformance_conditions
                    .iter()
                    .map(|(name, value)| (name.clone(), map.expression(*value)))
                    .collect();
                let where_clauses = values(map, &declaration.where_clauses);
                let associated = declaration
                    .associated
                    .iter()
                    .map(|member| CheckedStructComptime {
                        name: member.name.clone(),
                        where_clauses: values(map, &member.where_clauses),
                        value: map.expression(member.value),
                    })
                    .collect();
                let methods = declaration
                    .methods
                    .iter()
                    .map(|method| method.map_operands(map))
                    .collect();
                Self::Struct(Box::new(CheckedStruct {
                    name: declaration.name.clone(),
                    conforms: declaration.conforms.clone(),
                    conformance_conditions,
                    where_clauses,
                    fields: declaration.fields.clone(),
                    associated,
                    methods,
                    fieldwise_init: declaration.fieldwise_init,
                }))
            }
            Self::Trait(declaration) => {
                let methods = declaration
                    .methods
                    .iter()
                    .map(|method| CheckedTraitMethod {
                        params: map_params(map, &method.params),
                        where_clauses: values(map, &method.where_clauses),
                        default: method.default.map(|default| map.statement(default)),
                        ..method.clone()
                    })
                    .collect();
                let comptime_members = declaration
                    .comptime_members
                    .iter()
                    .map(|member| CheckedTraitComptime {
                        name: member.name.clone(),
                        where_clauses: values(map, &member.where_clauses),
                    })
                    .collect();
                Self::Trait(Box::new(CheckedTrait {
                    name: declaration.name.clone(),
                    refines: declaration.refines.clone(),
                    methods,
                    comptime_members,
                }))
            }
            Self::Comptime {
                name,
                where_clauses,
                value,
                denotes,
            } => Self::Comptime {
                name: name.clone(),
                where_clauses: values(map, where_clauses),
                value: map.expression(*value),
                denotes: denotes.clone(),
            },
            Self::ComptimeIf {
                branches: arms,
                orelse,
            } => Self::ComptimeIf {
                branches: branches(map, arms),
                orelse: optional_block(map, orelse),
            },
            Self::If {
                branches: arms,
                orelse,
            } => Self::If {
                branches: branches(map, arms),
                orelse: optional_block(map, orelse),
            },
            Self::ComptimeFor { var, iter, body } => Self::ComptimeFor {
                var: var.clone(),
                iter: map.expression(*iter),
                body: block(map, body),
            },
            Self::ComptimeAssert { cond, message } => Self::ComptimeAssert {
                cond: map.expression(*cond),
                message: message.clone(),
            },
            Self::While { cond, body, orelse } => Self::While {
                cond: cond.map_operands(map),
                body: block(map, body),
                orelse: optional_block(map, orelse),
            },
            Self::Match {
                anchor,
                cases: arms,
            } => Self::Match {
                anchor: anchor.map(|anchor| map.statement(anchor)),
                cases: cases(map, arms),
            },
            Self::ComptimeMatch {
                subject,
                cases: arms,
            } => Self::ComptimeMatch {
                subject: map.expression(*subject),
                cases: cases(map, arms),
            },
            Self::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => Self::For {
                var: var.clone(),
                binding: *binding,
                iter: map.expression(*iter),
                body: block(map, body),
                orelse: optional_block(map, orelse),
            },
            Self::Return(value) => Self::Return(value.map(|value| map.expression(value))),
            Self::Raise(value) => Self::Raise(map.expression(*value)),
            Self::Expr(value) => Self::Expr(map.expression(*value)),
            Self::With { items, body } => Self::With {
                items: items
                    .iter()
                    .map(|item| CheckedWithItem {
                        context: map.expression(item.context),
                        var: item.var.clone(),
                    })
                    .collect(),
                body: block(map, body),
            },
            Self::Try {
                body,
                except,
                orelse,
                finalbody,
            } => Self::Try {
                body: block(map, body),
                except: except
                    .as_ref()
                    .map(|(name, body)| (name.clone(), block(map, body))),
                orelse: optional_block(map, orelse),
                finalbody: optional_block(map, finalbody),
            },
            Self::Scope(body) => Self::Scope(block(map, body)),
            Self::Import { .. }
            | Self::FromImport { .. }
            | Self::Pass
            | Self::Break
            | Self::Continue => self.clone(),
        }
    }

    /// The statement kind's name, for diagnostics.
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::VarDecl { .. } => "VarDecl",
            Self::RefDecl { .. } => "RefDecl",
            Self::Assign { .. } => "Assign",
            Self::AugAssign { .. } => "AugAssign",
            Self::Destructure { .. } => "Destructure",
            Self::SetPlace { .. } => "SetPlace",
            Self::Def(_) => "Def",
            Self::Struct(_) => "Struct",
            Self::Trait(_) => "Trait",
            Self::Comptime { .. } => "Comptime",
            Self::ComptimeIf { .. } => "ComptimeIf",
            Self::ComptimeFor { .. } => "ComptimeFor",
            Self::ComptimeAssert { .. } => "ComptimeAssert",
            Self::ComptimeMatch { .. } => "ComptimeMatch",
            Self::If { .. } => "If",
            Self::While { .. } => "While",
            Self::Match { .. } => "Match",
            Self::For { .. } => "For",
            Self::Return(_) => "Return",
            Self::Raise(_) => "Raise",
            Self::Import { .. } => "Import",
            Self::FromImport { .. } => "FromImport",
            Self::With { .. } => "With",
            Self::Try { .. } => "Try",
            Self::Scope(_) => "Scope",
            Self::Pass => "Pass",
            Self::Break => "Break",
            Self::Continue => "Continue",
            Self::Expr(_) => "Expr",
        }
    }
}

/// A checked `def`: its symbol and checked signature, its parameter
/// defaults and constraints as nodes, its body, and its captures.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedDef {
    pub name: String,
    /// The symbol it lowers to: signature-qualified when a module `def`'s
    /// name is overloaded, and a nested `def`'s own name, which its
    /// enclosing function's lowering qualifies.
    pub symbol: String,
    pub signature: CheckedSignature,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub where_clauses: Vec<CheckedNodeId>,
    pub body: Vec<CheckedStmtId>,
    /// The explicit captures the check resolved in its enclosing scope, as
    /// upstream's closure op holds its captured values; default captures
    /// are found from its body's checked bindings.
    pub captures: Vec<CheckedCapture>,
    /// The level its inline decorator names, as upstream's function op
    /// records it.
    pub inline: crate::declaration::InlineLevel,
}

/// A declaration's checked signature, as the check recorded it at the
/// declaration's sites: as upstream's function op holds its resolved
/// function type and its parameter list.
///
/// A type is absent where the check recorded none, which lowering reports
/// as a broken invariant.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CheckedSignature {
    /// Its own compile-time parameters.
    pub parameters: Vec<mojito_types::types::ParamDecl>,
    /// Each declared parameter's checked type, in declaration order (the
    /// receiver excluded), aligned with the declaration's parameters.
    pub params: Vec<Option<Ty>>,
    pub result: Option<Ty>,
    pub effect: Option<DeclarationEffect>,
}

impl CheckedSignature {
    /// The checked type of the declared parameter at `index`.
    pub fn param(&self, index: usize) -> Option<&Ty> {
        self.params.get(index)?.as_ref()
    }
}

/// A function parameter whose default is a node; its type is its
/// declaration's [`CheckedSignature`]'s.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedFnParam {
    pub name: String,
    pub default: Option<CheckedNodeId>,
    pub kind: ParamKind,
    pub convention: Option<ArgConvention>,
}

impl mojito_ast::ast::DeclaredParam for CheckedFnParam {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ParamKind {
        self.kind
    }

    fn convention(&self) -> Option<ArgConvention> {
        self.convention
    }

    fn has_default(&self) -> bool {
        self.default.is_some()
    }
}

/// A checked `struct`: its members, with their bodies and constraints as
/// nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedStruct {
    pub name: String,
    pub conforms: Vec<String>,
    pub conformance_conditions: Vec<(String, CheckedNodeId)>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub fields: Vec<CheckedField>,
    pub associated: Vec<CheckedStructComptime>,
    pub methods: Vec<CheckedMethod>,
    pub fieldwise_init: bool,
}

/// A struct's stored field: its name and checked type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedField {
    pub name: String,
    /// Absent where the check recorded none.
    pub ty: Option<Ty>,
}

/// A struct's associated `comptime` member.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedStructComptime {
    pub name: String,
    pub where_clauses: Vec<CheckedNodeId>,
    pub value: CheckedNodeId,
}

/// A struct method: its registered name, symbol, and checked signature,
/// with its body as statement nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedMethod {
    pub name: String,
    /// The name it is registered and counted under: a copy or move
    /// constructor's lifecycle name, as
    /// [`mojito_symbol::symbol::lifecycle_method_name`] names its syntax.
    pub registered: String,
    /// The symbol it lowers to: signature-qualified when overloaded.
    pub symbol: String,
    pub signature: CheckedSignature,
    pub has_self: bool,
    pub self_convention: Option<ArgConvention>,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub where_clauses: Vec<CheckedNodeId>,
    pub body: Vec<CheckedStmtId>,
    pub provenance: MethodProvenance,
    /// The level its inline decorator names.
    pub inline: crate::declaration::InlineLevel,
}

impl CheckedMethod {
    /// The method with each node it names replaced by `map`'s, in the
    /// order its nodes are built: its parameters' defaults, its
    /// constraints, its body.
    fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        let params = map_params(map, &self.params);
        let where_clauses = self
            .where_clauses
            .iter()
            .map(|clause| map.expression(*clause))
            .collect();
        Self {
            params,
            where_clauses,
            body: self
                .body
                .iter()
                .map(|statement| map.statement(*statement))
                .collect(),
            ..self.clone()
        }
    }
}

/// A checked `trait`: its requirements, a default's body as statement
/// nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTrait {
    pub name: String,
    pub refines: Vec<String>,
    pub methods: Vec<CheckedTraitMethod>,
    pub comptime_members: Vec<CheckedTraitComptime>,
}

/// A trait method requirement, and its default's member node when it has
/// one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTraitMethod {
    pub name: String,
    pub self_convention: Option<ArgConvention>,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub where_clauses: Vec<CheckedNodeId>,
    /// The `def` node of its trait's member
    /// ([`CheckedProgram::trait_members`]) when the method declares a
    /// default, as upstream's trait names its member function.
    pub default: Option<CheckedStmtId>,
}

/// A trait's `comptime` member requirement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTraitComptime {
    pub name: String,
    pub where_clauses: Vec<CheckedNodeId>,
}

/// One `with` item: its context as a node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedWithItem {
    pub context: CheckedNodeId,
    pub var: Option<String>,
}

/// An `if`/`elif`/`while` condition: a boolean node, or a `case` test of a
/// value's node by a pattern.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedCondition {
    Expr(CheckedNodeId),
    /// `case pattern = value`: the value's node, the anchor declaring the
    /// hidden subject the check synthesized over it, and the pattern's
    /// commands, as a `match` case's; a failed test takes the branch's
    /// else path.
    Case {
        pattern: CheckedPattern,
        value: CheckedNodeId,
        anchor: Option<CheckedStmtId>,
        commands: Vec<CheckedCommand>,
    },
}

impl CheckedCondition {
    /// The boolean node of an expression condition.
    pub const fn boolean(&self) -> Option<CheckedNodeId> {
        match self {
            Self::Expr(value) => Some(*value),
            Self::Case { .. } => None,
        }
    }

    /// The condition's nodes in checked order: the value, then a
    /// pattern's embedded expressions, then its commands'.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        match self {
            Self::Expr(value) => vec![*value],
            Self::Case {
                pattern,
                value,
                commands,
                ..
            } => std::iter::once(*value)
                .chain(pattern.expressions())
                .chain(commands.iter().flat_map(CheckedCommand::expressions))
                .collect(),
        }
    }

    /// The statements a pattern condition runs: its anchor and its
    /// commands' element reads and bindings, each a statement of its own.
    pub fn statements(&self) -> Vec<&[CheckedStmtId]> {
        match self {
            Self::Expr(_) => Vec::new(),
            Self::Case {
                anchor, commands, ..
            } => anchor
                .iter()
                .map(std::slice::from_ref)
                .chain(commands.iter().flat_map(CheckedCommand::statements))
                .collect(),
        }
    }

    pub(crate) fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        match self {
            Self::Expr(value) => Self::Expr(map.expression(*value)),
            Self::Case {
                pattern,
                value,
                anchor,
                commands,
            } => {
                let value = map.expression(*value);
                Self::Case {
                    pattern: pattern.map_operands(map),
                    value,
                    anchor: anchor.map(|anchor| map.statement(anchor)),
                    commands: commands
                        .iter()
                        .map(|command| command.map_operands(map))
                        .collect(),
                }
            }
        }
    }
}

/// One `case` arm of a `match`: its pattern, the commands the check
/// compiled it to, its guard, and its body.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedMatchCase {
    pub pattern: CheckedPattern,
    /// The case's program, in order: each test, which advances to the next
    /// case when it fails, and each binding, as upstream's command list.
    pub commands: Vec<CheckedCommand>,
    pub guard: Option<CheckedNodeId>,
    pub body: Vec<CheckedStmtId>,
}

impl CheckedMatchCase {
    /// The arm's nodes in checked order: the pattern's, the tests', then
    /// the guard.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        self.pattern
            .expressions()
            .into_iter()
            .chain(self.commands.iter().flat_map(CheckedCommand::expressions))
            .chain(self.guard)
            .collect()
    }

    /// The statements the case's commands run (element reads and
    /// bindings), in command order, each a statement of its own.
    pub fn statements(&self) -> Vec<&[CheckedStmtId]> {
        self.commands
            .iter()
            .flat_map(CheckedCommand::statements)
            .collect()
    }
}

/// One step of a case's program, as upstream's `PatternCommand`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedCommand {
    /// Compare the subject through the `__eq__` the check selected: the
    /// node of the comparison it synthesized (`$match<id> == value`), whose
    /// `Bool` selects; `False` advances to the next case.
    Equal { test: CheckedNodeId },
    /// Test a sequence pattern's subject length: the node of the call the
    /// check synthesized to `std.builtin._stubs.list_length_matches`
    /// (`list_length_matches($match<id>, arity, True)`), whose `Bool`
    /// selects; `False` advances to the next case before any element is
    /// read.
    ListLength { test: CheckedNodeId },
    /// Read one element of a sequence pattern's subject, once, at the first
    /// command that needs it: the declaration the check synthesized
    /// (`ref $elem<id> = parent[i]` over a reference result, `var $elem<id>
    /// = parent[i]` over an owned one), which the element's later commands
    /// name, as upstream's per-path value.
    PathValue { declaration: CheckedStmtId },
    /// Bind a name to the subject: the declaration the check synthesized
    /// for it (`ref name = $match<id>`, `var name = $match<id>`), or the
    /// assignment a later `|` alternative makes to the first's binding.
    Bind { declaration: CheckedStmtId },
    /// The alternatives of a `|` pattern, each a program of its own: the
    /// first whose tests all pass selects, and the case advances when none
    /// does.
    Alternatives(Vec<Vec<Self>>),
}

impl CheckedCommand {
    /// The command's expression nodes in checked order, alternatives'
    /// included.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        match self {
            Self::Equal { test } | Self::ListLength { test } => vec![*test],
            Self::Bind { .. } | Self::PathValue { .. } => Vec::new(),
            Self::Alternatives(alternatives) => alternatives
                .iter()
                .flatten()
                .flat_map(Self::expressions)
                .collect(),
        }
    }

    /// The statements the command runs (element reads and bindings),
    /// alternatives' included.
    pub fn statements(&self) -> Vec<&[CheckedStmtId]> {
        match self {
            Self::Bind { declaration } | Self::PathValue { declaration } => {
                vec![std::slice::from_ref(declaration)]
            }
            Self::Equal { .. } | Self::ListLength { .. } => Vec::new(),
            Self::Alternatives(alternatives) => alternatives
                .iter()
                .flatten()
                .flat_map(Self::statements)
                .collect(),
        }
    }

    fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        match self {
            Self::Equal { test } => Self::Equal {
                test: map.expression(*test),
            },
            Self::ListLength { test } => Self::ListLength {
                test: map.expression(*test),
            },
            Self::Bind { declaration } => Self::Bind {
                declaration: map.statement(*declaration),
            },
            Self::PathValue { declaration } => Self::PathValue {
                declaration: map.statement(*declaration),
            },
            Self::Alternatives(alternatives) => Self::Alternatives(
                alternatives
                    .iter()
                    .map(|commands| {
                        commands
                            .iter()
                            .map(|command| command.map_operands(map))
                            .collect()
                    })
                    .collect(),
            ),
        }
    }
}

/// The check's plan for one `match`, recorded at the statement.
///
/// It holds the declaration anchoring the subject, each case's commands,
/// which the checked program's nodes are built from, and whether the
/// cases cover every value of the subject, so that nothing falls through.
#[derive(Debug, Clone)]
pub struct MatchPlan {
    /// `ref $match<id> = subject` or `var $match<id> = subject`.
    pub anchor: Stmt,
    pub cases: Vec<Vec<PlannedCommand>>,
    pub concluded: bool,
}

/// A nonfatal diagnostic the check recorded at a source location: the
/// program still compiles and runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub message: String,
    pub span: SourceSpan,
}

/// One step of a planned case: the synthesized syntax the check recorded
/// facts for.
#[derive(Debug, Clone)]
pub enum PlannedCommand {
    /// The comparison `$match<id> == value`.
    Equal(Expr),
    /// The length test `list_length_matches($match<id>, arity, True)`.
    ListLength(Expr),
    /// The element read `ref $elem<id> = parent[i]` or `var $elem<id> =
    /// parent[i]`.
    PathValue(Box<Stmt>),
    /// The declaration `ref name = $match<id>` or `var name = $match<id>`,
    /// or a later `|` alternative's assignment to the first's binding.
    Bind(Box<Stmt>),
    /// The alternatives of a `|` pattern, each a program of its own.
    Alternatives(Vec<Vec<Self>>),
}

/// A checked `case` pattern: the source pattern
/// ([`MatchPattern`](mojito_ast::ast::MatchPattern)) with each embedded
/// expression replaced by its node.
///
/// The names it binds are definitions the pattern facts of the statement
/// describe, not nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedPattern {
    pub kind: CheckedPatternKind,
    pub span: Span,
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: SyntaxId,
}

impl CheckedPattern {
    /// The pattern's nodes in checked order.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        let mut out = Vec::new();
        self.collect_expressions(&mut out);
        out
    }

    fn collect_expressions(&self, out: &mut Vec<CheckedNodeId>) {
        match &self.kind {
            CheckedPatternKind::Literal(value)
            | CheckedPatternKind::Equal(value)
            | CheckedPatternKind::Head(value) => out.push(*value),
            CheckedPatternKind::As { pattern, .. } => pattern.collect_expressions(out),
            CheckedPatternKind::Tuple(items) | CheckedPatternKind::Or(items) => {
                for item in items {
                    item.collect_expressions(out);
                }
            }
            CheckedPatternKind::Sequence(items) => {
                for item in items {
                    if let CheckedSequencePattern::Element(pattern) = item {
                        pattern.collect_expressions(out);
                    }
                }
            }
            CheckedPatternKind::Call { head, args, kwargs } => {
                out.push(*head);
                for pattern in args {
                    pattern.collect_expressions(out);
                }
                for (_, pattern) in kwargs {
                    pattern.collect_expressions(out);
                }
            }
            CheckedPatternKind::Fields(fields) => {
                for (_, pattern) in fields {
                    pattern.collect_expressions(out);
                }
            }
            CheckedPatternKind::Wildcard | CheckedPatternKind::Bind { .. } => {}
        }
    }

    pub(crate) fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        let kind = match &self.kind {
            CheckedPatternKind::Wildcard => CheckedPatternKind::Wildcard,
            CheckedPatternKind::Bind { name, mode } => CheckedPatternKind::Bind {
                name: name.clone(),
                mode: *mode,
            },
            CheckedPatternKind::Literal(value) => {
                CheckedPatternKind::Literal(map.expression(*value))
            }
            CheckedPatternKind::Equal(value) => CheckedPatternKind::Equal(map.expression(*value)),
            CheckedPatternKind::Head(value) => CheckedPatternKind::Head(map.expression(*value)),
            CheckedPatternKind::Tuple(items) => {
                CheckedPatternKind::Tuple(items.iter().map(|item| item.map_operands(map)).collect())
            }
            CheckedPatternKind::Or(items) => {
                CheckedPatternKind::Or(items.iter().map(|item| item.map_operands(map)).collect())
            }
            CheckedPatternKind::Sequence(items) => CheckedPatternKind::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        CheckedSequencePattern::Element(pattern) => {
                            CheckedSequencePattern::Element(pattern.map_operands(map))
                        }
                        CheckedSequencePattern::Star { name } => {
                            CheckedSequencePattern::Star { name: name.clone() }
                        }
                    })
                    .collect(),
            ),
            CheckedPatternKind::Call { head, args, kwargs } => CheckedPatternKind::Call {
                head: map.expression(*head),
                args: args.iter().map(|item| item.map_operands(map)).collect(),
                kwargs: kwargs
                    .iter()
                    .map(|(name, pattern)| (name.clone(), pattern.map_operands(map)))
                    .collect(),
            },
            CheckedPatternKind::Fields(fields) => CheckedPatternKind::Fields(
                fields
                    .iter()
                    .map(|(name, pattern)| (name.clone(), pattern.map_operands(map)))
                    .collect(),
            ),
            CheckedPatternKind::As { pattern, name } => CheckedPatternKind::As {
                pattern: Box::new(pattern.map_operands(map)),
                name: name.clone(),
            },
        };
        Self {
            kind,
            span: self.span,
            syntax_id: self.syntax_id,
        }
    }
}

/// The forms of a [`CheckedPattern`], one per
/// [`MatchPatternKind`](mojito_ast::ast::MatchPatternKind).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedPatternKind {
    Wildcard,
    Bind {
        name: String,
        mode: BindingMode,
    },
    Literal(CheckedNodeId),
    Equal(CheckedNodeId),
    Tuple(Vec<CheckedPattern>),
    Sequence(Vec<CheckedSequencePattern>),
    Call {
        head: CheckedNodeId,
        args: Vec<CheckedPattern>,
        kwargs: Vec<(String, CheckedPattern)>,
    },
    Head(CheckedNodeId),
    Fields(Vec<(String, CheckedPattern)>),
    Or(Vec<CheckedPattern>),
    As {
        pattern: Box<CheckedPattern>,
        name: String,
    },
}

/// One element of a checked sequence pattern.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedSequencePattern {
    Element(CheckedPattern),
    Star { name: Option<String> },
}

/// A checked destructuring target: the source target
/// ([`DestructureTarget`](mojito_ast::ast::DestructureTarget)) with each
/// place replaced by its node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTarget {
    pub kind: CheckedTargetKind,
    pub span: Span,
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: SyntaxId,
}

impl CheckedTarget {
    /// The target's place nodes in checked order.
    pub fn expressions(&self) -> Vec<CheckedNodeId> {
        let mut out = Vec::new();
        self.collect_expressions(&mut out);
        out
    }

    fn collect_expressions(&self, out: &mut Vec<CheckedNodeId>) {
        match &self.kind {
            CheckedTargetKind::Place(place) => out.push(*place),
            CheckedTargetKind::Tuple(items) => {
                for item in items {
                    item.collect_expressions(out);
                }
            }
            CheckedTargetKind::Sequence(items) => {
                for item in items {
                    match item {
                        CheckedSequenceTarget::Element(target)
                        | CheckedSequenceTarget::Star(target) => target.collect_expressions(out),
                    }
                }
            }
            CheckedTargetKind::Discard | CheckedTargetKind::Declare { .. } => {}
        }
    }

    /// The names the target declares, in source order.
    pub fn declared_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_declared(&mut out);
        out
    }

    fn collect_declared<'a>(&'a self, out: &mut Vec<&'a str>) {
        match &self.kind {
            CheckedTargetKind::Declare { name, .. } => out.push(name),
            CheckedTargetKind::Tuple(items) => {
                for item in items {
                    item.collect_declared(out);
                }
            }
            CheckedTargetKind::Sequence(items) => {
                for item in items {
                    match item {
                        CheckedSequenceTarget::Element(target)
                        | CheckedSequenceTarget::Star(target) => target.collect_declared(out),
                    }
                }
            }
            CheckedTargetKind::Discard | CheckedTargetKind::Place(_) => {}
        }
    }

    pub(crate) fn map_operands(&self, map: &mut dyn NodeMap) -> Self {
        let kind = match &self.kind {
            CheckedTargetKind::Discard => CheckedTargetKind::Discard,
            CheckedTargetKind::Declare {
                name,
                mode,
                binding,
                binding_ty,
            } => CheckedTargetKind::Declare {
                name: name.clone(),
                mode: *mode,
                binding: *binding,
                binding_ty: binding_ty.clone(),
            },
            CheckedTargetKind::Place(place) => CheckedTargetKind::Place(map.expression(*place)),
            CheckedTargetKind::Tuple(items) => {
                CheckedTargetKind::Tuple(items.iter().map(|item| item.map_operands(map)).collect())
            }
            CheckedTargetKind::Sequence(items) => CheckedTargetKind::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        CheckedSequenceTarget::Element(target) => {
                            CheckedSequenceTarget::Element(target.map_operands(map))
                        }
                        CheckedSequenceTarget::Star(target) => {
                            CheckedSequenceTarget::Star(target.map_operands(map))
                        }
                    })
                    .collect(),
            ),
        };
        Self {
            kind,
            span: self.span,
            syntax_id: self.syntax_id,
        }
    }
}

/// The forms of a [`CheckedTarget`], one per
/// [`DestructureTargetKind`](mojito_ast::ast::DestructureTargetKind).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedTargetKind {
    Discard,
    /// A declared leaf: the binding the check introduced for it and that
    /// binding's storage type (a `ref` leaf's is a reference type).
    Declare {
        name: String,
        mode: BindingMode,
        binding: Option<mojito_types::origin::OwnerId>,
        binding_ty: Option<Ty>,
    },
    Place(CheckedNodeId),
    Tuple(Vec<CheckedTarget>),
    Sequence(Vec<CheckedSequenceTarget>),
}

/// One element of a checked sequence target.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedSequenceTarget {
    Element(CheckedTarget),
    Star(CheckedTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Declaration identity is independent of declaration spelling.
///
/// Future classes, pattern binders, coroutine state declarations, and
/// generated declarations can add variants without changing consumers of the
/// common metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum CheckedDeclKind {
    Function,
    Struct,
    Trait,
    Binding,
    CompileTime,
}

/// A declaration the check made. It encodes as [`CheckedExpr`] does,
/// behind an encoding mask.
#[derive(Debug, Clone)]
pub struct CheckedDeclaration {
    /// Its index in its tables, which an encoding leaves to its position.
    pub id: CheckedDeclId,
    pub kind: CheckedDeclKind,
    pub name: String,
    pub location: SourceSpan,
    /// Stable value-binding identity for declarations which introduce a runtime
    /// name. This is independent of spelling and therefore survives shadowing.
    pub binding: Option<mojito_types::origin::OwnerId>,
    pub ty: Option<Ty>,
    pub children: Vec<CheckedDeclId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// A node a [`CheckedTables::walk`] reaches.
#[derive(Clone, Copy)]
pub enum Walked<'a> {
    Statement(&'a CheckedStmt),
    Expression(&'a CheckedExpr),
}

/// What a [`CheckedTables::walk`] visitor asks of the node it was given, as
/// upstream's `WalkResult` does of an op.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WalkResult {
    /// Walk the node's operands and bodies.
    Advance,
    /// Walk the node's operands, but not its bodies: a statement's nested
    /// statement lists, a lambda's hidden `def`.
    SkipBodies,
}

/// The whole program's checked expression and statement nodes and its
/// declarations.
///
/// Every function's HIR and MIR lowering shares them through one `Arc`:
/// lowering a function never copies program-wide tables. A package unit
/// encodes its own, less each node's identity, which [`Self::reindexed`]
/// gives back from its position.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CheckedTables {
    /// Indexed by `CheckedNodeId`.
    pub expressions: Vec<CheckedExpr>,
    /// Indexed by `CheckedStmtId`.
    pub statements: Vec<CheckedStmt>,
    /// Indexed by `CheckedDeclId`.
    pub declarations: Vec<CheckedDeclaration>,
    /// The module's root statement nodes, in order.
    roots: Vec<CheckedStmtId>,
    /// Each default wrapper's root statements, parallel to
    /// [`CheckedProgram::default_wrappers`].
    wrapper_roots: Vec<Vec<CheckedStmtId>>,
}

impl CheckedTables {
    pub fn expression(&self, id: CheckedNodeId) -> Option<&CheckedExpr> {
        self.expressions.get(id.0 as usize)
    }

    pub fn declaration(&self, id: CheckedDeclId) -> Option<&CheckedDeclaration> {
        self.declarations.get(id.0 as usize)
    }

    pub fn statement(&self, id: CheckedStmtId) -> Option<&CheckedStmt> {
        self.statements.get(id.0 as usize)
    }

    /// The module's root statement nodes, in order.
    pub fn roots(&self) -> &[CheckedStmtId] {
        &self.roots
    }

    /// The root statements of default wrapper `index`, parallel to
    /// [`CheckedProgram::default_wrappers`].
    pub fn wrapper_roots(&self, index: usize) -> &[CheckedStmtId] {
        self.wrapper_roots.get(index).map_or(&[][..], Vec::as_slice)
    }

    /// Visit the statement nodes `statements` and every node under them in
    /// source preorder: a statement, then its annotations' lifted
    /// applications and its expressions (an `if` condition before its own
    /// branch), then its bodies; an expression, then its operands, a type
    /// argument's lifted applications where the argument is written; a
    /// lambda's hidden `def` as a statement.
    pub fn walk(
        &self,
        statements: &[CheckedStmtId],
        visit: &mut dyn FnMut(Walked<'_>) -> WalkResult,
    ) {
        for id in statements {
            self.walk_statement(*id, visit);
        }
    }

    /// [`Self::walk`] from expression node `id`.
    pub fn walk_expression(
        &self,
        id: CheckedNodeId,
        visit: &mut dyn FnMut(Walked<'_>) -> WalkResult,
    ) {
        let Some(node) = self.expression(id) else {
            return;
        };
        let result = visit(Walked::Expression(node));
        if let CheckedOp::Lambda { def } = &node.op {
            if result == WalkResult::Advance {
                self.walk_statement(*def, visit);
            }
            return;
        }
        for operand in node.op.reached() {
            self.walk_expression(operand, visit);
        }
    }

    /// The live prefix of statement list `body`: its statements up to and
    /// including the first that ends the block — a `return`, `break`,
    /// `continue`, or `raise`, or a scope whose own live statements end
    /// that way. The rest is dead, and lowering erases it as
    /// `LowerSemanticCF`'s `eraseOpToEndOfBlock` erases the ops after a
    /// terminator. An `if`, loop, or `try` never ends the block, even when
    /// every arm leaves it.
    pub fn live_statements<'a>(&self, body: &'a [CheckedStmtId]) -> &'a [CheckedStmtId] {
        body.iter()
            .position(|id| self.ends_block(*id))
            .map_or(body, |last| &body[..=last])
    }

    /// The tables of one lowering unit: the statement `root` and every node
    /// under it, held as a struct member's unit holds its struct when
    /// `shell` says so, then the statements of a wrapper's `body`, and the
    /// declarations the copied statements make.
    ///
    /// The nodes are copied in the order the check builds them, children
    /// before their parent, each renumbered by its position in the copy:
    /// a unit is its program's subtree, as upstream's is an op of its
    /// module.
    pub(crate) fn extract(
        &self,
        root: CheckedStmtId,
        shell: Option<StructShell>,
        body: &[CheckedStmtId],
    ) -> Self {
        let mut copy = Extraction {
            source: self,
            expressions: Vec::new(),
            statements: Vec::new(),
        };
        let root = match (shell, self.statement(root)) {
            (Some(shell), Some(statement)) => copy.shell(statement, shell),
            _ => copy.statement(root),
        };
        let mut declared: Vec<CheckedDeclId> = copy
            .statements
            .iter()
            .filter_map(|statement| statement.declaration)
            .collect();
        let wrapper_roots = if body.is_empty() {
            Vec::new()
        } else {
            vec![
                body.iter()
                    .map(|statement| copy.statement(*statement))
                    .collect(),
            ]
        };
        declared.sort_unstable();
        declared.dedup();
        let renumbered: HashMap<CheckedDeclId, CheckedDeclId> = declared
            .iter()
            .enumerate()
            .map(|(position, id)| (*id, CheckedDeclId(position as u32)))
            .collect();
        let declarations = declared
            .iter()
            .filter_map(|id| self.declaration(*id))
            .map(|declaration| CheckedDeclaration {
                id: renumbered[&declaration.id],
                children: declaration
                    .children
                    .iter()
                    .filter_map(|child| renumbered.get(child).copied())
                    .collect(),
                ..declaration.clone()
            })
            .collect();
        let mut tables = Self::new(CheckedArena {
            nodes: copy.expressions,
            statements: copy.statements,
            roots: vec![root],
            wrapper_roots,
        });
        for statement in &mut tables.statements {
            statement.declaration = statement
                .declaration
                .and_then(|id| renumbered.get(&id).copied());
        }
        tables.declarations = declarations;
        tables
    }

    /// The `def` node of trait member `member`, which its trait's op names
    /// as its method's default.
    pub(crate) fn trait_member_root(&self, member: &CheckedTraitMember) -> Option<CheckedStmtId> {
        self.roots.iter().find_map(|root| {
            let node = self.statement(*root)?;
            let CheckedStmtOp::Trait(declaration) = &node.op else {
                return None;
            };
            if (&declaration.name, &node.module) != (&member.trait_name, &member.module) {
                return None;
            }
            declaration.methods.get(member.method_index)?.default
        })
    }

    /// The tables of `arena`, which declare nothing yet.
    fn new(arena: CheckedArena) -> Self {
        let CheckedArena {
            nodes: expressions,
            statements,
            roots,
            wrapper_roots,
        } = arena;
        Self {
            expressions,
            statements,
            declarations: Vec::new(),
            roots,
            wrapper_roots,
        }
    }

    /// Make the declarations of the statements under the roots, then of
    /// the trait members whose `def` nodes are `members`, each declaring
    /// statement naming its own: in source preorder, the lambdas of a
    /// statement's own expressions before it, and a declaration before the
    /// ones its bodies make. A function's type is `function_ty`'s.
    fn declare(
        &mut self,
        members: &[CheckedStmtId],
        function_ty: &dyn Fn(&CheckedStmt) -> Option<Ty>,
    ) {
        let mut declarer = Declarer {
            tables: self,
            function_ty,
            declarations: Vec::new(),
            declared: Vec::new(),
        };
        declarer.block(&self.roots);
        declarer.block(members);
        let Declarer {
            declarations,
            declared,
            ..
        } = declarer;
        for (statement, declaration) in declared {
            self.statements[statement.0 as usize].declaration = Some(declaration);
        }
        self.declarations = declarations;
    }

    /// The tables with each node's identity its position.
    fn reindexed(mut self) -> Self {
        for (index, node) in self.expressions.iter_mut().enumerate() {
            node.id = CheckedNodeId(index as u32);
        }
        for (index, statement) in self.statements.iter_mut().enumerate() {
            statement.id = CheckedStmtId(index as u32);
        }
        for (index, declaration) in self.declarations.iter_mut().enumerate() {
            declaration.id = CheckedDeclId(index as u32);
        }
        self
    }

    fn walk_statement(&self, id: CheckedStmtId, visit: &mut dyn FnMut(Walked<'_>) -> WalkResult) {
        let Some(statement) = self.statement(id) else {
            return;
        };
        let bodies = visit(Walked::Statement(statement)) == WalkResult::Advance;
        let walk = |body: &[CheckedStmtId], visit: &mut dyn FnMut(Walked<'_>) -> WalkResult| {
            if bodies {
                self.walk(body, visit);
            }
        };
        match &statement.op {
            CheckedStmtOp::VarDecl {
                applications,
                value,
                ..
            } => {
                for node in applications.iter().chain([value]) {
                    self.walk_expression(*node, visit);
                }
            }
            CheckedStmtOp::If { branches, orelse }
            | CheckedStmtOp::ComptimeIf { branches, orelse } => {
                for (condition, body) in branches {
                    for node in condition.expressions() {
                        self.walk_expression(node, visit);
                    }
                    for statements in condition.statements() {
                        walk(statements, visit);
                    }
                    walk(body, visit);
                }
                walk(orelse.as_deref().unwrap_or_default(), visit);
            }
            CheckedStmtOp::Match { anchor, cases } => {
                if let Some(anchor) = anchor {
                    walk(std::slice::from_ref(anchor), visit);
                }
                for case in cases {
                    for node in case.pattern.expressions() {
                        self.walk_expression(node, visit);
                    }
                    for command in &case.commands {
                        for node in command.expressions() {
                            self.walk_expression(node, visit);
                        }
                        for declaration in command.statements() {
                            walk(declaration, visit);
                        }
                    }
                    if let Some(guard) = case.guard {
                        self.walk_expression(guard, visit);
                    }
                    walk(&case.body, visit);
                }
            }
            CheckedStmtOp::ComptimeMatch { subject, cases } => {
                self.walk_expression(*subject, visit);
                for case in cases {
                    for node in case.expressions() {
                        self.walk_expression(node, visit);
                    }
                    walk(&case.body, visit);
                }
            }
            op => {
                for node in op.expressions() {
                    self.walk_expression(node, visit);
                }
                for body in op.bodies() {
                    walk(body, visit);
                }
            }
        }
    }

    /// Whether statement `id` ends its block, per [`Self::live_statements`].
    fn ends_block(&self, id: CheckedStmtId) -> bool {
        self.statement(id)
            .is_some_and(|statement| match &statement.op {
                CheckedStmtOp::Return(_)
                | CheckedStmtOp::Break
                | CheckedStmtOp::Continue
                | CheckedStmtOp::Raise(_) => true,
                CheckedStmtOp::Scope(body) => self
                    .live_statements(body)
                    .last()
                    .is_some_and(|last| self.ends_block(*last)),
                _ => false,
            })
    }
}

/// The declarations [`CheckedTables::declare`] makes, being made.
struct Declarer<'t> {
    tables: &'t CheckedTables,
    function_ty: &'t dyn Fn(&CheckedStmt) -> Option<Ty>,
    declarations: Vec<CheckedDeclaration>,
    /// Each declaring statement and its declaration.
    declared: Vec<(CheckedStmtId, CheckedDeclId)>,
}

impl Declarer<'_> {
    /// The declarations `statements` make, the ones their bodies make
    /// held as their children.
    fn block(&mut self, statements: &[CheckedStmtId]) -> Vec<CheckedDeclId> {
        let mut ids = Vec::new();
        for id in statements {
            let Some(node) = self.tables.statement(*id) else {
                continue;
            };
            for lambda in self.lambdas(&node.op) {
                if let Some(def) = self.tables.statement(lambda)
                    && let CheckedStmtOp::Def(declaration) = &def.op
                {
                    ids.push(self.declaration(
                        def,
                        CheckedDeclKind::Function,
                        &declaration.name,
                        &[&declaration.body],
                    ));
                }
            }
            let (kind, name, bodies): (_, _, Vec<&[CheckedStmtId]>) = match &node.op {
                CheckedStmtOp::Def(declaration) => (
                    CheckedDeclKind::Function,
                    &declaration.name,
                    vec![&declaration.body],
                ),
                CheckedStmtOp::Struct(declaration) => (
                    CheckedDeclKind::Struct,
                    &declaration.name,
                    declaration
                        .methods
                        .iter()
                        .map(|method| method.body.as_slice())
                        .collect(),
                ),
                CheckedStmtOp::Trait(declaration) => {
                    (CheckedDeclKind::Trait, &declaration.name, Vec::new())
                }
                CheckedStmtOp::VarDecl { name, .. } | CheckedStmtOp::RefDecl { name, .. } => {
                    (CheckedDeclKind::Binding, name, Vec::new())
                }
                CheckedStmtOp::For { var, .. } | CheckedStmtOp::ComptimeFor { var, .. } => {
                    (CheckedDeclKind::Binding, var, node.op.bodies())
                }
                CheckedStmtOp::Try {
                    except: Some((Some(name), _)),
                    ..
                } => (CheckedDeclKind::Binding, name, node.op.bodies()),
                CheckedStmtOp::Comptime { name, .. } => {
                    (CheckedDeclKind::CompileTime, name, Vec::new())
                }
                // A control-flow statement declares nothing; its bodies'
                // declarations are its block's.
                op => {
                    for body in op.bodies() {
                        ids.extend(self.block(body));
                    }
                    continue;
                }
            };
            ids.push(self.declaration(node, kind, name, &bodies));
        }
        ids
    }

    /// The declaration of kind `kind` and name `name` the statement `node`
    /// makes, whose children are the declarations of `bodies`.
    fn declaration(
        &mut self,
        node: &CheckedStmt,
        kind: CheckedDeclKind,
        name: &str,
        bodies: &[&[CheckedStmtId]],
    ) -> CheckedDeclId {
        let id = CheckedDeclId(self.declarations.len() as u32);
        let ty = match kind {
            CheckedDeclKind::Function => (self.function_ty)(node),
            CheckedDeclKind::Binding | CheckedDeclKind::CompileTime => node.binding_ty.clone(),
            CheckedDeclKind::Struct | CheckedDeclKind::Trait => None,
        };
        self.declarations.push(CheckedDeclaration {
            id,
            kind,
            name: name.to_string(),
            location: node.location(),
            binding: node.binding,
            ty,
            children: Vec::new(),
        });
        self.declared.push((node.id, id));
        let children = bodies.iter().flat_map(|body| self.block(body)).collect();
        self.declarations[id.0 as usize].children = children;
        id
    }

    /// The `def` nodes of the lambdas in `op`'s own expressions, a
    /// declaration's signature's and a constant's constraints excepted, in
    /// source order; a lambda in a lambda is its `def`'s body's.
    fn lambdas(&self, op: &CheckedStmtOp) -> Vec<CheckedStmtId> {
        let expressions = match op {
            CheckedStmtOp::Comptime { value, .. } => vec![*value],
            CheckedStmtOp::Def(_) | CheckedStmtOp::Struct(_) | CheckedStmtOp::Trait(_) => {
                Vec::new()
            }
            op => op.expressions(),
        };
        let mut lambdas = Vec::new();
        for expression in expressions {
            self.tables.walk_expression(expression, &mut |node| {
                if let Walked::Expression(CheckedExpr {
                    op: CheckedOp::Lambda { def },
                    ..
                }) = node
                {
                    lambdas.push(*def);
                    return WalkResult::SkipBodies;
                }
                WalkResult::Advance
            });
        }
        lambdas
    }
}

/// One erased `rebind` the check judged, at its operand (or, for a rebound
/// assignment, its statement): the operand's own type, the target, and
/// whether the rebind is by value.
///
/// Source validation takes `dest` on faith while `operand` is symbolic, so
/// the elaborator owes their equality per instance after substitution.
/// `by_value` is the overload upstream selects once, on the declaration: a
/// trivially register-passable operand is rebound by value and is not a
/// place.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RebindAssertion {
    pub operand: Ty,
    pub dest: Ty,
    pub by_value: bool,
}

/// The wrapper a struct gets for a trait default it inherits and does not
/// spell.
///
/// As upstream's `StructEmitter::synthesizeDefaultTraitMethodWrapper` builds
/// it, it is a member over the default's resolved signature, whose parameter
/// and return types are the checked sites of its `method_index`, and whose
/// body is one call of the default's function with `Self` bound first and
/// every argument forwarded (`populateDefaultedTraitFunction`). It has no
/// syntax of its own: the struct's method list holds only what the struct
/// spells, and every wrapper's index follows them. Its parameters' defaults
/// are its member's, which the member's lowered declaration holds, as
/// `getTraitFunctionSignature` gives the wrapper its trait function's.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedDefaultWrapper {
    #[serde(with = "mojito_ast::ast::statement_module")]
    pub module: Option<String>,
    /// The struct that inherits the default.
    pub declaration: String,
    pub method: String,
    /// Its position among the struct's members, after every spelled method:
    /// the index of its `AnnotationSite::MethodParam`/`MethodReturn` and
    /// `GenericSite::Method` sites.
    pub method_index: usize,
    /// The symbol it lowers to: signature-qualified when the name is
    /// overloaded.
    pub lowered_name: String,
    /// The trait member it calls ([`CheckedTraitMember`]'s symbol).
    pub member: String,
    pub self_convention: Option<mojito_ast::ast::ArgConvention>,
    /// Every declared parameter but the receiver, in order.
    pub params: Vec<SignatureParam>,
    /// Its checked signature, read from its member sites when the program
    /// is assembled.
    pub signature: CheckedSignature,
    /// The check's syntax of the body. A package unit does not hold it:
    /// its nodes are the unit's [`CheckedTables::wrapper_roots`].
    #[serde(skip)]
    pub body: Vec<Stmt>,
}

impl CheckedDefaultWrapper {
    /// A wrapper's inline level: `nodebug`, whatever its struct, as
    /// upstream's `synthesizeDefaultTraitMethodWrapper` sets it, so a
    /// statically resolved call holds the forwarding call instead of the
    /// wrapper.
    pub const INLINE: crate::declaration::InlineLevel = crate::declaration::InlineLevel::NoDebug;
}

/// A trait default as the check checked it: its trait's member.
///
/// As upstream emits a default inside its trait, it is one function over the
/// trait's `Self`, its first compile-time parameter `_Self`, named by its
/// member symbol (`Trait.method`, signature-qualified when overloaded) and
/// taking the receiver as its first argument. Its declaration is the one the
/// check checked, built from its trait's method, whose checked sites are the
/// declaration's `AnnotationSite::FunctionParam`/`FunctionReturn` and
/// `GenericSite::Function`; a conformer's wrapper calls it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTraitMember {
    #[serde(with = "mojito_ast::ast::statement_module")]
    pub module: Option<String>,
    pub trait_name: String,
    /// Its position among its trait's methods.
    pub method_index: usize,
    /// A `def` named by the member symbol, over `[_Self, ...]` and
    /// `(self: _Self, ...)`, whose body is the default's. The whole program
    /// holds it as its node too, where its trait is, and the trait's
    /// [`CheckedTraitMethod::default`] names it. A package unit does not
    /// hold its syntax: its node is the unit's root.
    #[serde(skip, default = "absent_declaration")]
    pub declaration: Stmt,
}

/// A parameter of a resolved signature as a call binds and a body declares
/// it: everything but its type, which the signature holds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SignatureParam {
    pub name: String,
    pub kind: mojito_ast::ast::ParamKind,
    pub convention: Option<mojito_ast::ast::ArgConvention>,
}

impl mojito_ast::ast::DeclaredParam for SignatureParam {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> mojito_ast::ast::ParamKind {
        self.kind
    }

    fn convention(&self) -> Option<mojito_ast::ast::ArgConvention> {
        self.convention
    }

    /// None: a wrapper's defaults are its member's, which the member's
    /// declaration holds.
    fn has_default(&self) -> bool {
        false
    }
}

/// What one check of an elaborated program established, before the executable
/// handoff is assembled.
///
/// It is [`CheckedProgram::new`]'s input alone, until the check emits its
/// nodes itself: the check's facts, one record per occurrence
/// ([`NodeFacts`]), and the syntax they were recorded at, beside its
/// declaration-site tables. Every rejecting check has already run. It is
/// not an executable program and never reaches HIR or MIR.
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    pub statements: Vec<Stmt>,
    /// Every fact the check recorded at an occurrence, one record per
    /// occurrence.
    pub node_facts: HashMap<SourceSpan, NodeFacts>,
    pub struct_instantiations: Vec<StructInstantiation>,
    pub checked_types: HashMap<AnnotationSite, Ty>,
    pub generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    pub declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
    pub explicit_destroy_types: HashMap<String, ExplicitDestroyInfo>,
    /// Each struct's named destructors (`deinit self` methods), by struct,
    /// beside whether each raises; a struct declaring none has no entry.
    pub struct_destructors: HashMap<String, HashMap<String, bool>>,
    pub conformances: ConformanceFacts,
    /// The overload sets of every declaration the check knew: its own,
    /// and an imported package's, answered from the package's declarations.
    pub overloads: Arc<crate::package::ProgramOverloads>,
    /// The module integer constants the check folded: its own, and an
    /// imported package's, answered from the package's declarations.
    pub constants: Arc<crate::package::ProgramConstants>,
    /// Every inherited trait default's wrapper the check built.
    pub default_wrappers: Vec<CheckedDefaultWrapper>,
    /// Every trait default the check checked as its trait's member.
    pub trait_members: Vec<CheckedTraitMember>,
}

impl DiscoveryResult {
    /// Assemble the executable handoff. Nothing is inferred again: the arena
    /// and its indexes are built from the facts this result already holds.
    pub fn finalize(self) -> CheckedProgram {
        CheckedProgram::new(self)
    }
}

/// A successfully checked program plus semantic facts that downstream phases
/// previously recomputed from AST syntax or checker-private side tables.
#[derive(Debug, Clone)]
pub struct CheckedProgram {
    /// Every warning the check recorded, in source order.
    warnings: Vec<Warning>,
    compatibility_overload_targets: HashMap<SourceSpan, String>,
    generic_instantiations: HashMap<SourceSpan, GenericInstantiation>,
    method_instantiations: HashMap<SourceSpan, MethodInstantiation>,
    struct_instantiations: Vec<StructInstantiation>,
    compatibility_implicit_conversions: HashMap<SourceSpan, String>,
    /// The converted-to checked type of each implicit conversion site.
    implicit_conversion_types: HashMap<SourceSpan, Ty>,
    checked_types: HashMap<AnnotationSite, Ty>,
    generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    tables: Arc<CheckedTables>,
    explicit_destroy_types: HashMap<String, ExplicitDestroyInfo>,
    struct_destructors: HashMap<String, HashMap<String, bool>>,
    conformances: ConformanceFacts,
    declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
    overloads: Arc<crate::package::ProgramOverloads>,
    constants: Arc<crate::package::ProgramConstants>,
    default_wrappers: Vec<CheckedDefaultWrapper>,
    trait_members: Vec<CheckedTraitMember>,
}

/// What decides a conformance below the checker.
///
/// The program's own traits with their parents, and each of its own
/// structs' declared conformances and synthesized lifecycle rows. The
/// elaborator decides a struct's conformance from them when a clause asks
/// it; a package's structs and traits carry the same facts in its
/// declarations.
#[derive(Debug, Clone, Default)]
pub struct ConformanceFacts {
    /// Every trait the program declares, sorted by name. A compiler-known
    /// type conforms to none of them by its shape.
    pub traits: Vec<crate::declaration::TraitDeclaration>,
    /// Each struct's synthesized lifecycle rows; see
    /// [`crate::declaration::StructDeclaration::conformances`].
    pub structs: HashMap<String, Vec<StructConformance>>,
    /// Each struct's declared conformances; see
    /// [`crate::declaration::StructDeclaration::declared`].
    pub declared: HashMap<String, Vec<StructConformance>>,
    /// Each struct's unparameterized associated types (`Element` of `List`),
    /// sorted by member, each over the struct's own binders. The elaborator
    /// solves `C.Element` from them once `C` is bound to an instance.
    pub associated: HashMap<String, Vec<(String, mojito_types::types::Ty)>>,
    /// Each struct's parameterized associated types (`IteratorType[o]` of
    /// `List`), sorted by member; see
    /// [`crate::declaration::StructDeclaration::parameterized_associated_types`].
    pub parameterized_associated:
        HashMap<String, Vec<crate::declaration::ParameterizedAssociatedType>>,
    /// Each struct's compile-time value members (`K` of `comptime K = 7`),
    /// sorted by member, each a parameter expression over the struct's own
    /// binders (`Self.n + 1`). The elaborator answers `T.K` from them once
    /// `T` is bound to an instance.
    pub associated_values: HashMap<String, Vec<(String, mojito_types::param_expr::ParamExpr)>>,
    /// Each struct's parameterized compile-time value members (`get` of
    /// `comptime get[idx: Int]: Int`), sorted by member; see
    /// [`crate::declaration::StructDeclaration::parameterized_associated_values`].
    pub parameterized_values:
        HashMap<String, Vec<crate::declaration::ParameterizedAssociatedValue>>,
}

/// One struct's conformance to one trait: it holds for the instances whose
/// arguments satisfy any of `conditions`, each over the struct's own binders.
/// No condition means no instance conforms.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StructConformance {
    pub trait_name: String,
    pub conditions: Vec<mojito_types::types::GenericConstraint>,
}

/// A struct's explicit-destruction obligation, which its MIR declaration
/// carries for each struct that has one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitDestroyInfo {
    /// The `@explicit_destroy` message, or [`Self::default_message`].
    pub message: String,
    /// Whether `@explicit_destroy` declared `message`.
    pub declared: bool,
    #[serde(serialize_with = "sorted")]
    pub destructors: HashMap<String, bool>,
    /// Direct fields whose types carry their own explicit-destroy obligation.
    #[serde(serialize_with = "sorted")]
    pub fields: HashMap<String, String>,
}

impl ExplicitDestroyInfo {
    /// The message of a struct `name` that is not `Deinitable` without
    /// declaring `@explicit_destroy`.
    pub fn default_message(name: &str) -> String {
        let shown = mojito_types::types::unqualified_type_name(&Ty::new(
            mojito_types::types::TyKind::Struct(name.to_string(), Vec::new()),
        ));
        format!("type '{shown}' does not conform to 'Deinitable' and must be explicitly destroyed")
    }
}

/// The declaration-owned location of a source annotation. Unlike `SourceType`
/// syntax itself, this identity preserves the scope in which syntax such as a
/// bare `T` was resolved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AnnotationSite {
    FunctionParam {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
        param: usize,
    },
    /// Struct-owned sites are identified by the struct's unique name, not its
    /// span: compile-time struct specializations share their template's span
    /// (correct provenance), so spans cannot distinguish the declarations.
    StructField {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: String,
        field: usize,
    },
    MethodParam {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: String,
        method: usize,
        param: usize,
    },
    /// The checked return type of a free function declaration.
    FunctionReturn {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    /// The checked callable type of a free function declaration — the exact
    /// `Func`/`GenericFunc` type the checker binds the name to. Lowering uses
    /// it to type closure values without reconstructing signatures.
    FunctionType {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    /// The checked return type of a struct method declaration.
    MethodReturn {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: String,
        method: usize,
    },
}

/// Declaration-owned identity for a checked compile-time parameter list.
/// Runtime reification consumes these already-resolved declarations instead of
/// reclassifying source bounds in MIR or the VM.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GenericSite {
    Function {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: Span,
        syntax: mojito_common::token::SyntaxId,
    },
    Struct {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: String,
    },
    Method {
        #[serde(with = "mojito_ast::ast::statement_module")]
        module: Option<String>,
        declaration: String,
        method: usize,
    },
}

pub use mojito_types::types::{TransferEffect, TransferSet};

/// A higher-order transfer residue.
///
/// The callable's body calls through one of its own callable parameters, whose
/// transfer effects are unknowable in the body (a `def(...)` annotation cannot
/// spell effects).
///
/// Each call site — which knows the concrete callable — translates that
/// callable's effects through the recorded argument mapping into effects of
/// THIS callable and replays them against its own actuals.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CallThroughEffect {
    pub callee: CallThroughCallee,
    /// Signature abstraction of each inner-call argument slot.
    pub args: Vec<CallThroughArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CallThroughCallee {
    /// An ordinary runtime parameter with a function type (its slot index).
    RuntimeParam(usize),
    /// A compile-time callable value parameter (its declaration name).
    ValueParam(String),
}

/// How one inner-call actual maps to the enclosing signature.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallTransfer {
    pub dest: CheckedTransferDest,
    /// Interior path below the destination actual's root that receives the
    /// loans (empty = the whole root). Lowering turns a non-empty path into
    /// the `EstablishLoans` destination domain.
    pub dest_path: Vec<mojito_types::origin::OriginSeg>,
    pub sources: Vec<mojito_types::origin::Origin>,
    pub mutable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeclarationEffect {
    pub raises: bool,
    pub error: Option<Ty>,
    pub returns_reference: bool,
    /// Whether each explicit runtime parameter (receiver excluded, in
    /// declaration order) writes through its retained caller place: a
    /// `mut`/`var`/`deinit` parameter, or a `ref` whose declared origin is
    /// statically mutable. A `ref` under an immutable, parametric, or bare
    /// contract only reads (its body cannot prove mutability), so a place
    /// lent to it stays a shared access for loan checking.
    pub param_writes: Vec<bool>,
    /// A struct member's trailing `where` clauses, one entry per clause,
    /// over the owner's and the member's binders. A clause that does not
    /// hold under an instance's arguments leaves the member out of that
    /// instance.
    pub availability: Vec<mojito_types::types::GenericConstraint>,
}

impl DeclarationEffect {
    /// The `param_writes` mask of a declaration from its parameter
    /// conventions and checked `ref` contracts (aligned, receiver excluded).
    pub fn param_writes(
        conventions: &[Option<mojito_ast::ast::ArgConvention>],
        ref_params: &[Option<mojito_types::origin::RefSig>],
    ) -> Vec<bool> {
        use mojito_ast::ast::ArgConvention;
        use mojito_types::origin::SigMutability;
        conventions
            .iter()
            .enumerate()
            .map(|(index, convention)| match convention {
                Some(ArgConvention::Mut | ArgConvention::Var | ArgConvention::Deinit) => true,
                Some(ArgConvention::Ref) => ref_params
                    .get(index)
                    .and_then(Option::as_ref)
                    .is_some_and(|signature| signature.mutability == SigMutability::Mutable),
                _ => false,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Literal value retained after semantic checking for declaration metadata.
pub enum CheckedConst {
    Int(mojito_common::literal::IntLiteral),
    Float(mojito_common::literal::FloatLiteral),
    Bool(bool),
    String(String),
    None,
    /// A `DType.<name>` default.
    Dtype(mojito_ast::ast::Dtype),
    /// A default that is an `@implicit` conversion of `arg`: the omitted-arg
    /// slot materializes by running the converting constructor `target` on the
    /// folded literal (e.g. a `None` default for an `Optional[T]` parameter runs
    /// the empty-Optional constructor). Produced only when the checker recorded
    /// an implicit conversion at the default expression's span.
    Construct {
        #[serde(serialize_with = "mojito_types::symbol_ref::serialize")]
        target: String,
        arg: Box<Self>,
    },
    /// A default that is neither a folded literal nor a conversion of one
    /// (`String("a")`, `TWO + 1`): MIR lowers the expression as the
    /// zero-parameter function `function`, which each call that takes the
    /// slot runs, so the default is evaluated anew at every such call.
    ///
    /// `arguments` applies the function's own compile-time parameters, in
    /// the order of its declaration, over the binders of the declaration
    /// holding the default: a conformer's wrapper reaches its trait member's
    /// default at the wrapper's instantiation of the member. Empty when the
    /// default's owner declares the function, whose binders it binds by
    /// identity.
    Evaluate {
        #[serde(serialize_with = "mojito_types::symbol_ref::serialize")]
        function: String,
        arguments: Vec<mojito_types::types::TyArg>,
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
            ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType") => {
                mojito_ast::ast::Dtype::from_name(field).map(Self::Dtype)
            }
            ExprKind::Prefix(PrefixOp::Neg, inner) => match Self::from_expr(inner)? {
                Self::Int(value) => Some(Self::Int(value.neg())),
                Self::Float(value) => Some(Self::Float(value.neg())),
                _ => None,
            },
            _ => None,
        }
    }

    /// [`Self::from_expr`] over the node `id` of `tables`.
    pub fn from_node(tables: &CheckedTables, id: CheckedNodeId) -> Option<Self> {
        match &tables.expression(id)?.op {
            CheckedOp::Int(value) => Some(Self::Int(value.clone())),
            CheckedOp::Float(value) => Some(Self::Float(value.clone())),
            CheckedOp::Bool(value) => Some(Self::Bool(*value)),
            CheckedOp::Str(value) => Some(Self::String(value.clone())),
            CheckedOp::None => Some(Self::None),
            CheckedOp::Member { object, field }
                if tables.expression(*object).is_some_and(
                    |object| matches!(&object.op, CheckedOp::Identifier(name) if name == "DType"),
                ) =>
            {
                mojito_ast::ast::Dtype::from_name(field).map(Self::Dtype)
            }
            CheckedOp::Prefix(PrefixOp::Neg, inner) => match Self::from_node(tables, *inner)? {
                Self::Int(value) => Some(Self::Int(value.neg())),
                Self::Float(value) => Some(Self::Float(value.neg())),
                _ => None,
            },
            _ => None,
        }
    }

    /// The literal a conversion default converts, or the default itself.
    pub fn unconverted(&self) -> &Self {
        match self {
            Self::Construct { arg, .. } => arg.unconverted(),
            _ => self,
        }
    }
}

impl CheckedProgram {
    /// Every warning the check recorded, in source order: nonfatal
    /// diagnostics a program compiles and runs with.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The single checker-to-checked-boundary assembly point: the arena
    /// and the program's indexes, built from what `discovery` recorded.
    pub fn new(discovery: DiscoveryResult) -> Self {
        let DiscoveryResult {
            statements,
            node_facts,
            struct_instantiations,
            checked_types,
            generic_parameters,
            declaration_effects,
            explicit_destroy_types,
            struct_destructors,
            conformances,
            overloads,
            constants,
            mut default_wrappers,
            trait_members,
        } = discovery;
        let mut warnings: Vec<Warning> = node_facts
            .values()
            .flat_map(|facts| facts.match_warnings.iter().flatten().cloned())
            .collect();
        warnings.sort_by(|left, right| {
            (&left.span.source, left.span.span)
                .cmp(&(&right.span.source, right.span.span))
                .then_with(|| left.message.cmp(&right.message))
        });
        let expressions_span = mojito_common::timing::span("expressions");
        let signatures = Signatures {
            checked_types: &checked_types,
            generic_parameters: &generic_parameters,
            declaration_effects: &declaration_effects,
            overloads: &overloads,
            constants: &constants,
        };
        for wrapper in &mut default_wrappers {
            wrapper.signature = signatures.method(
                wrapper.module.as_ref(),
                &wrapper.declaration,
                wrapper.method_index,
                wrapper.params.len(),
            );
        }
        let arena = build_checked_expressions(
            &statements,
            &node_facts,
            &default_wrappers,
            &trait_members,
            signatures,
        );
        drop(expressions_span);
        let declarations_span = mojito_common::timing::span("declarations");
        let mut tables = CheckedTables::new(arena);
        let members: Vec<CheckedStmtId> = trait_members
            .iter()
            .filter_map(|member| tables.trait_member_root(member))
            .collect();
        tables.declare(&members, &|node| {
            checked_types
                .get(&AnnotationSite::FunctionType {
                    module: node.module.clone(),
                    declaration: node.span,
                    syntax: node.syntax_id,
                })
                .cloned()
        });
        drop(declarations_span);
        Self {
            warnings,
            compatibility_overload_targets: fact_index(&node_facts, |facts| {
                facts.overload_target.as_ref()
            }),
            generic_instantiations: fact_index(&node_facts, |facts| {
                facts.generic_instantiation.as_ref()
            }),
            method_instantiations: fact_index(&node_facts, |facts| {
                facts.method_instantiation.as_ref()
            }),
            struct_instantiations,
            compatibility_implicit_conversions: fact_index(&node_facts, |facts| {
                facts.implicit_conversion.as_ref()
            }),
            implicit_conversion_types: fact_index(&node_facts, |facts| {
                facts.implicit_conversion_type.as_ref()
            }),
            checked_types,
            generic_parameters,
            tables: Arc::new(tables),
            explicit_destroy_types,
            struct_destructors,
            conformances,
            declaration_effects,
            overloads,
            constants,
            default_wrappers,
            trait_members,
        }
    }

    /// The program of one package unit: its checked `tables` as the
    /// package holds them, its declaration-site facts `sites`, and
    /// `wrapper` or `trait_member` when it is one.
    ///
    /// It holds the unit's nodes and no syntax, as upstream's package
    /// holds each body as its IR: no statements, and neither the wrapper's
    /// body nor the member's declaration, whose nodes are the tables'. The
    /// location maps unit lowering reads are built from the nodes'
    /// attributes; the instantiation tables, which only whole-program
    /// passes read, are empty.
    pub fn installed(
        tables: CheckedTables,
        sites: InstalledSites,
        overloads: Arc<crate::package::ProgramOverloads>,
        constants: Arc<crate::package::ProgramConstants>,
        wrapper: Option<CheckedDefaultWrapper>,
        trait_member: Option<CheckedTraitMember>,
    ) -> Self {
        let tables = tables.reindexed();
        let mut overload_targets = HashMap::new();
        let mut implicit_conversions = HashMap::new();
        let mut implicit_conversion_types = HashMap::new();
        for node in &tables.expressions {
            let location = node.location();
            for adjustment in &node.adjustments {
                match adjustment {
                    SemanticAdjustment::ResolveCallable(target) => {
                        overload_targets.insert(location.clone(), target.clone());
                    }
                    SemanticAdjustment::ImplicitConversion(target)
                    | SemanticAdjustment::IndexNormalization { target } => {
                        implicit_conversions.insert(location.clone(), target.clone());
                    }
                    SemanticAdjustment::SplatScalar(ty) => {
                        implicit_conversions.insert(
                            location.clone(),
                            mojito_symbol::symbol::SCALAR_SPLAT_CONVERSION.to_string(),
                        );
                        implicit_conversion_types.insert(location.clone(), ty.clone());
                    }
                    SemanticAdjustment::ConversionResultType(ty) => {
                        implicit_conversion_types.insert(location.clone(), ty.clone());
                    }
                    _ => {}
                }
            }
        }
        let InstalledSites {
            checked_types,
            generic_parameters,
            declaration_effects,
        } = sites;
        Self {
            warnings: Vec::new(),
            compatibility_overload_targets: overload_targets,
            generic_instantiations: HashMap::new(),
            method_instantiations: HashMap::new(),
            struct_instantiations: Vec::new(),
            compatibility_implicit_conversions: implicit_conversions,
            implicit_conversion_types,
            checked_types,
            generic_parameters,
            tables: Arc::new(tables),
            explicit_destroy_types: HashMap::new(),
            struct_destructors: HashMap::new(),
            conformances: ConformanceFacts::default(),
            declaration_effects,
            overloads,
            constants,
            default_wrappers: wrapper.into_iter().collect(),
            trait_members: trait_member.into_iter().collect(),
        }
    }

    /// The wrapper of every trait default a struct of the program inherits
    /// and does not spell, as the check built it.
    pub fn default_wrappers(&self) -> &[CheckedDefaultWrapper] {
        &self.default_wrappers
    }

    /// Every trait default of the program, as the check checked it as its
    /// trait's member.
    pub fn trait_members(&self) -> &[CheckedTraitMember] {
        &self.trait_members
    }

    /// The overload sets of every declaration the check knew: the
    /// program's own and those of a package it imports.
    pub const fn overloads(&self) -> &Arc<crate::package::ProgramOverloads> {
        &self.overloads
    }

    /// The module integer constants the check folded, each its
    /// declaration's one value: the program's own and those of a package
    /// it imports. Every declaration's symbol spells a constant from here.
    pub const fn constants(&self) -> &Arc<crate::package::ProgramConstants> {
        &self.constants
    }

    /// Checker-selected lowered callable name at each resolved call site.
    pub const fn overload_targets(&self) -> &HashMap<SourceSpan, String> {
        &self.compatibility_overload_targets
    }

    /// The resolved generic application at each bound-generic call site: the
    /// source callee and the exact compile-time arguments `resolve_use_params`
    /// selected (types, values, and origins, symbolic entries included).
    /// Recorded so instantiation discovery can monomorphize call sites the
    /// elaborator cannot re-infer on its own.
    pub const fn generic_instantiations(&self) -> &HashMap<SourceSpan, GenericInstantiation> {
        &self.generic_instantiations
    }

    /// The resolved compile-time arguments per generic method call site,
    /// retained for per-call method specialization discovery.
    pub const fn method_instantiations(&self) -> &HashMap<SourceSpan, MethodInstantiation> {
        &self.method_instantiations
    }

    /// Every generic-struct application reached as a constructor target or
    /// method-call receiver, retained for per-instantiation method-clone
    /// discovery (the driver keeps the closed ones).
    pub fn struct_instantiations(&self) -> &[StructInstantiation] {
        &self.struct_instantiations
    }

    /// Checker-selected converting constructor for an expression used in a
    /// context that permits a user-defined implicit conversion.
    pub const fn implicit_conversions(&self) -> &HashMap<SourceSpan, String> {
        &self.compatibility_implicit_conversions
    }

    /// The checked type an implicit conversion at `span` converts to (the
    /// constructed type with its arguments, e.g. `Optional[Int]`).
    pub fn implicit_conversion_type(&self, span: &SourceSpan) -> Option<&Ty> {
        self.implicit_conversion_types.get(span)
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
        &self.tables.expressions
    }

    pub fn declarations(&self) -> &[CheckedDeclaration] {
        &self.tables.declarations
    }

    /// The shared expression/declaration tables that HIR and MIR lowering
    /// borrow per function.
    pub const fn tables(&self) -> &Arc<CheckedTables> {
        &self.tables
    }

    pub fn declaration(&self, id: CheckedDeclId) -> Option<&CheckedDeclaration> {
        self.tables.declaration(id)
    }

    pub fn expression(&self, id: CheckedNodeId) -> Option<&CheckedExpr> {
        self.tables.expression(id)
    }

    pub const fn explicit_destroy_types(&self) -> &HashMap<String, ExplicitDestroyInfo> {
        &self.explicit_destroy_types
    }

    /// Each struct's named destructors, beside whether each raises.
    pub const fn struct_destructors(&self) -> &HashMap<String, HashMap<String, bool>> {
        &self.struct_destructors
    }

    pub const fn conformances(&self) -> &ConformanceFacts {
        &self.conformances
    }

    /// Module identity attached to a declaration or expression location.
    pub fn declaration_module<'a>(&self, location: &'a SourceSpan) -> Option<&'a str> {
        location.source.as_deref()
    }

    pub(crate) const fn checked_types(&self) -> &HashMap<AnnotationSite, Ty> {
        &self.checked_types
    }

    pub(crate) const fn generic_parameters(
        &self,
    ) -> &HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>> {
        &self.generic_parameters
    }

    pub(crate) const fn declaration_effects(&self) -> &HashMap<AnnotationSite, DeclarationEffect> {
        &self.declaration_effects
    }
}

/// The declaration-site facts a package unit's program is installed with
/// ([`CheckedProgram::installed`]).
#[derive(Debug, Default)]
pub struct InstalledSites {
    pub checked_types: HashMap<AnnotationSite, Ty>,
    pub generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    pub declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
}

/// How a struct member's unit holds the struct it is a member of: as
/// its name, holding no method but the member's own. Its compile-time
/// parameters are its declaration's.
#[derive(Debug, Clone, Copy)]
pub(crate) enum StructShell {
    /// A spelled method, at its position among the struct's node's
    /// methods.
    Method(usize),
    /// An inherited default's wrapper, whose body the unit holds beside.
    Wrapper,
    /// A synthesized lifecycle member (the struct's destructor, copy
    /// constructor, or move constructor), which has no checked body: its
    /// lowering reads the struct's fields alone.
    Synthesized,
}

/// What a node operand is replaced by when an op is copied
/// ([`CheckedOp::map_operands`], [`CheckedStmtOp::map_operands`]).
pub(crate) trait NodeMap {
    fn expression(&mut self, id: CheckedNodeId) -> CheckedNodeId;
    fn statement(&mut self, id: CheckedStmtId) -> CheckedStmtId;
}

/// The lookups a declaration op's symbol and checked signature are read
/// from: the check's declaration sites, and the overload sets and module
/// constants a symbol is spelled by.
#[derive(Clone, Copy)]
pub(crate) struct Signatures<'a> {
    pub(crate) checked_types: &'a HashMap<AnnotationSite, Ty>,
    pub(crate) generic_parameters: &'a HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    pub(crate) declaration_effects: &'a HashMap<AnnotationSite, DeclarationEffect>,
    pub(crate) overloads: &'a crate::package::ProgramOverloads,
    pub(crate) constants: &'a crate::package::ProgramConstants,
}

impl Signatures<'_> {
    /// The checked signature of the free `def` `statement`, of `arity`
    /// declared parameters.
    fn function(&self, statement: &Stmt, arity: usize) -> CheckedSignature {
        let (module, declaration, syntax) =
            (&statement.module, statement.span, statement.syntax_id);
        let result = AnnotationSite::FunctionReturn {
            module: module.clone(),
            declaration,
            syntax,
        };
        CheckedSignature {
            parameters: self
                .generic_parameters
                .get(&GenericSite::Function {
                    module: module.clone(),
                    declaration,
                    syntax,
                })
                .cloned()
                .unwrap_or_default(),
            params: (0..arity)
                .map(|param| {
                    self.checked_types
                        .get(&AnnotationSite::FunctionParam {
                            module: module.clone(),
                            declaration,
                            syntax,
                            param,
                        })
                        .cloned()
                })
                .collect(),
            result: self.checked_types.get(&result).cloned(),
            effect: self.declaration_effects.get(&result).cloned(),
        }
    }

    /// The checked signature of the member `method` of struct
    /// `declaration`, of `arity` declared parameters.
    fn method(
        &self,
        module: Option<&String>,
        declaration: &str,
        method: usize,
        arity: usize,
    ) -> CheckedSignature {
        let result = AnnotationSite::MethodReturn {
            module: module.cloned(),
            declaration: declaration.to_string(),
            method,
        };
        CheckedSignature {
            parameters: self
                .generic_parameters
                .get(&GenericSite::Method {
                    module: module.cloned(),
                    declaration: declaration.to_string(),
                    method,
                })
                .cloned()
                .unwrap_or_default(),
            params: (0..arity)
                .map(|param| {
                    self.checked_types
                        .get(&AnnotationSite::MethodParam {
                            module: module.cloned(),
                            declaration: declaration.to_string(),
                            method,
                            param,
                        })
                        .cloned()
                })
                .collect(),
            result: self.checked_types.get(&result).cloned(),
            effect: self.declaration_effects.get(&result).cloned(),
        }
    }
}

/// The checked arena of `statements`, of `default_wrappers`' bodies, and of
/// each of `trait_members`' declarations, each node holding the facts
/// `facts` recorded at its occurrence, each declaration op's symbol and
/// signature read from `signatures`.
fn build_checked_expressions<'a>(
    statements: &[Stmt],
    facts: &'a HashMap<SourceSpan, NodeFacts>,
    default_wrappers: &[CheckedDefaultWrapper],
    trait_members: &'a [CheckedTraitMember],
    signatures: Signatures<'a>,
) -> CheckedArena {
    let mut builder = ArenaBuilder {
        nodes: Vec::new(),
        statements: Vec::new(),
        facts,
        trait_members,
        signatures,
        functions: 0,
    };
    let roots = builder.block(statements);
    let wrapper_roots = default_wrappers
        .iter()
        .map(|wrapper| builder.block(&wrapper.body))
        .collect();
    CheckedArena {
        nodes: builder.nodes,
        statements: builder.statements,
        roots,
        wrapper_roots,
    }
}

/// What [`build_checked_expressions`] builds.
struct CheckedArena {
    nodes: Vec<CheckedExpr>,
    statements: Vec<CheckedStmt>,
    roots: Vec<CheckedStmtId>,
    wrapper_roots: Vec<Vec<CheckedStmtId>>,
}

/// A unit's nodes being copied out of its program's tables
/// ([`CheckedTables::extract`]).
struct Extraction<'t> {
    source: &'t CheckedTables,
    expressions: Vec<CheckedExpr>,
    statements: Vec<CheckedStmt>,
}

impl Extraction<'_> {
    /// The struct `statement` as a member's unit holds it: its name, and
    /// the member's method when `shell` names one.
    fn shell(&mut self, statement: &CheckedStmt, shell: StructShell) -> CheckedStmtId {
        let CheckedStmtOp::Struct(declaration) = &statement.op else {
            return self.statement(statement.id);
        };
        let methods = match shell {
            StructShell::Method(member) => declaration
                .methods
                .get(member)
                .map(|method| method.map_operands(self))
                .into_iter()
                .collect(),
            StructShell::Wrapper | StructShell::Synthesized => Vec::new(),
        };
        let fields = match shell {
            StructShell::Synthesized => declaration.fields.clone(),
            StructShell::Method(_) | StructShell::Wrapper => Vec::new(),
        };
        self.push(CheckedStmt {
            op: CheckedStmtOp::Struct(Box::new(CheckedStruct {
                name: declaration.name.clone(),
                conforms: Vec::new(),
                conformance_conditions: Vec::new(),
                where_clauses: Vec::new(),
                fields,
                associated: Vec::new(),
                methods,
                fieldwise_init: false,
            })),
            ..statement.clone()
        })
    }

    fn push(&mut self, statement: CheckedStmt) -> CheckedStmtId {
        let id = CheckedStmtId(self.statements.len() as u32);
        self.statements.push(CheckedStmt { id, ..statement });
        id
    }
}

impl NodeMap for Extraction<'_> {
    fn expression(&mut self, id: CheckedNodeId) -> CheckedNodeId {
        let source = self.source;
        let Some(node) = source.expression(id) else {
            return id;
        };
        let op = node.op.map_operands(self);
        let id = CheckedNodeId(self.expressions.len() as u32);
        self.expressions.push(CheckedExpr {
            id,
            op,
            ..node.clone()
        });
        id
    }

    fn statement(&mut self, id: CheckedStmtId) -> CheckedStmtId {
        let source = self.source;
        let Some(statement) = source.statement(id) else {
            return id;
        };
        let op = statement.op.map_operands(self);
        self.push(CheckedStmt {
            op,
            ..statement.clone()
        })
    }
}

/// The checked arena under construction.
struct ArenaBuilder<'a> {
    nodes: Vec<CheckedExpr>,
    statements: Vec<CheckedStmt>,
    facts: &'a HashMap<SourceSpan, NodeFacts>,
    /// The members whose `def` nodes a trait's defaults are built as.
    trait_members: &'a [CheckedTraitMember],
    /// What a declaration op's symbol and signature are read from.
    signatures: Signatures<'a>,
    /// How many function bodies enclose the statement being built: a
    /// `def` inside one is named by its own name.
    functions: usize,
}

impl<'a> ArenaBuilder<'a> {
    fn facts(&self, span: &SourceSpan) -> &'a NodeFacts {
        self.facts.get(span).unwrap_or(&NodeFacts::EMPTY)
    }

    /// The `def` of the member method `index` of trait `trait_name`, which
    /// `statement` declares, defaults to.
    fn member_declaration(
        &self,
        statement: &Stmt,
        trait_name: &str,
        index: usize,
    ) -> Option<&'a Stmt> {
        self.trait_members
            .iter()
            .find(|member| {
                member.module == statement.module
                    && member.trait_name == trait_name
                    && member.method_index == index
            })
            .map(|member| &member.declaration)
    }

    /// A condition's nodes: the value, then a pattern's expressions.
    fn condition(&mut self, condition: &mojito_ast::ast::Condition) -> CheckedCondition {
        match condition {
            mojito_ast::ast::Condition::Expr(value) => CheckedCondition::Expr(self.expr(value)),
            mojito_ast::ast::Condition::Case { pattern, value } => {
                let value = self.expr(value);
                let plan = self.facts(&pattern.source_span()).match_plan.clone();
                CheckedCondition::Case {
                    pattern: self.pattern(pattern),
                    value,
                    anchor: plan.as_ref().map(|plan| self.stmt(&plan.anchor)),
                    commands: plan
                        .as_ref()
                        .and_then(|plan| plan.cases.first())
                        .map(|commands| {
                            commands
                                .iter()
                                .map(|command| self.command(command))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }
        }
    }

    /// A pattern's node, its embedded expressions built in source order.
    fn pattern(&mut self, pattern: &mojito_ast::ast::MatchPattern) -> CheckedPattern {
        use mojito_ast::ast::{MatchPatternKind, SequencePattern};
        let kind = match &pattern.kind {
            MatchPatternKind::Wildcard => CheckedPatternKind::Wildcard,
            MatchPatternKind::Bind { name, mode } => CheckedPatternKind::Bind {
                name: name.clone(),
                mode: *mode,
            },
            MatchPatternKind::Literal(value) => CheckedPatternKind::Literal(self.expr(value)),
            MatchPatternKind::Equal(value) => CheckedPatternKind::Equal(self.expr(value)),
            MatchPatternKind::Head(value) => CheckedPatternKind::Head(self.expr(value)),
            MatchPatternKind::Tuple(items) => {
                CheckedPatternKind::Tuple(items.iter().map(|item| self.pattern(item)).collect())
            }
            MatchPatternKind::Or(items) => {
                CheckedPatternKind::Or(items.iter().map(|item| self.pattern(item)).collect())
            }
            MatchPatternKind::Sequence(items) => CheckedPatternKind::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        SequencePattern::Element(pattern) => {
                            CheckedSequencePattern::Element(self.pattern(pattern))
                        }
                        SequencePattern::Star { name, .. } => {
                            CheckedSequencePattern::Star { name: name.clone() }
                        }
                    })
                    .collect(),
            ),
            MatchPatternKind::Call { head, args, kwargs } => CheckedPatternKind::Call {
                head: self.expr(head),
                args: args.iter().map(|item| self.pattern(item)).collect(),
                kwargs: kwargs
                    .iter()
                    .map(|(name, pattern)| (name.clone(), self.pattern(pattern)))
                    .collect(),
            },
            MatchPatternKind::Fields(fields) => CheckedPatternKind::Fields(
                fields
                    .iter()
                    .map(|(name, pattern)| (name.clone(), self.pattern(pattern)))
                    .collect(),
            ),
            MatchPatternKind::As { pattern, name } => CheckedPatternKind::As {
                pattern: Box::new(self.pattern(pattern)),
                name: name.clone(),
            },
        };
        CheckedPattern {
            kind,
            span: pattern.span,
            syntax_id: pattern.syntax_id,
        }
    }

    /// A target's node, its places built in source order.
    fn target(&mut self, target: &mojito_ast::ast::DestructureTarget) -> CheckedTarget {
        use mojito_ast::ast::{DestructureTargetKind, SequenceTarget};
        let kind = match &target.kind {
            DestructureTargetKind::Discard => CheckedTargetKind::Discard,
            DestructureTargetKind::Declare { name, mode } => {
                let facts = self.facts(&target.source_span());
                CheckedTargetKind::Declare {
                    name: name.clone(),
                    mode: *mode,
                    binding: facts.binding,
                    binding_ty: facts.binding_ty.clone(),
                }
            }
            DestructureTargetKind::Place(place) => CheckedTargetKind::Place(self.expr(place)),
            DestructureTargetKind::Tuple(items) => {
                CheckedTargetKind::Tuple(items.iter().map(|item| self.target(item)).collect())
            }
            DestructureTargetKind::Sequence(items) => CheckedTargetKind::Sequence(
                items
                    .iter()
                    .map(|item| match item {
                        SequenceTarget::Element(target) => {
                            CheckedSequenceTarget::Element(self.target(target))
                        }
                        SequenceTarget::Star(target) => {
                            CheckedSequenceTarget::Star(self.target(target))
                        }
                    })
                    .collect(),
            ),
        };
        CheckedTarget {
            kind,
            span: target.span,
            syntax_id: target.syntax_id,
        }
    }

    fn expr(&mut self, expression: &Expr) -> CheckedNodeId {
        use ExprKind::{Identifier, Index, Member, TypeApply, TypeValue};
        if let Some(node) = self.spelled_construction(expression) {
            return node;
        }
        let op = self.op(expression);
        let facts = self.facts(&expression.source_span());
        let adjustments = node_adjustments(facts, expression);
        let variant_projection = matches!(
            facts.adjustment.as_deref(),
            Some(SemanticAdjustment::VariantProject { .. })
        );
        let resolved_callable_value = facts.overload_target.is_some();
        let ty = facts.ty.clone();
        let place_ty = facts.place_ty.clone();
        let binding_ty = facts.binding_ty.clone();
        let binding = facts.binding;
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
            op,
            span: expression.span,
            source: expression.source.clone(),
            syntax_id: expression.syntax_id,
            ty,
            place_ty,
            binding_ty,
            category,
            binding,
            contextual_base: facts.contextual_base.clone(),
            effects: facts.effects.clone().unwrap_or_default(),
            adjustments,
            comprehension_bindings: facts.comprehension_bindings.clone().unwrap_or_default(),
            call_transfers: facts.call_transfers.clone().unwrap_or_default(),
        });
        id
    }

    /// The node of the construction the check spelled for `expression`, if
    /// it is a spelled sugar: the sugar's operand position holds it, so HIR
    /// lowers the construction where the sugar was written (an
    /// initializer list, a t-string, a value pack read as its
    /// `ParameterList`, a scalar `range` call, a `comptime for` iterable read
    /// as its `__iter__()` call).
    fn spelled_construction(&mut self, expression: &Expr) -> Option<CheckedNodeId> {
        if !matches!(
            expression.kind,
            ExprKind::BraceLit(_)
                | ExprKind::ListLit(_)
                | ExprKind::TString { .. }
                | ExprKind::Identifier(_)
                | ExprKind::Member { .. }
                | ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Invoke { .. }
                | ExprKind::Index { .. }
        ) {
            return None;
        }
        let SemanticAdjustment::SpelledConstruction { construction } = self
            .facts(&expression.source_span())
            .adjustment
            .as_deref()?
        else {
            return None;
        };
        let node = self.expr(&construction.0);
        // A `for` loop iterates the construction it spells, and a `comptime
        // for` steps it.
        let facts = self.facts(&expression.source_span());
        let loops: Vec<SemanticAdjustment> = facts
            .iteration_protocol
            .iter()
            .map(|protocol| SemanticAdjustment::Iterate((**protocol).clone()))
            .chain(
                facts
                    .comptime_iteration
                    .iter()
                    .map(|iteration| SemanticAdjustment::ComptimeIteration(iteration.clone())),
            )
            .collect();
        self.nodes[node.0 as usize].adjustments.extend(loops);
        Some(node)
    }

    /// `expression`'s own operation, its operands' nodes built in their
    /// structural order.
    fn op(&mut self, expression: &Expr) -> CheckedOp {
        let values = |this: &mut Self, values: &[Expr]| {
            values
                .iter()
                .map(|value| this.expr(value))
                .collect::<Vec<_>>()
        };
        match &expression.kind {
            ExprKind::Int(value) => CheckedOp::Int(value.clone()),
            ExprKind::Float(value) => CheckedOp::Float(value.clone()),
            ExprKind::Bool(value) => CheckedOp::Bool(*value),
            ExprKind::Str(value) => CheckedOp::Str(value.clone()),
            ExprKind::None => CheckedOp::None,
            ExprKind::Uninitialized => CheckedOp::Uninitialized,
            ExprKind::EmptySubscript => CheckedOp::EmptySubscript,
            ExprKind::Identifier(name) => CheckedOp::Identifier(name.clone()),
            ExprKind::TypeValue(ty) => CheckedOp::TypeValue {
                ty: self.facts(&expression.source_span()).type_operand.clone(),
                applications: self.type_applications(ty),
            },
            ExprKind::Prefix(op, value) => CheckedOp::Prefix(*op, self.expr(value)),
            ExprKind::Transfer(value) => CheckedOp::Transfer(self.expr(value)),
            ExprKind::Spread(value) => CheckedOp::Spread(self.expr(value)),
            ExprKind::Named { name, value } => CheckedOp::Named {
                name: name.clone(),
                value: self.expr(value),
            },
            ExprKind::Infix(op, left, right) => {
                let left = self.expr(left);
                CheckedOp::Infix(*op, left, self.expr(right))
            }
            ExprKind::Index { object, index } => CheckedOp::Index {
                object: self.expr(object),
                index: self.expr(index),
            },
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Call {
                name: name.clone(),
                param_args: self.param_args(expression, param_args),
                args: values(self, args),
                kwargs: self.kwargs(kwargs),
            },
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => CheckedOp::Invoke {
                callee: self.expr(callee),
                param_args: self.param_args(expression, param_args),
                args: values(self, args),
                kwargs: self.kwargs(kwargs),
            },
            ExprKind::Member { object, field } => CheckedOp::Member {
                object: self.expr(object),
                field: field.clone(),
            },
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => CheckedOp::MethodCall {
                object: self.expr(object),
                method: method.clone(),
                args: values(self, args),
                kwargs: self.kwargs(kwargs),
            },
            ExprKind::TypeApply { name, args } => CheckedOp::TypeApply {
                name: name.clone(),
                args: self.param_args(expression, args),
            },
            ExprKind::ListLit(items) => CheckedOp::ListLit(values(self, items)),
            ExprKind::TupleLit(items) => CheckedOp::TupleLit(values(self, items)),
            ExprKind::BraceLit(entries) => CheckedOp::BraceLit(
                entries
                    .iter()
                    .map(|(key, value)| {
                        (self.expr(key), value.as_ref().map(|value| self.expr(value)))
                    })
                    .collect(),
            ),
            ExprKind::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => {
                let clauses = clauses
                    .iter()
                    .map(|clause| match clause {
                        ComprehensionClause::For { var, binding, iter } => {
                            CheckedComprehensionClause::For {
                                var: var.clone(),
                                binding: *binding,
                                iter: self.expr(iter),
                            }
                        }
                        ComprehensionClause::If(condition) => {
                            CheckedComprehensionClause::If(self.expr(condition))
                        }
                    })
                    .collect();
                CheckedOp::Comprehension {
                    kind: *kind,
                    key: key.as_ref().map(|key| self.expr(key)),
                    value: self.expr(value),
                    clauses,
                }
            }
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => CheckedOp::IfExpr {
                cond: self.expr(cond),
                then_branch: self.expr(then_branch),
                else_branch: self.expr(else_branch),
            },
            ExprKind::Compare { first, rest } => CheckedOp::Compare {
                first: self.expr(first),
                rest: rest
                    .iter()
                    .map(|(op, value)| (*op, self.expr(value)))
                    .collect(),
            },
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
            } => CheckedOp::Slice {
                object: self.expr(object),
                lower: self.optional(lower.as_deref()),
                upper: self.optional(upper.as_deref()),
                step: self.optional(step.as_deref()),
            },
            ExprKind::MultiIndex { object, args } => CheckedOp::MultiIndex {
                object: self.expr(object),
                args: args
                    .iter()
                    .map(|argument| match argument {
                        SubscriptArg::Index(value) => CheckedSubscriptArg::Index(self.expr(value)),
                        SubscriptArg::Keyword { name, value } => CheckedSubscriptArg::Keyword {
                            name: name.clone(),
                            value: self.expr(value),
                        },
                        SubscriptArg::Slice { lower, upper, step } => CheckedSubscriptArg::Slice {
                            lower: self.optional(lower.as_deref()),
                            upper: self.optional(upper.as_deref()),
                            step: self.optional(step.as_deref()),
                        },
                        SubscriptArg::KeywordSlice {
                            name,
                            lower,
                            upper,
                            step,
                        } => CheckedSubscriptArg::KeywordSlice {
                            name: name.clone(),
                            lower: self.optional(lower.as_deref()),
                            upper: self.optional(upper.as_deref()),
                            step: self.optional(step.as_deref()),
                        },
                    })
                    .collect(),
            },
            ExprKind::TString { parts, raw } => CheckedOp::TString {
                parts: parts
                    .iter()
                    .map(|part| match part {
                        TStringPart::Literal(text) => CheckedTStringPart::Literal(text.clone()),
                        TStringPart::Expr(value) => CheckedTStringPart::Expr(self.expr(value)),
                    })
                    .collect(),
                raw: *raw,
            },
            // A lambda node has no operands; it names its hidden
            // definition, built exactly like a nested `def` statement.
            ExprKind::Lambda { def } => CheckedOp::Lambda {
                def: self.stmt(def),
            },
        }
    }

    fn optional(&mut self, expression: Option<&Expr>) -> Option<CheckedNodeId> {
        expression.map(|expression| self.expr(expression))
    }

    fn kwargs(&mut self, kwargs: &[KwArg]) -> Vec<CheckedKwArg> {
        kwargs
            .iter()
            .map(|kwarg| CheckedKwArg {
                name: kwarg.name.clone(),
                value: self.expr(&kwarg.value),
            })
            .collect()
    }

    /// The bracket arguments of `expression`. An argument the check
    /// resolved to an attribute — a type-shaped one, or a value-shaped one
    /// it bound to a type or an origin — is the attribute it recorded at
    /// its position ([`NodeFacts::parameter_attributes`]), and builds the
    /// nodes of its lifted applications only.
    fn param_args(&mut self, expression: &Expr, arguments: &[ParamArg]) -> Vec<CheckedParamArg> {
        let attributes = self
            .facts(&expression.source_span())
            .parameter_attributes
            .as_deref()
            .unwrap_or_default();
        arguments
            .iter()
            .enumerate()
            .map(|(position, argument)| {
                let attribute = attributes.get(position).cloned().flatten();
                self.param_arg(argument, attribute)
            })
            .collect()
    }

    fn param_arg(
        &mut self,
        argument: &ParamArg,
        attribute: Option<mojito_types::types::TyArg>,
    ) -> CheckedParamArg {
        match argument {
            ParamArg::Type(ty) => CheckedParamArg::Attribute {
                value: attribute,
                applications: self.type_applications(ty),
            },
            ParamArg::Value(value) if attribute.is_some() => CheckedParamArg::Attribute {
                value: attribute,
                applications: self.expression_applications(value),
            },
            ParamArg::Value(value) => CheckedParamArg::Value(self.expr(value)),
            ParamArg::Named { name, value } => CheckedParamArg::Named {
                name: name.clone(),
                value: Box::new(self.param_arg(value, attribute)),
            },
        }
    }

    /// Build the node of each expression inside `ty` the check named a
    /// lifted function for ([`SemanticAdjustment::ComptimeApplication`]):
    /// MIR lowers it in that function, away from the annotation that
    /// spells it. Any other expression of an annotation has no node.
    fn type_applications(&mut self, ty: &mojito_ast::ast::Type) -> Vec<CheckedNodeId> {
        self.applications(|applied| mojito_ast::visit::walk_type(applied, ty))
    }

    /// [`Self::type_applications`] for a bracket argument written as a
    /// value that the check bound to a type or an origin: the argument is
    /// an attribute, and only its lifted applications are nodes.
    fn expression_applications(&mut self, value: &Expr) -> Vec<CheckedNodeId> {
        self.applications(|applied| mojito_ast::visit::walk_expr(applied, value))
    }

    fn applications(
        &mut self,
        walk: impl FnOnce(&mut LiftedApplications<'_>),
    ) -> Vec<CheckedNodeId> {
        let mut applied = LiftedApplications {
            facts: self.facts,
            found: Vec::new(),
        };
        walk(&mut applied);
        applied
            .found
            .iter()
            .map(|expression| self.expr(expression))
            .collect()
    }

    fn block(&mut self, statements: &[Stmt]) -> Vec<CheckedStmtId> {
        statements
            .iter()
            .map(|statement| self.stmt(statement))
            .collect()
    }

    /// `statement`'s node, built after its operands' and bodies' nodes.
    /// One planned command as the checked program's: its synthesized node
    /// or statement converted, alternatives recursively.
    fn command(&mut self, planned: &PlannedCommand) -> CheckedCommand {
        match planned {
            PlannedCommand::Equal(test) => CheckedCommand::Equal {
                test: self.expr(test),
            },
            PlannedCommand::ListLength(test) => CheckedCommand::ListLength {
                test: self.expr(test),
            },
            PlannedCommand::Bind(declaration) => CheckedCommand::Bind {
                declaration: self.stmt(declaration),
            },
            PlannedCommand::PathValue(declaration) => CheckedCommand::PathValue {
                declaration: self.stmt(declaration),
            },
            PlannedCommand::Alternatives(alternatives) => CheckedCommand::Alternatives(
                alternatives
                    .iter()
                    .map(|commands| {
                        commands
                            .iter()
                            .map(|command| self.command(command))
                            .collect()
                    })
                    .collect(),
            ),
        }
    }

    fn stmt(&mut self, statement: &Stmt) -> CheckedStmtId {
        let op = self.stmt_op(statement);
        let facts = self.facts(&statement.source_span());
        let id = CheckedStmtId(self.statements.len() as u32);
        self.statements.push(CheckedStmt {
            id,
            op,
            span: statement.span,
            module: statement.module.clone(),
            syntax_id: statement.syntax_id,
            binding: facts.statement_binding,
            binding_ty: facts.binding_ty.clone(),
            declaration: None,
        });
        id
    }

    /// `statement`'s own operation, its expressions' and bodies' nodes built
    /// in their structural order.
    /// A declaration's op (`def`, `struct`, `trait`): its symbol, signature,
    /// and members. `None` for any other statement.
    fn declaration_op(&mut self, statement: &Stmt) -> Option<CheckedStmtOp> {
        use mojito_ast::ast::StmtKind;
        let values = |this: &mut Self, values: &[Expr]| {
            values
                .iter()
                .map(|value| this.expr(value))
                .collect::<Vec<_>>()
        };
        Some(match &statement.kind {
            StmtKind::Def {
                name,
                type_params,
                params,
                positional_only,
                keyword_only,
                raises,
                where_clauses,
                body,
                decorators,
                ..
            } => CheckedStmtOp::Def(Box::new(CheckedDef {
                name: name.clone(),
                symbol: self.function_symbol(name, type_params, params),
                signature: self.signatures.function(statement, params.len()),
                params: self.fn_params(params),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                where_clauses: values(self, where_clauses),
                body: self.function_body(body),
                captures: self
                    .facts(&statement.source_span())
                    .captures
                    .clone()
                    .unwrap_or_default(),
                inline: crate::declaration::InlineLevel::of(decorators).unwrap_or_default(),
            })),
            StmtKind::Struct {
                name,
                type_params,
                conforms,
                conformance_conditions,
                where_clauses,
                fields,
                associated,
                methods,
                fieldwise_init,
                ..
            } => {
                let conformance_conditions = conformance_conditions
                    .iter()
                    .map(|(name, value)| (name.clone(), self.expr(value)))
                    .collect();
                let where_clauses = values(self, where_clauses);
                let associated = associated
                    .iter()
                    .map(|member| CheckedStructComptime {
                        name: member.name.clone(),
                        where_clauses: values(self, &member.where_clauses),
                        value: self.expr(&member.value),
                    })
                    .collect();
                let methods = methods
                    .iter()
                    .enumerate()
                    .map(|(position, method)| {
                        let registered =
                            mojito_symbol::symbol::lifecycle_method_name(method).to_string();
                        let symbol = self.method_symbol(name, type_params, &registered, method);
                        CheckedMethod {
                            name: method.name.clone(),
                            registered,
                            symbol,
                            signature: self.signatures.method(
                                statement.module.as_ref(),
                                name,
                                position,
                                method.params.len(),
                            ),
                            has_self: method.has_self,
                            self_convention: method.self_convention,
                            params: self.fn_params(&method.params),
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            where_clauses: values(self, &method.where_clauses),
                            body: self.function_body(&method.body),
                            provenance: method.provenance,
                            inline: crate::declaration::InlineLevel::of(&method.decorators)
                                .unwrap_or_default(),
                        }
                    })
                    .collect();
                let fields = fields
                    .iter()
                    .enumerate()
                    .map(|(field, declared)| CheckedField {
                        name: declared.name.clone(),
                        ty: self
                            .signatures
                            .checked_types
                            .get(&AnnotationSite::StructField {
                                module: statement.module.clone(),
                                declaration: name.clone(),
                                field,
                            })
                            .cloned(),
                    })
                    .collect();
                CheckedStmtOp::Struct(Box::new(CheckedStruct {
                    name: name.clone(),
                    conforms: conforms.clone(),
                    conformance_conditions,
                    where_clauses,
                    fields,
                    associated,
                    methods,
                    fieldwise_init: *fieldwise_init,
                }))
            }
            StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } => {
                let methods = methods
                    .iter()
                    .enumerate()
                    .map(|(index, method)| CheckedTraitMethod {
                        name: method.name.clone(),
                        self_convention: method.self_convention,
                        params: self.fn_params(&method.params),
                        positional_only: method.positional_only,
                        keyword_only: method.keyword_only,
                        raises: method.raises,
                        where_clauses: values(self, &method.where_clauses),
                        default: method
                            .default_body
                            .as_ref()
                            .and_then(|_| self.member_declaration(statement, name, index))
                            .map(|member| self.stmt(member)),
                    })
                    .collect();
                let comptime_members = comptime_members
                    .iter()
                    .map(|member| CheckedTraitComptime {
                        name: member.name.clone(),
                        where_clauses: values(self, &member.where_clauses),
                    })
                    .collect();
                CheckedStmtOp::Trait(Box::new(CheckedTrait {
                    name: name.clone(),
                    refines: refines.clone(),
                    methods,
                    comptime_members,
                }))
            }
            _ => return None,
        })
    }

    fn stmt_op(&mut self, statement: &Stmt) -> CheckedStmtOp {
        use mojito_ast::ast::StmtKind;
        if let Some(op) = self.declaration_op(statement) {
            return op;
        }
        let values = |this: &mut Self, values: &[Expr]| {
            values
                .iter()
                .map(|value| this.expr(value))
                .collect::<Vec<_>>()
        };
        let optional_block =
            |this: &mut Self, body: &Option<Vec<Stmt>>| body.as_ref().map(|body| this.block(body));
        let branches = |this: &mut Self, branches: &[(mojito_ast::ast::Condition, Vec<Stmt>)]| {
            branches
                .iter()
                .map(|(condition, body)| (this.condition(condition), this.block(body)))
                .collect::<Vec<_>>()
        };
        let cases =
            |this: &mut Self, cases: &[mojito_ast::ast::MatchCase], plan: Option<&MatchPlan>| {
                cases
                    .iter()
                    .enumerate()
                    .map(|(index, case)| CheckedMatchCase {
                        pattern: this.pattern(&case.pattern),
                        commands: plan
                            .and_then(|plan| plan.cases.get(index))
                            .map(|commands| {
                                commands
                                    .iter()
                                    .map(|command| this.command(command))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        guard: case.guard.as_ref().map(|guard| this.expr(guard)),
                        body: this.block(&case.body),
                    })
                    .collect::<Vec<_>>()
            };
        match &statement.kind {
            StmtKind::VarDecl { name, ty, value } => CheckedStmtOp::VarDecl {
                name: name.clone(),
                applications: ty
                    .as_ref()
                    .map(|ty| self.type_applications(ty))
                    .unwrap_or_default(),
                value: self.expr(value),
            },
            StmtKind::RefDecl { name, value } => CheckedStmtOp::RefDecl {
                name: name.clone(),
                value: self.expr(value),
            },
            StmtKind::Assign { name, value } => CheckedStmtOp::Assign {
                name: name.clone(),
                value: self.expr(value),
            },
            StmtKind::Raise(value) => CheckedStmtOp::Raise(self.expr(value)),
            StmtKind::Expr(value) => CheckedStmtOp::Expr(self.expr(value)),
            StmtKind::Comptime {
                name,
                where_clauses,
                value,
                ..
            } => CheckedStmtOp::Comptime {
                name: name.clone(),
                where_clauses: values(self, where_clauses),
                value: self.expr(value),
                denotes: self
                    .facts(&statement.source_span())
                    .comptime_value
                    .as_deref()
                    .cloned(),
            },
            StmtKind::AugAssign { place, op, value } => CheckedStmtOp::AugAssign {
                place: self.expr(place),
                op: *op,
                value: self.expr(value),
            },
            StmtKind::SetPlace { place, value } => CheckedStmtOp::SetPlace {
                place: self.expr(place),
                value: self.expr(value),
            },
            StmtKind::Destructure { target, value } => CheckedStmtOp::Destructure {
                target: self.target(target),
                value: self.expr(value),
            },
            // Declarations were taken above.
            StmtKind::Def { .. } | StmtKind::Struct { .. } | StmtKind::Trait { .. } => {
                CheckedStmtOp::Pass
            }
            StmtKind::ComptimeIf {
                branches: arms,
                orelse,
            } => CheckedStmtOp::ComptimeIf {
                branches: branches(self, arms),
                orelse: optional_block(self, orelse),
            },
            StmtKind::If {
                branches: arms,
                orelse,
            } => CheckedStmtOp::If {
                branches: branches(self, arms),
                orelse: optional_block(self, orelse),
            },
            StmtKind::ComptimeFor { var, iter, body } => CheckedStmtOp::ComptimeFor {
                var: var.clone(),
                iter: self.expr(iter),
                body: self.block(body),
            },
            StmtKind::ComptimeAssert { cond, message } => CheckedStmtOp::ComptimeAssert {
                cond: self.expr(cond),
                message: message.as_ref().and_then(|message| match &message.kind {
                    ExprKind::Str(text) => Some(text.clone()),
                    _ => None,
                }),
            },
            StmtKind::Scope(body) => CheckedStmtOp::Scope(self.block(body)),
            StmtKind::While { cond, body, orelse } => CheckedStmtOp::While {
                cond: self.condition(cond),
                body: self.block(body),
                orelse: optional_block(self, orelse),
            },
            StmtKind::Match { cases: arms, .. } => {
                let plan = self.facts(&statement.source_span()).match_plan.clone();
                let anchor = plan.as_ref().map(|plan| self.stmt(&plan.anchor));
                CheckedStmtOp::Match {
                    anchor,
                    cases: cases(self, arms, plan.as_ref()),
                }
            }
            StmtKind::ComptimeMatch {
                subject,
                cases: arms,
            } => CheckedStmtOp::ComptimeMatch {
                subject: self.expr(subject),
                cases: cases(self, arms, None),
            },
            StmtKind::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => CheckedStmtOp::For {
                var: var.clone(),
                binding: *binding,
                iter: self.expr(iter),
                body: self.block(body),
                orelse: optional_block(self, orelse),
            },
            StmtKind::Return(value) => CheckedStmtOp::Return(self.optional(value.as_ref())),
            StmtKind::With { items, body } => CheckedStmtOp::With {
                items: items
                    .iter()
                    .map(|item| CheckedWithItem {
                        context: self.expr(&item.context),
                        var: item.var.clone(),
                    })
                    .collect(),
                body: self.block(body),
            },
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => CheckedStmtOp::Try {
                body: self.block(body),
                except: except
                    .as_ref()
                    .map(|(name, body)| (name.clone(), self.block(body))),
                orelse: optional_block(self, orelse),
                finalbody: optional_block(self, finalbody),
            },
            StmtKind::Import { path, alias } => CheckedStmtOp::Import {
                path: path.clone(),
                alias: alias.clone(),
            },
            StmtKind::FromImport { level, path, names } => CheckedStmtOp::FromImport {
                level: *level,
                path: path.clone(),
                names: names.clone(),
            },
            StmtKind::Pass => CheckedStmtOp::Pass,
            StmtKind::Break => CheckedStmtOp::Break,
            StmtKind::Continue => CheckedStmtOp::Continue,
        }
    }

    fn fn_params(&mut self, params: &[mojito_ast::ast::FnParam]) -> Vec<CheckedFnParam> {
        params
            .iter()
            .map(|param| CheckedFnParam {
                name: param.name.clone(),
                default: self.optional(param.default.as_ref()),
                kind: param.kind,
                convention: param.convention,
            })
            .collect()
    }

    /// The nodes of a function's `body`, inside one more function.
    fn function_body(&mut self, body: &[Stmt]) -> Vec<CheckedStmtId> {
        self.functions += 1;
        let body = self.block(body);
        self.functions -= 1;
        body
    }

    /// The symbol the `def` `name` lowers to, as the check names its
    /// owner: a module `def`'s signature-qualified when its name is
    /// overloaded, a nested one's its own name.
    fn function_symbol(
        &self,
        name: &str,
        type_params: &[TypeParam],
        params: &[mojito_ast::ast::FnParam],
    ) -> String {
        if self.functions > 0 {
            return name.to_string();
        }
        let signatures = self.signatures;
        mojito_symbol::symbol::lowered_def_name(
            name,
            type_params,
            params,
            signatures.overloads,
            &|name| signatures.constants.get(name),
        )
    }

    /// The symbol struct `structure`'s method `method`, registered as
    /// `registered`, lowers to.
    fn method_symbol(
        &self,
        structure: &str,
        type_params: &[TypeParam],
        registered: &str,
        method: &mojito_ast::ast::Method,
    ) -> String {
        let signatures = self.signatures;
        mojito_symbol::symbol::lowered_method_name(
            &format!("{structure}.{registered}"),
            type_params,
            mojito_symbol::symbol::MethodShape::of(method),
            signatures.overloads,
            &|name| signatures.constants.get(name),
        )
    }
}

/// The adjustments the check's `facts` at `expression` record, in the
/// order lowering applies them.
/// The outermost expressions of a walked annotation or bracket argument
/// the check named a lifted function for.
struct LiftedApplications<'a> {
    facts: &'a HashMap<SourceSpan, NodeFacts>,
    found: Vec<Expr>,
}

impl mojito_ast::visit::Visitor for LiftedApplications<'_> {
    fn visit_expr(&mut self, expression: &Expr) {
        let nested = self.found.last().is_some_and(|outer| {
            outer.span.0 <= expression.span.0 && expression.span.1 <= outer.span.1
        });
        if !nested
            && matches!(
                self.facts
                    .get(&expression.source_span())
                    .and_then(|facts| facts.adjustment.as_deref()),
                Some(SemanticAdjustment::ComptimeApplication { .. })
            )
        {
            self.found.push(expression.clone());
        }
    }
}

/// `params` with each default replaced by `map`'s node.
fn map_params(map: &mut dyn NodeMap, params: &[CheckedFnParam]) -> Vec<CheckedFnParam> {
    params
        .iter()
        .map(|param| CheckedFnParam {
            default: param.default.map(|default| map.expression(default)),
            ..param.clone()
        })
        .collect()
}

fn node_adjustments(facts: &NodeFacts, expression: &Expr) -> Vec<SemanticAdjustment> {
    let mut adjustments = Vec::new();
    if let Some(target) = &facts.overload_target {
        adjustments.push(SemanticAdjustment::ResolveCallable(target.clone()));
    }
    if let Some(call) = &facts.selected_call {
        adjustments.push(SemanticAdjustment::SelectedCall(call.clone()));
    }
    if let Some((descriptors, set_value_keyword)) = &facts.subscript_descriptor {
        adjustments.push(SemanticAdjustment::SliceDescriptors {
            descriptors: descriptors.clone(),
            set_value_keyword: *set_value_keyword,
        });
    }
    if let Some(target) = &facts.implicit_conversion {
        if mojito_symbol::symbol::is_index_normalization_symbol(target) {
            adjustments.push(SemanticAdjustment::IndexNormalization {
                target: target.clone(),
            });
        } else if target == mojito_symbol::symbol::SCALAR_SPLAT_CONVERSION {
            adjustments.extend(
                facts
                    .implicit_conversion_type
                    .clone()
                    .map(SemanticAdjustment::SplatScalar),
            );
        } else {
            adjustments.push(SemanticAdjustment::ImplicitConversion(target.clone()));
            if let Some(ty) = &facts.implicit_conversion_type {
                adjustments.push(SemanticAdjustment::ConversionResultType(ty.clone()));
            }
            if let Some(error) = &facts.conversion_raises {
                adjustments.push(SemanticAdjustment::ConversionRaises(error.clone()));
            }
            if let Some(arguments) = &facts.conversion_instantiation {
                adjustments.push(SemanticAdjustment::ConversionInstantiatedArguments(
                    arguments.clone(),
                ));
            }
            if let Some(mutable) = &facts.conversion_source_borrow {
                adjustments.push(SemanticAdjustment::BorrowConversionSource { mutable: *mutable });
            }
        }
    }
    // A `^` the checker bound to a read parameter or receiver lends
    // its place and does not move it.
    if matches!(expression.kind, ExprKind::Transfer(_)) && !facts.borrowed_read_call_place {
        adjustments.push(SemanticAdjustment::Move);
    }
    if facts.explicit_destroy_call {
        adjustments.push(SemanticAdjustment::ExplicitDestroy);
    }
    if let Some(protocol) = &facts.iteration_protocol {
        adjustments.push(SemanticAdjustment::Iterate((**protocol).clone()));
    }
    if let Some(writable) = &facts.reference_value_use {
        adjustments.push(if *writable {
            SemanticAdjustment::BorrowMutable
        } else {
            SemanticAdjustment::BorrowShared
        });
    }
    if facts.copy_place_value_use {
        adjustments.push(SemanticAdjustment::CopyPlaceValue);
    }
    if facts.call_place_use {
        adjustments.push(SemanticAdjustment::RetainCallPlace);
    }
    if facts.borrowed_read_call_place {
        adjustments.push(SemanticAdjustment::BorrowReadArgument);
    }
    if facts.read_temporary_argument {
        adjustments.push(SemanticAdjustment::ReadTemporaryArgument);
    }
    if facts.forwarded_argument {
        adjustments.push(SemanticAdjustment::ForwardedArgument);
    }
    if let Some(links) = &facts.compare_links {
        adjustments.push(SemanticAdjustment::CompareLinks(links.clone()));
    }
    if facts.implicitly_copied_consuming_receiver {
        adjustments.push(SemanticAdjustment::ImplicitlyCopyConsumingReceiver);
    }
    if facts.truthiness_condition {
        adjustments.push(SemanticAdjustment::Truthiness);
    }
    if facts.register_passable_result {
        adjustments.push(SemanticAdjustment::RegisterPassableResult);
    }
    if facts.comptime_conditional {
        adjustments.push(SemanticAdjustment::ComptimeConditional);
    }
    if let Some(assertion) = &facts.rebind_assertion
        && assertion.operand != assertion.dest
    {
        adjustments.push(SemanticAdjustment::Rebind {
            operand: assertion.operand.clone(),
            dest: assertion.dest.clone(),
        });
    }
    if let Some((dtype, width)) = &facts.simd_construction {
        adjustments.push(SemanticAdjustment::ConstructSimd {
            dtype: dtype.clone(),
            width: width.clone(),
        });
    }
    if let Some(operation) = facts.adjustment.as_deref() {
        adjustments.push(operation.clone());
        if let SemanticAdjustment::ReceiverFromFirstArgument { inner: Some(inner) }
        | SemanticAdjustment::ComptimeApplication {
            operation: Some(inner),
            ..
        } = operation
        {
            adjustments.push((**inner).clone());
        }
    }
    // Kept apart from the single per-span operation record: a
    // parameterized method invoke that yields a reference carries
    // both this and the `ReferenceResult` operation.
    if let Some(param_decls) = &facts.parameterized_method_call {
        adjustments.push(SemanticAdjustment::ParameterizedMethodCall {
            param_decls: param_decls.clone(),
        });
    }
    if let Some(instantiation) = &facts.generic_instantiation {
        adjustments.push(SemanticAdjustment::InstantiatedArguments(
            instantiation.arguments.clone(),
        ));
        if !instantiation.folded_arguments.is_empty() {
            adjustments.push(SemanticAdjustment::FoldedParameterArguments(
                instantiation.folded_arguments.clone(),
            ));
        }
    } else if let Some(instantiation) = &facts.method_instantiation {
        adjustments.push(SemanticAdjustment::InstantiatedArguments(
            instantiation.arguments.clone(),
        ));
        if !instantiation.folded_arguments.is_empty() {
            adjustments.push(SemanticAdjustment::FoldedParameterArguments(
                instantiation.folded_arguments.clone(),
            ));
        }
    }
    if let Some(elements) = &facts.tuple_unpack_plan {
        adjustments.push(SemanticAdjustment::TupleUnpack {
            elements: elements.clone(),
        });
    }
    if let Some(origin) = &facts.interior_reference {
        adjustments.push(SemanticAdjustment::InteriorReference {
            origin: origin.clone(),
        });
    }
    if let Some(invalidations) = &facts.interior_invalidations {
        adjustments.push(SemanticAdjustment::InvalidateInteriors {
            invalidations: invalidations.clone(),
        });
    }
    adjustments
}

/// The declarations of `statements`, then of each trait member's.
/// The capability a materialized temporary's hidden slot is lent with.
///
/// For the adjustments [`materialized_borrow_owner`] accepts: a
/// `MaterializeBorrowSource` records what its borrow needs; a view
/// construction lends mutably when any `ref` argument does; a parameter
/// read lends immutably; a view result keeps the conservative mutable loan.
pub fn materialized_borrow_mutability(adjustments: &[SemanticAdjustment]) -> bool {
    adjustments
        .iter()
        .find_map(|adjustment| match adjustment {
            SemanticAdjustment::MaterializeBorrowSource { mutable, .. } => Some(*mutable),
            SemanticAdjustment::BorrowRefArguments {
                arguments,
                materialized: Some(_),
            } => Some(arguments.iter().any(|(_, mutable)| *mutable)),
            SemanticAdjustment::BorrowViewResult {
                materialized: Some(_),
                ..
            }
            | SemanticAdjustment::ConstructCollection {
                materialized: Some(_),
                ..
            }
            | SemanticAdjustment::ConstructArrayLiteral {
                materialized: Some(_),
                ..
            } => Some(true),
            SemanticAdjustment::ParamValue {
                materialized: Some(_),
                ..
            } => Some(false),
            _ => None,
        })
        .unwrap_or(true)
}

/// The anonymous owner a temporary expression materializes as.
///
/// A `MaterializeBorrowSource`, or a `BorrowRefArguments` construction or
/// `BorrowViewResult` view that is also a borrow source; `None` when the
/// expression is not materialized.
pub fn materialized_borrow_owner(
    adjustments: &[SemanticAdjustment],
) -> Option<mojito_types::origin::OwnerId> {
    adjustments.iter().find_map(|adjustment| match adjustment {
        SemanticAdjustment::MaterializeBorrowSource { owner, .. } => Some(*owner),
        SemanticAdjustment::BorrowRefArguments {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::BorrowViewResult {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::ConstructCollection {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::ConstructArrayLiteral {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::ParamValue {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::PointerToPlace {
            materialized: Some(owner),
            ..
        }
        | SemanticAdjustment::PointerOffset {
            materialized: Some(owner),
        }
        | SemanticAdjustment::PointerOriginCast {
            materialized: Some(owner),
            ..
        } => Some(*owner),
        _ => None,
    })
}

/// The declaration syntax of a decoded trait member, which a package unit
/// does not hold.
fn absent_declaration() -> Stmt {
    Stmt::new(
        mojito_ast::ast::StmtKind::Pass,
        mojito_common::token::DUMMY_SPAN,
    )
}

/// One fact of every occurrence that holds it, by occurrence.
fn fact_index<V: Clone>(
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    fact: impl Fn(&NodeFacts) -> Option<&V>,
) -> HashMap<SourceSpan, V> {
    node_facts
        .iter()
        .filter_map(|(span, facts)| Some((span.clone(), fact(facts)?.clone())))
        .collect()
}

/// Write `map` sorted by key, so a record encodes the same bytes in every
/// build.
fn sorted<V: serde::Serialize, S: serde::Serializer>(
    map: &HashMap<String, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &map.iter().collect::<std::collections::BTreeMap<_, _>>(),
        serializer,
    )
}

macro_rules! node_facts {
    (
        facts { $($(#[$doc:meta])* $field:ident: $value:ty,)* }
        flags { $($(#[$flag_doc:meta])* $flag:ident,)* }
    ) => {
        /// Every fact the check recorded at one occurrence, as an op holds
        /// its own attributes.
        #[derive(Debug, Clone, Default)]
        pub struct NodeFacts {
            $($(#[$doc])* pub $field: Option<$value>,)*
            $($(#[$flag_doc])* pub $flag: bool,)*
        }

        impl NodeFacts {
            /// An occurrence the check recorded nothing at.
            pub const EMPTY: Self = Self {
                $($field: None,)*
                $($flag: false,)*
            };

            /// Whether the check recorded nothing here: every fact absent,
            /// every flag clear.
            pub const fn is_empty(&self) -> bool {
                true $(&& self.$field.is_none())* $(&& !self.$flag)*
            }
        }
    };
}

node_facts! {
    facts {
        /// The callable a call or name resolved to.
        overload_target: String,
        /// The base type a `$contextual` leading-dot sentinel resolved to.
        contextual_base: String,
        generic_instantiation: GenericInstantiation,
        method_instantiation: MethodInstantiation,
        /// Caller-substituted loan transfers of a call.
        call_transfers: Vec<CheckedCallTransfer>,
        implicit_conversion: String,
        /// The converted-to type of an implicit conversion.
        implicit_conversion_type: Ty,
        conversion_source_borrow: bool,
        conversion_raises: Ty,
        /// The struct arguments an implicit conversion's constructor takes.
        conversion_instantiation: Vec<mojito_types::types::TyArg>,
        ty: Ty,
        binding: mojito_types::origin::OwnerId,
        /// The binding a declaring statement introduces.
        statement_binding: mojito_types::origin::OwnerId,
        /// The application a local `comptime` binding denotes, which each
        /// of its reads materializes.
        comptime_value: Box<mojito_types::param_expr::ParamExpr>,
        /// A nested declaration's explicit captures.
        captures: Vec<CheckedCapture>,
        comprehension_bindings: Vec<CheckedComprehensionBinding>,
        place_ty: Ty,
        binding_ty: Ty,
        effects: EffectFacts,
        selected_call: Box<CheckedCallContract>,
        subscript_descriptor: (Vec<Option<mojito_types::types::SliceKind>>, bool),
        iteration_protocol: Box<IterationProtocol>,
        /// The protocol a `comptime for` steps the construction spelled
        /// for its iterable by (`seq.__iter__()`).
        comptime_iteration: Box<ComptimeIteration>,
        simd_construction: (SimdDtype, SimdWidth),
        /// The one operation record of the occurrence.
        adjustment: Box<SemanticAdjustment>,
        parameterized_method_call: Vec<mojito_types::types::ParamDecl>,
        tuple_unpack_plan: Vec<CheckedTupleUnpackElement>,
        /// A `match` statement's anchor and commands.
        match_plan: MatchPlan,
        /// A `match` statement's coverage warnings, in case order.
        match_warnings: Vec<Warning>,
        interior_reference: mojito_types::origin::OriginPlace,
        interior_invalidations: Vec<InteriorInvalidation>,
        /// A reference read as a value: whether it is writable.
        reference_value_use: bool,
        rebind_assertion: RebindAssertion,
        /// What each bracket argument of a call or type application
        /// resolved to, in source order: `Some` for a type-shaped one or a
        /// value-shaped one the check bound to a type, an origin, or a raw
        /// parameter list (`*ParameterList[...]()`)
        /// ([`CheckedParamArg::Attribute`]), `None` for a value-shaped one
        /// that is a run-time value, which is a node of its own.
        parameter_attributes: Vec<Option<mojito_types::types::TyArg>>,
        /// The type, origin, or raw parameter list a value-shaped bracket
        /// argument resolved to when the check bound it, which the call's
        /// `parameter_attributes` carries in its place.
        argument_attribute: mojito_types::types::TyArg,
        /// The type a type in value position resolved to.
        type_operand: Ty,
        /// The dunder each link of a chained comparison selected, in link
        /// order.
        compare_links: Vec<CheckedCompareLink>,
    }
    flags {
        explicit_destroy_call,
        copy_place_value_use,
        call_place_use,
        borrowed_read_call_place,
        read_temporary_argument,
        forwarded_argument,
        implicitly_copied_consuming_receiver,
        truthiness_condition,
        register_passable_result,
        comptime_conditional,
    }
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
