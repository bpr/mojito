//! Checked semantic handoff between the frontend and lowering.

use mojito_ast::ast::Stmt;
use mojito_ast::ast::{
    ArgConvention, CaptureList, CollectionKind, ComprehensionClause, Decorator, Expr, ExprKind,
    ImportNames, InfixOp, KwArg, LoopBindingMode, MethodProvenance, OriginSpec, Param, ParamArg,
    ParamKind, PrefixOp, SubscriptArg, TStringPart, Type, TypeParam,
};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::{SourceSpan, Span, SyntaxId};
use mojito_types::types::{SimdDtype, SimdWidth, Ty};
use std::collections::HashMap;
use std::sync::Arc;

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
/// Type arguments occupy a declaration slot with no runtime value; value
/// arguments name the checked expression occurrence whose already-evaluated
/// register lowering must reuse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckedCallParameterArgument {
    pub name: Option<String>,
    pub value_source: Option<SourceSpan>,
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
    /// The source the computed result is bound into the selected setter at,
    /// including inferred compile-time value arguments: the store's own site,
    /// since the result is no expression of the body. Absent without a setter.
    pub value_source: Option<SourceSpan>,
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
    pub mode: mojito_ast::ast::LoopBindingMode,
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
    /// their value expressions' spans
    /// ([`SemanticAdjustment::FoldedParameterArguments`]).
    pub folded_arguments: Vec<SourceSpan>,
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
    /// The bracket arguments carried as closed values, as
    /// [`GenericInstantiation::folded_arguments`] records a `def`'s.
    pub folded_arguments: Vec<SourceSpan>,
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
    pub next: Option<Box<CheckedIteratorCall>>,
    pub exhaustion: Option<Ty>,
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

/// A `comptime for` over a compile-time sequence of parameter expressions.
///
/// The loop's own binder, of the element's type, and the sequence it
/// iterates over the binders in scope. The body is checked once with the
/// binder symbolic; `native::mono` unrolls it under each instance's
/// bindings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComptimeIteration {
    pub binder: mojito_types::param_expr::ParamRef,
    pub source: ComptimeSource,
}

/// How the check leaves a `comptime for` iterable to the phases below it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ComptimeSource {
    /// A sequence the check compiled over the binders in scope.
    Sequence(ComptimeSequence),
    /// A collection display the check typed and did not close (`[n, n +
    /// 1]`), whose value binds `element`s: MIR lifts the display as a
    /// function over the binders in scope, and the loop header iterates its
    /// application, which the elaborator evaluates per instance.
    ///
    /// `construction` is the display's own checked construction, which this
    /// record stands in for on the display's span.
    Evaluated {
        element: mojito_types::param_expr::MetaTy,
        construction: Box<SemanticAdjustment>,
    },
    /// A local `comptime` binding of such a display (`comptime L = [n, n +
    /// 1]`), which the loop names: MIR lifts the display once, where the
    /// binding is declared, and every loop over the name iterates that one
    /// application.
    ///
    /// `display` is the bound display's span, which holds its
    /// [`SemanticAdjustment::ComptimeDisplay`].
    Bound {
        element: mojito_types::param_expr::MetaTy,
        display: SourceSpan,
    },
    /// A sequence a compile-time application builds (`mk(n)`), whose value
    /// binds `element`s: MIR lifts the iterable as a function over the
    /// binders in scope, as it lifts an [`Self::Evaluated`] display.
    Applied {
        element: mojito_types::param_expr::MetaTy,
    },
    /// A `range(...)` with a bound the check does not close (`range(len(L))`
    /// over a local display binding): each bound as written is its
    /// parameter expression, or `None` for one MIR lifts as a function over
    /// the binders in scope, which the elaborator evaluates per instance.
    EvaluatedRange {
        bounds: Vec<Option<mojito_types::param_expr::ParamExpr>>,
    },
}

impl ComptimeSource {
    /// The meta-type of the loop's binder.
    pub fn binder_meta(&self) -> mojito_types::param_expr::MetaTy {
        match self {
            Self::Sequence(sequence) => sequence.binder_meta(),
            Self::Evaluated { element, .. }
            | Self::Applied { element }
            | Self::Bound { element, .. } => element.clone(),
            Self::EvaluatedRange { .. } => mojito_types::param_expr::MetaTy::int(),
        }
    }
}

/// What a `comptime for` iterates, as upstream's `kgen.param.for` iterates
/// a compile-time sequence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ComptimeSequence {
    /// `range(start, stop, step)`: the integers the bounds span.
    Range {
        start: mojito_types::param_expr::ParamExpr,
        stop: mojito_types::param_expr::ParamExpr,
        step: mojito_types::param_expr::ParamExpr,
    },
    /// Any other compile-time iterable, whose value yields the elements
    /// [`mojito_types::ct::CtValue::comptime_iteration_elements`] names: a
    /// list's or a set's elements, a dictionary's keys.
    Elements(mojito_types::param_expr::ParamExpr),
}

impl ComptimeSequence {
    /// The meta-type of the loop's binder: `Int` over a range, the
    /// sequence's element otherwise. A tuple element is a value of the
    /// nominal `Tuple` over its elements' types.
    pub fn binder_meta(&self) -> mojito_types::param_expr::MetaTy {
        use mojito_types::param_expr::MetaTy;
        fn nominal(meta: &MetaTy) -> Option<mojito_types::types::Ty> {
            match meta {
                MetaTy::Tuple(elements) => elements
                    .iter()
                    .map(nominal)
                    .collect::<Option<Vec<_>>>()
                    .map(mojito_types::types::tuple_type),
                meta => meta.as_value().cloned(),
            }
        }
        match self {
            Self::Range { .. } => MetaTy::int(),
            Self::Elements(elements) => match elements.meta().iteration_element() {
                Some(element @ MetaTy::Tuple(_)) => {
                    nominal(element).map_or_else(|| element.clone(), MetaTy::value)
                }
                Some(element) => element.clone(),
                // A collection an application builds (`mk(n)`) binds its
                // element type, a string as its literal.
                None => elements
                    .meta()
                    .as_value()
                    .and_then(|ty| {
                        mojito_types::types::list_element(ty)
                            .or_else(|| mojito_types::types::set_element(ty))
                            .or_else(|| mojito_types::types::dict_elements(ty).map(|(key, _)| key))
                    })
                    .map_or_else(MetaTy::int, |element| match element.kind() {
                        mojito_types::types::TyKind::Struct(name, args)
                            if args.is_empty()
                                && mojito_types::types::is_stdlib_string_struct(name) =>
                        {
                            MetaTy::value(Ty::new(mojito_types::types::TyKind::StringLiteral))
                        }
                        _ => MetaTy::value(element.clone()),
                    }),
            },
        }
    }

    /// Every parameter expression the sequence holds.
    pub fn expressions(&self) -> Vec<&mojito_types::param_expr::ParamExpr> {
        match self {
            Self::Range { start, stop, step } => vec![start, stop, step],
            Self::Elements(elements) => vec![elements],
        }
    }
}

impl std::fmt::Display for ComptimeSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Range { start, stop, step } => write!(f, "range({start}, {stop}, {step})"),
            Self::Elements(elements) => write!(f, "{elements}"),
        }
    }
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
    /// This actual argument is an owned temporary (a call result, a
    /// construction, a converted literal) bound to a read-convention
    /// parameter: the callee borrows it, so the caller destroys it once the
    /// call returns. A temporary at a `var`/`deinit` slot moves into the
    /// callee and omits this marker.
    ReadTemporaryArgument,
    /// Concrete owner accesses performed by a callable environment during this
    /// call. The checker derives these from the selected callable and any
    /// non-escaping callable arguments; MIR translates stable owner identities
    /// to function-local slots and loan analysis treats them as call effects.
    CallableCaptureAccesses(Vec<mojito_types::origin::CaptureOrigin>),
    /// This checked compile-time argument is semantic-only. MIR retains its
    /// source position for declaration alignment but must not evaluate it into
    /// a runtime register (notably an explicitly supplied `Origin`).
    EraseCompileTimeArgument,
    /// Every compile-time argument a call of a generic `def` or a
    /// construction solved, in declaration order
    /// ([`GenericInstantiation::arguments`]), those it inferred from its
    /// arguments' types (`size(Counter[4](1))` binds `n = 4`) included: the
    /// call's MIR carries them as compile-time data, so the elaborator binds
    /// a parameter no runtime parameter or result spells and the call emits
    /// no code for one.
    InstantiatedArguments(Vec<mojito_types::types::TyArg>),
    /// The value expressions, by span, of the call's bracket arguments that
    /// [`Self::InstantiatedArguments`] carries as closed values
    /// (`tup[(1, Tag(8, "x"))]()`, `s[1 + 2]()`). As Mojo's call carries a parameter
    /// attribute, such an argument has no run-time form at the call site:
    /// MIR lowers no code for it and leaves its slot's register absent.
    FoldedParameterArguments(Vec<SourceSpan>),
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

/// The construction a sugar checked as.
///
/// See [`SemanticAdjustment::SpelledConstruction`]. Equality is the expression's
/// structural equality, an equivalence on parsed syntax (a literal is never
/// NaN).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpelledConstruction(pub Box<Expr>);

impl Eq for SpelledConstruction {}

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
}

/// One expression in the typed semantic arena.
///
/// A node holds its own operation, as an upstream op does: its opcode and
/// attributes, with each operand the node of a child expression, never a
/// copy of the child's syntax. Its location is its own occurrence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedExpr {
    /// Its index in its tables, which an encoding leaves to its position.
    #[serde(skip)]
    pub id: CheckedNodeId,
    pub op: CheckedOp,
    pub span: Span,
    #[serde(with = "mojito_ast::ast::statement_module")]
    pub source: Option<String>,
    #[serde(with = "mojito_common::token::node_syntax")]
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
/// the child's node. A type operand stays the annotation as written. A
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
    TypeValue(Type),
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
        explicit_step: bool,
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
            | Self::TypeValue(_)
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
}

/// A compile-time parameter argument of a checked call or type application.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedParamArg {
    Type(Type),
    Value(CheckedNodeId),
    Named { name: String, value: Box<Self> },
}

impl CheckedParamArg {
    fn operands(&self, operands: &mut Vec<CheckedNodeId>) {
        match self {
            Self::Type(_) => {}
            Self::Value(value) => operands.push(*value),
            Self::Named { value, .. } => value.operands(operands),
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
        explicit_step: bool,
    },
    KeywordSlice {
        name: String,
        lower: Option<CheckedNodeId>,
        upper: Option<CheckedNodeId>,
        step: Option<CheckedNodeId>,
        explicit_step: bool,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedComprehensionClause {
    For {
        var: String,
        binding: LoopBindingMode,
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
/// own occurrence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedStmt {
    /// Its index in its tables, which an encoding leaves to its position.
    #[serde(skip)]
    pub id: CheckedStmtId,
    pub op: CheckedStmtOp,
    pub span: Span,
    #[serde(with = "mojito_ast::ast::statement_module")]
    pub module: Option<String>,
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: SyntaxId,
    /// The binding the statement introduces.
    pub binding: Option<mojito_types::origin::OwnerId>,
    /// The checked type of that binding.
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
/// its statements' nodes. A signature (parameter and return types, type
/// parameters, decorators, fields) stays the syntax as written.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CheckedStmtOp {
    VarDecl {
        name: String,
        ty: Option<Type>,
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
    Unpack {
        targets: Vec<CheckedNodeId>,
        value: CheckedNodeId,
        declares: bool,
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
        type_params: Vec<TypeParam>,
        ty: Option<Type>,
        where_clauses: Vec<CheckedNodeId>,
        value: CheckedNodeId,
    },
    ComptimeIf {
        branches: Vec<(CheckedNodeId, Vec<CheckedStmtId>)>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    ComptimeFor {
        var: String,
        iter: CheckedNodeId,
        body: Vec<CheckedStmtId>,
    },
    If {
        branches: Vec<(CheckedNodeId, Vec<CheckedStmtId>)>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    While {
        cond: CheckedNodeId,
        body: Vec<CheckedStmtId>,
        orelse: Option<Vec<CheckedStmtId>>,
    },
    For {
        var: String,
        binding: LoopBindingMode,
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
            Self::Unpack { targets, value, .. } => {
                targets.iter().copied().chain([*value]).collect()
            }
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
            Self::ComptimeIf { branches, .. } | Self::If { branches, .. } => {
                branches.iter().map(|(condition, _)| *condition).collect()
            }
            Self::ComptimeFor { iter, .. } | Self::For { iter, .. } => vec![*iter],
            Self::While { cond, .. } => vec![*cond],
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
}

/// A checked `def`: its signature as written, its parameter defaults and
/// constraints as nodes, and its body.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedDef {
    pub name: String,
    pub decorators: Vec<Decorator>,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub captures: Option<CaptureList>,
    pub raises: bool,
    pub raises_type: Option<Type>,
    pub ret: Option<Type>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub body: Vec<CheckedStmtId>,
}

/// A function parameter whose default is a node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedFnParam {
    pub name: String,
    pub ty: Type,
    pub default: Option<CheckedNodeId>,
    pub kind: ParamKind,
    pub convention: Option<ArgConvention>,
    pub origin: Option<OriginSpec>,
}

/// A checked `struct`: its members, with their bodies and constraints as
/// nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedStruct {
    pub name: String,
    pub decorators: Vec<Decorator>,
    pub type_params: Vec<TypeParam>,
    pub conforms: Vec<String>,
    pub callable_conformance: Option<Type>,
    pub conformance_conditions: Vec<(String, CheckedNodeId)>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub fields: Vec<Param>,
    pub associated: Vec<CheckedStructComptime>,
    pub methods: Vec<CheckedMethod>,
    pub fieldwise_init: bool,
}

/// A struct's associated `comptime` member.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedStructComptime {
    pub name: String,
    pub params: Vec<TypeParam>,
    pub ty: Option<Type>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub value: CheckedNodeId,
}

/// A struct method, with its body as statement nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedMethod {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub has_self: bool,
    pub self_convention: Option<ArgConvention>,
    pub self_origin: Option<OriginSpec>,
    pub decorators: Vec<Decorator>,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub raises_type: Option<Type>,
    pub ret: Option<Type>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub body: Vec<CheckedStmtId>,
    pub provenance: MethodProvenance,
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

/// A trait method requirement, and its default's body when it has one.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTraitMethod {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub self_convention: Option<ArgConvention>,
    pub self_origin: Option<OriginSpec>,
    pub params: Vec<CheckedFnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub raises_type: Option<Type>,
    pub ret: Option<Type>,
    pub where_clauses: Vec<CheckedNodeId>,
    pub default_body: Option<Vec<CheckedStmtId>>,
}

/// A trait's `comptime` member requirement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedTraitComptime {
    pub name: String,
    pub params: Vec<TypeParam>,
    pub ty: Type,
    pub where_clauses: Vec<CheckedNodeId>,
}

/// One `with` item: its context as a node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedWithItem {
    pub context: CheckedNodeId,
    pub var: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckedDeclaration {
    /// Its index in its tables, which an encoding leaves to its position.
    #[serde(skip)]
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

/// The whole program's checked expression and statement nodes and its
/// declarations.
///
/// Every function's HIR and MIR lowering shares them through one `Arc`:
/// lowering a function never copies program-wide tables. A package unit
/// encodes its own, less the location indexes, which
/// [`Self::reindexed`] rebuilds from the nodes.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CheckedTables {
    /// Indexed by `CheckedNodeId`.
    pub expressions: Vec<CheckedExpr>,
    /// Indexed by `CheckedStmtId`.
    pub statements: Vec<CheckedStmt>,
    /// Indexed by `CheckedDeclId`.
    pub declarations: Vec<CheckedDeclaration>,
    /// The module's root statements, parallel to
    /// [`CheckedProgram::statements`].
    roots: Vec<CheckedStmtId>,
    /// Each default wrapper's root statements, parallel to
    /// [`CheckedProgram::default_wrappers`].
    wrapper_roots: Vec<Vec<CheckedStmtId>>,
    /// Each location a spelled sugar was written at, beside the node of
    /// the construction the check spelled for it, which answers there too.
    aliases: Vec<(SourceSpan, CheckedNodeId)>,
    /// Nodes whose syntax sits at a source location (several may share one).
    #[serde(skip)]
    expressions_by_span: HashMap<SourceSpan, Vec<CheckedNodeId>>,
    /// The last statement node at a source location.
    #[serde(skip)]
    statements_by_span: HashMap<SourceSpan, CheckedStmtId>,
}

impl CheckedTables {
    pub fn expression(&self, id: CheckedNodeId) -> Option<&CheckedExpr> {
        self.expressions.get(id.0 as usize)
    }

    /// The syntax of node `id` and its operands, rebuilt from their ops: a
    /// spelled sugar's place holds the construction the check spelled for
    /// it. Transitional, for HIR, which still lowers syntax; R623 lowers
    /// from the ops and deletes it.
    pub fn syntax_of(&self, id: CheckedNodeId) -> Option<Expr> {
        let node = self.expression(id)?;
        let child = |id: &CheckedNodeId| self.syntax_of(*id);
        let boxed = |id: &CheckedNodeId| child(id).map(Box::new);
        let optional =
            |id: &Option<CheckedNodeId>| id.as_ref().map_or(Some(None), |id| boxed(id).map(Some));
        let values = |ids: &[CheckedNodeId]| ids.iter().map(child).collect::<Option<Vec<_>>>();
        let kwargs = |kwargs: &[CheckedKwArg]| {
            kwargs
                .iter()
                .map(|kwarg| {
                    Some(KwArg {
                        name: kwarg.name.clone(),
                        value: child(&kwarg.value)?,
                    })
                })
                .collect::<Option<Vec<_>>>()
        };
        let param_args = |arguments: &[CheckedParamArg]| {
            arguments
                .iter()
                .map(|argument| self.param_arg_syntax(argument))
                .collect::<Option<Vec<_>>>()
        };
        let kind = match &node.op {
            CheckedOp::Int(value) => ExprKind::Int(value.clone()),
            CheckedOp::Float(value) => ExprKind::Float(value.clone()),
            CheckedOp::Bool(value) => ExprKind::Bool(*value),
            CheckedOp::Str(value) => ExprKind::Str(value.clone()),
            CheckedOp::None => ExprKind::None,
            CheckedOp::Uninitialized => ExprKind::Uninitialized,
            CheckedOp::EmptySubscript => ExprKind::EmptySubscript,
            CheckedOp::Identifier(name) => ExprKind::Identifier(name.clone()),
            CheckedOp::TypeValue(ty) => ExprKind::TypeValue(ty.clone()),
            CheckedOp::Prefix(op, value) => ExprKind::Prefix(*op, boxed(value)?),
            CheckedOp::Infix(op, left, right) => ExprKind::Infix(*op, boxed(left)?, boxed(right)?),
            CheckedOp::Call {
                name,
                param_args: parameters,
                args,
                kwargs: keywords,
            } => ExprKind::Call {
                name: name.clone(),
                param_args: param_args(parameters)?,
                args: values(args)?,
                kwargs: kwargs(keywords)?,
            },
            CheckedOp::Invoke {
                callee,
                param_args: parameters,
                args,
                kwargs: keywords,
            } => ExprKind::Invoke {
                callee: boxed(callee)?,
                param_args: param_args(parameters)?,
                args: values(args)?,
                kwargs: kwargs(keywords)?,
            },
            CheckedOp::Member { object, field } => ExprKind::Member {
                object: boxed(object)?,
                field: field.clone(),
            },
            CheckedOp::MethodCall {
                object,
                method,
                args,
                kwargs: keywords,
            } => ExprKind::MethodCall {
                object: boxed(object)?,
                method: method.clone(),
                args: values(args)?,
                kwargs: kwargs(keywords)?,
            },
            CheckedOp::Index { object, index } => ExprKind::Index {
                object: boxed(object)?,
                index: boxed(index)?,
            },
            CheckedOp::TypeApply { name, args } => ExprKind::TypeApply {
                name: name.clone(),
                args: param_args(args)?,
            },
            CheckedOp::Spread(value) => ExprKind::Spread(boxed(value)?),
            CheckedOp::Transfer(value) => ExprKind::Transfer(boxed(value)?),
            CheckedOp::ListLit(items) => ExprKind::ListLit(values(items)?),
            CheckedOp::TupleLit(items) => ExprKind::TupleLit(values(items)?),
            CheckedOp::BraceLit(entries) => ExprKind::BraceLit(
                entries
                    .iter()
                    .map(|(key, value)| {
                        Some((
                            child(key)?,
                            value
                                .as_ref()
                                .map_or(Some(None), |id| child(id).map(Some))?,
                        ))
                    })
                    .collect::<Option<_>>()?,
            ),
            CheckedOp::Comprehension {
                kind,
                key,
                value,
                clauses,
            } => ExprKind::Comprehension {
                kind: *kind,
                key: optional(key)?,
                value: boxed(value)?,
                clauses: clauses
                    .iter()
                    .map(|clause| {
                        Some(match clause {
                            CheckedComprehensionClause::For { var, binding, iter } => {
                                ComprehensionClause::For {
                                    var: var.clone(),
                                    binding: *binding,
                                    iter: boxed(iter)?,
                                }
                            }
                            CheckedComprehensionClause::If(condition) => {
                                ComprehensionClause::If(boxed(condition)?)
                            }
                        })
                    })
                    .collect::<Option<_>>()?,
            },
            CheckedOp::Lambda { def } => ExprKind::Lambda {
                def: Box::new(self.statement_syntax_of(*def)?),
            },
            CheckedOp::Named { name, value } => ExprKind::Named {
                name: name.clone(),
                value: boxed(value)?,
            },
            CheckedOp::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => ExprKind::IfExpr {
                cond: boxed(cond)?,
                then_branch: boxed(then_branch)?,
                else_branch: boxed(else_branch)?,
            },
            CheckedOp::Compare { first, rest } => ExprKind::Compare {
                first: boxed(first)?,
                rest: rest
                    .iter()
                    .map(|(op, value)| Some((*op, child(value)?)))
                    .collect::<Option<_>>()?,
            },
            CheckedOp::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => ExprKind::Slice {
                object: boxed(object)?,
                lower: optional(lower)?,
                upper: optional(upper)?,
                step: optional(step)?,
                explicit_step: *explicit_step,
            },
            CheckedOp::MultiIndex { object, args } => ExprKind::MultiIndex {
                object: boxed(object)?,
                args: args
                    .iter()
                    .map(|argument| {
                        Some(match argument {
                            CheckedSubscriptArg::Index(value) => SubscriptArg::Index(child(value)?),
                            CheckedSubscriptArg::Keyword { name, value } => SubscriptArg::Keyword {
                                name: name.clone(),
                                value: child(value)?,
                            },
                            CheckedSubscriptArg::Slice {
                                lower,
                                upper,
                                step,
                                explicit_step,
                            } => SubscriptArg::Slice {
                                lower: optional(lower)?,
                                upper: optional(upper)?,
                                step: optional(step)?,
                                explicit_step: *explicit_step,
                            },
                            CheckedSubscriptArg::KeywordSlice {
                                name,
                                lower,
                                upper,
                                step,
                                explicit_step,
                            } => SubscriptArg::KeywordSlice {
                                name: name.clone(),
                                lower: optional(lower)?,
                                upper: optional(upper)?,
                                step: optional(step)?,
                                explicit_step: *explicit_step,
                            },
                        })
                    })
                    .collect::<Option<_>>()?,
            },
            CheckedOp::TString { parts, raw } => ExprKind::TString {
                parts: parts
                    .iter()
                    .map(|part| {
                        Some(match part {
                            CheckedTStringPart::Literal(text) => TStringPart::Literal(text.clone()),
                            CheckedTStringPart::Expr(value) => TStringPart::Expr(boxed(value)?),
                        })
                    })
                    .collect::<Option<_>>()?,
                raw: *raw,
            },
        };
        Some(Expr {
            kind,
            span: node.span,
            source: node.source.clone(),
            syntax_id: node.syntax_id,
        })
    }

    /// Nodes whose syntax sits at `span`, in checked order. Several nodes may
    /// share a location, so the span itself is never semantic identity.
    pub fn expression_ids_at(&self, span: &SourceSpan) -> &[CheckedNodeId] {
        self.expressions_by_span
            .get(span)
            .map_or(&[][..], Vec::as_slice)
    }

    pub fn declaration(&self, id: CheckedDeclId) -> Option<&CheckedDeclaration> {
        self.declarations.get(id.0 as usize)
    }

    pub fn statement(&self, id: CheckedStmtId) -> Option<&CheckedStmt> {
        self.statements.get(id.0 as usize)
    }

    /// The module's root statements, parallel to
    /// [`CheckedProgram::statements`].
    pub fn roots(&self) -> &[CheckedStmtId] {
        &self.roots
    }

    /// The root statements of default wrapper `index`, parallel to
    /// [`CheckedProgram::default_wrappers`].
    pub fn wrapper_roots(&self, index: usize) -> &[CheckedStmtId] {
        self.wrapper_roots.get(index).map_or(&[][..], Vec::as_slice)
    }

    /// The statement node at a statement's source location. Transitional,
    /// for HIR's and MIR's syntax walks; R623 deletes it once both walk
    /// statement nodes.
    pub fn statement_at(&self, span: &SourceSpan) -> Option<&CheckedStmt> {
        self.statements_by_span
            .get(span)
            .and_then(|id| self.statement(*id))
    }

    /// The syntax of statement `id` and its bodies, rebuilt from their ops,
    /// each expression through [`Self::syntax_of`]. Transitional, as
    /// `syntax_of` is; R623 deletes it.
    pub fn statement_syntax_of(&self, id: CheckedStmtId) -> Option<Stmt> {
        use mojito_ast::ast::{
            FnParam, Method, StmtKind, StructComptime, TraitComptime, TraitMethod, WithItem,
        };
        let node = self.statement(id)?;
        let expr = |id: &CheckedNodeId| self.syntax_of(*id);
        let exprs = |ids: &[CheckedNodeId]| ids.iter().map(expr).collect::<Option<Vec<_>>>();
        let block = |ids: &[CheckedStmtId]| {
            ids.iter()
                .map(|id| self.statement_syntax_of(*id))
                .collect::<Option<Vec<_>>>()
        };
        let optional_block = |ids: &Option<Vec<CheckedStmtId>>| {
            ids.as_ref().map_or(Some(None), |ids| block(ids).map(Some))
        };
        let params = |params: &[CheckedFnParam]| {
            params
                .iter()
                .map(|param| {
                    Some(FnParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                        default: param
                            .default
                            .as_ref()
                            .map_or(Some(None), |id| expr(id).map(Some))?,
                        kind: param.kind,
                        convention: param.convention,
                        origin: param.origin.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()
        };
        let branches = |branches: &[(CheckedNodeId, Vec<CheckedStmtId>)]| {
            branches
                .iter()
                .map(|(condition, body)| Some((expr(condition)?, block(body)?)))
                .collect::<Option<Vec<_>>>()
        };
        let kind = match &node.op {
            CheckedStmtOp::VarDecl { name, ty, value } => StmtKind::VarDecl {
                name: name.clone(),
                ty: ty.clone(),
                value: expr(value)?,
            },
            CheckedStmtOp::RefDecl { name, value } => StmtKind::RefDecl {
                name: name.clone(),
                value: expr(value)?,
            },
            CheckedStmtOp::Assign { name, value } => StmtKind::Assign {
                name: name.clone(),
                value: expr(value)?,
            },
            CheckedStmtOp::AugAssign { place, op, value } => StmtKind::AugAssign {
                place: expr(place)?,
                op: *op,
                value: expr(value)?,
            },
            CheckedStmtOp::Unpack {
                targets,
                value,
                declares,
            } => StmtKind::Unpack {
                targets: exprs(targets)?,
                value: expr(value)?,
                declares: *declares,
            },
            CheckedStmtOp::SetPlace { place, value } => StmtKind::SetPlace {
                place: expr(place)?,
                value: expr(value)?,
            },
            CheckedStmtOp::Def(def) => StmtKind::Def {
                name: def.name.clone(),
                decorators: def.decorators.clone(),
                type_params: def.type_params.clone(),
                params: params(&def.params)?,
                positional_only: def.positional_only,
                keyword_only: def.keyword_only,
                captures: def.captures.clone(),
                raises: def.raises,
                raises_type: def.raises_type.clone(),
                ret: def.ret.clone(),
                where_clauses: exprs(&def.where_clauses)?,
                body: block(&def.body)?,
            },
            CheckedStmtOp::Struct(declaration) => StmtKind::Struct {
                name: declaration.name.clone(),
                decorators: declaration.decorators.clone(),
                type_params: declaration.type_params.clone(),
                conforms: declaration.conforms.clone(),
                callable_conformance: declaration.callable_conformance.clone(),
                conformance_conditions: declaration
                    .conformance_conditions
                    .iter()
                    .map(|(name, value)| Some((name.clone(), expr(value)?)))
                    .collect::<Option<_>>()?,
                where_clauses: exprs(&declaration.where_clauses)?,
                fields: declaration.fields.clone(),
                associated: declaration
                    .associated
                    .iter()
                    .map(|member| {
                        Some(StructComptime {
                            name: member.name.clone(),
                            params: member.params.clone(),
                            ty: member.ty.clone(),
                            where_clauses: exprs(&member.where_clauses)?,
                            value: expr(&member.value)?,
                        })
                    })
                    .collect::<Option<_>>()?,
                methods: declaration
                    .methods
                    .iter()
                    .map(|method| {
                        Some(Method {
                            name: method.name.clone(),
                            type_params: method.type_params.clone(),
                            has_self: method.has_self,
                            self_convention: method.self_convention,
                            self_origin: method.self_origin.clone(),
                            decorators: method.decorators.clone(),
                            params: params(&method.params)?,
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            raises_type: method.raises_type.clone(),
                            ret: method.ret.clone(),
                            where_clauses: exprs(&method.where_clauses)?,
                            body: block(&method.body)?,
                            provenance: method.provenance,
                        })
                    })
                    .collect::<Option<_>>()?,
                fieldwise_init: declaration.fieldwise_init,
            },
            CheckedStmtOp::Trait(declaration) => StmtKind::Trait {
                name: declaration.name.clone(),
                refines: declaration.refines.clone(),
                methods: declaration
                    .methods
                    .iter()
                    .map(|method| {
                        Some(TraitMethod {
                            name: method.name.clone(),
                            type_params: method.type_params.clone(),
                            self_convention: method.self_convention,
                            self_origin: method.self_origin.clone(),
                            params: params(&method.params)?,
                            positional_only: method.positional_only,
                            keyword_only: method.keyword_only,
                            raises: method.raises,
                            raises_type: method.raises_type.clone(),
                            ret: method.ret.clone(),
                            where_clauses: exprs(&method.where_clauses)?,
                            default_body: optional_block(&method.default_body)?,
                        })
                    })
                    .collect::<Option<_>>()?,
                comptime_members: declaration
                    .comptime_members
                    .iter()
                    .map(|member| {
                        Some(TraitComptime {
                            name: member.name.clone(),
                            params: member.params.clone(),
                            ty: member.ty.clone(),
                            where_clauses: exprs(&member.where_clauses)?,
                        })
                    })
                    .collect::<Option<_>>()?,
            },
            CheckedStmtOp::Comptime {
                name,
                type_params,
                ty,
                where_clauses,
                value,
            } => StmtKind::Comptime {
                name: name.clone(),
                type_params: type_params.clone(),
                ty: ty.clone(),
                where_clauses: exprs(where_clauses)?,
                value: expr(value)?,
            },
            CheckedStmtOp::ComptimeIf {
                branches: arms,
                orelse,
            } => StmtKind::ComptimeIf {
                branches: branches(arms)?,
                orelse: optional_block(orelse)?,
            },
            CheckedStmtOp::ComptimeFor { var, iter, body } => StmtKind::ComptimeFor {
                var: var.clone(),
                iter: expr(iter)?,
                body: block(body)?,
            },
            CheckedStmtOp::If {
                branches: arms,
                orelse,
            } => StmtKind::If {
                branches: branches(arms)?,
                orelse: optional_block(orelse)?,
            },
            CheckedStmtOp::While { cond, body, orelse } => StmtKind::While {
                cond: expr(cond)?,
                body: block(body)?,
                orelse: optional_block(orelse)?,
            },
            CheckedStmtOp::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => StmtKind::For {
                var: var.clone(),
                binding: *binding,
                iter: expr(iter)?,
                body: block(body)?,
                orelse: optional_block(orelse)?,
            },
            CheckedStmtOp::Return(value) => {
                StmtKind::Return(value.as_ref().map_or(Some(None), |id| expr(id).map(Some))?)
            }
            CheckedStmtOp::Raise(value) => StmtKind::Raise(expr(value)?),
            CheckedStmtOp::Import { path, alias } => StmtKind::Import {
                path: path.clone(),
                alias: alias.clone(),
            },
            CheckedStmtOp::FromImport { level, path, names } => StmtKind::FromImport {
                level: *level,
                path: path.clone(),
                names: names.clone(),
            },
            CheckedStmtOp::With { items, body } => StmtKind::With {
                items: items
                    .iter()
                    .map(|item| {
                        Some(WithItem {
                            context: expr(&item.context)?,
                            var: item.var.clone(),
                        })
                    })
                    .collect::<Option<_>>()?,
                body: block(body)?,
            },
            CheckedStmtOp::Try {
                body,
                except,
                orelse,
                finalbody,
            } => StmtKind::Try {
                body: block(body)?,
                except: except.as_ref().map_or(Some(None), |(name, body)| {
                    block(body).map(|body| Some((name.clone(), body)))
                })?,
                orelse: optional_block(orelse)?,
                finalbody: optional_block(finalbody)?,
            },
            CheckedStmtOp::Scope(body) => StmtKind::Scope(block(body)?),
            CheckedStmtOp::Pass => StmtKind::Pass,
            CheckedStmtOp::Break => StmtKind::Break,
            CheckedStmtOp::Continue => StmtKind::Continue,
            CheckedStmtOp::Expr(value) => StmtKind::Expr(expr(value)?),
        };
        Some(Stmt {
            kind,
            span: node.span,
            module: node.module.clone(),
            syntax_id: node.syntax_id,
        })
    }

    /// `expressions_by_span` is the arena builder's index: every node at its
    /// own location, and a construction the checker spelled for a source
    /// expression (an initializer list's) at that expression's location too.
    /// Each statement node takes the last declaration recorded at its
    /// location.
    fn new(arena: CheckedArena, declarations: Vec<CheckedDeclaration>) -> Self {
        let CheckedArena {
            nodes: expressions,
            index: expressions_by_span,
            mut statements,
            roots,
            wrapper_roots,
            aliases,
            statements_by_span,
        } = arena;
        let declared: HashMap<&SourceSpan, CheckedDeclId> = declarations
            .iter()
            .map(|declaration| (&declaration.location, declaration.id))
            .collect();
        for statement in &mut statements {
            statement.declaration = declared.get(&statement.location()).copied();
        }
        drop(declared);
        Self {
            expressions,
            statements,
            declarations,
            roots,
            wrapper_roots,
            aliases,
            expressions_by_span,
            statements_by_span,
        }
    }

    /// The tables with each node's identity its position, and their
    /// location indexes rebuilt from the nodes as the arena builder fills
    /// them: each node at its own location in build order, then each
    /// alias.
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
        self.expressions_by_span.clear();
        for node in &self.expressions {
            self.expressions_by_span
                .entry(node.location())
                .or_default()
                .push(node.id);
        }
        for (location, node) in &self.aliases {
            self.expressions_by_span
                .entry(location.clone())
                .or_default()
                .push(*node);
        }
        self.statements_by_span = self
            .statements
            .iter()
            .map(|statement| (statement.location(), statement.id))
            .collect();
        self
    }

    fn param_arg_syntax(&self, argument: &CheckedParamArg) -> Option<ParamArg> {
        Some(match argument {
            CheckedParamArg::Type(ty) => ParamArg::Type(ty.clone()),
            CheckedParamArg::Value(value) => ParamArg::Value(self.syntax_of(*value)?),
            CheckedParamArg::Named { name, value } => ParamArg::Named {
                name: name.clone(),
                value: Box::new(self.param_arg_syntax(value)?),
            },
        })
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
    /// A package unit holds the body as its checked nodes, and
    /// [`CheckedProgram::installed`] rebuilds it from them.
    #[serde(skip)]
    pub body: Vec<Stmt>,
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
    /// `(self: _Self, ...)`, whose body is the default's. A package unit
    /// holds it as its checked nodes, and [`CheckedProgram::installed`]
    /// rebuilds it from them.
    #[serde(skip, default = "unrebuilt_declaration")]
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
    fn default_value(&self) -> Option<&Expr> {
        None
    }
}

/// What one check of an elaborated program established, before the executable
/// handoff is assembled.
///
/// Discovery needs a round's requests, not its checked expression arena, and
/// every round but the last is re-elaborated and checked again. A round
/// therefore ends here: every rejecting check has already run, the request
/// queries below read the recorded facts directly, and only the round that
/// converges pays for [`Self::finalize`]. This is not an executable program
/// and never reaches HIR or MIR.
///
/// It is [`CheckedProgram::new`]'s input: the check's facts, one record per
/// occurrence ([`NodeFacts`]), beside its declaration-site tables.
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

    /// Visit every expression the checked arena would hold a node for, in the
    /// arena's own order. The traversal is the arena builder's, so a request
    /// query here sees exactly the expressions one over
    /// [`CheckedProgram::expressions`] would.
    pub fn scan_expressions(&self, visit: &mut dyn FnMut(&Expr)) {
        build_checked_expressions(
            &self.statements,
            &self.node_facts,
            &self.default_wrappers,
            Some(visit),
        );
    }

    /// The generic instantiation recorded at each call.
    pub fn generic_instantiations(&self) -> HashMap<SourceSpan, GenericInstantiation> {
        fact_index(&self.node_facts, |facts| {
            facts.generic_instantiation.as_ref()
        })
    }

    /// The method instantiation recorded at each call.
    pub fn method_instantiations(&self) -> HashMap<SourceSpan, MethodInstantiation> {
        fact_index(&self.node_facts, |facts| {
            facts.method_instantiation.as_ref()
        })
    }

    pub fn struct_instantiations(&self) -> &[StructInstantiation] {
        &self.struct_instantiations
    }

    pub fn expression_type(&self, expression: &Expr) -> Option<&Ty> {
        self.facts_of(expression)?.ty.as_ref()
    }

    pub fn expression_place_type(&self, expression: &Expr) -> Option<&Ty> {
        self.facts_of(expression)?.place_ty.as_ref()
    }

    pub fn expression_binding_type(&self, expression: &Expr) -> Option<&Ty> {
        self.facts_of(expression)?.binding_ty.as_ref()
    }

    /// The checked type of every declaration the arena would hold.
    pub fn declaration_types(&self) -> Vec<Ty> {
        build_checked_declarations(
            &self.statements,
            &self.trait_members,
            &self.checked_types,
            &self.node_facts,
        )
        .into_iter()
        .filter_map(|declaration| declaration.ty)
        .collect()
    }

    /// [`Self::scan_expressions`] over `statements` and the bodies of
    /// `wrappers` in place of the program's.
    pub(crate) fn scan_unit_expressions(
        &self,
        statements: &[Stmt],
        wrappers: &[CheckedDefaultWrapper],
        visit: &mut dyn FnMut(&Expr),
    ) {
        build_checked_expressions(statements, &self.node_facts, wrappers, Some(visit));
    }

    fn facts_of(&self, expression: &Expr) -> Option<&NodeFacts> {
        self.node_facts.get(&expression.source_span())
    }
}

/// A successfully checked program plus semantic facts that downstream phases
/// previously recomputed from AST syntax or checker-private side tables.
#[derive(Debug, Clone)]
pub struct CheckedProgram {
    statements: Vec<Stmt>,
    compatibility_overload_targets: HashMap<SourceSpan, String>,
    generic_instantiations: HashMap<SourceSpan, GenericInstantiation>,
    method_instantiations: HashMap<SourceSpan, MethodInstantiation>,
    struct_instantiations: Vec<StructInstantiation>,
    /// Caller-substituted loan transfers per call occurrence, keyed by the
    /// call expression's span; MIR lowering installs the implied loans.
    call_transfers: HashMap<SourceSpan, Vec<CheckedCallTransfer>>,
    compatibility_implicit_conversions: HashMap<SourceSpan, String>,
    /// The converted-to checked type of each implicit conversion site.
    implicit_conversion_types: HashMap<SourceSpan, Ty>,
    checked_types: HashMap<AnnotationSite, Ty>,
    generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    /// Each struct's `Origin` and `OriginSet` parameters, by name in
    /// declaration order, which its `generic_parameters` leave out.
    origin_parameters: HashMap<GenericSite, Vec<String>>,
    tables: Arc<CheckedTables>,
    expression_index: HashMap<SourceSpan, Vec<CheckedNodeId>>,
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
    /// Each struct's compile-time value members (`K` of `comptime K = 7`),
    /// sorted by member, each a parameter expression over the struct's own
    /// binders (`Self.n + 1`). The elaborator answers `T.K` from them once
    /// `T` is bound to an instance.
    pub associated_values: HashMap<String, Vec<(String, mojito_types::param_expr::ParamExpr)>>,
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

    /// The literal a conversion default converts, or the default itself.
    pub fn unconverted(&self) -> &Self {
        match self {
            Self::Construct { arg, .. } => arg.unconverted(),
            _ => self,
        }
    }
}

impl CheckedProgram {
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
            default_wrappers,
            trait_members,
        } = discovery;
        let expressions_span = mojito_common::timing::span("expressions");
        let arena = build_checked_expressions(&statements, &node_facts, &default_wrappers, None);
        let expression_index = arena.index.clone();
        drop(expressions_span);
        let _declarations = mojito_common::timing::span("declarations");
        let declarations =
            build_checked_declarations(&statements, &trait_members, &checked_types, &node_facts);
        let origin_parameters = statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                mojito_ast::ast::StmtKind::Struct {
                    name, type_params, ..
                } => Some((
                    GenericSite::Struct {
                        module: statement.module.clone(),
                        declaration: name.clone(),
                    },
                    origin_parameter_names(type_params),
                )),
                _ => None,
            })
            .collect();
        Self {
            statements,
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
            call_transfers: fact_index(&node_facts, |facts| facts.call_transfers.as_ref()),
            compatibility_implicit_conversions: fact_index(&node_facts, |facts| {
                facts.implicit_conversion.as_ref()
            }),
            implicit_conversion_types: fact_index(&node_facts, |facts| {
                facts.implicit_conversion_type.as_ref()
            }),
            checked_types,
            generic_parameters,
            origin_parameters,
            tables: Arc::new(CheckedTables::new(arena, declarations)),
            expression_index,
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
    /// Its statements, and the wrapper's body and the member's declaration,
    /// are rebuilt from the nodes, which HIR and MIR still walk as syntax.
    /// The location maps unit lowering reads are built from the nodes'
    /// attributes; the instantiation tables, which only whole-program
    /// passes read, are empty.
    ///
    /// # Errors
    ///
    /// When a root names no statement node.
    pub fn installed(
        tables: CheckedTables,
        sites: InstalledSites,
        overloads: Arc<crate::package::ProgramOverloads>,
        constants: Arc<crate::package::ProgramConstants>,
        mut wrapper: Option<CheckedDefaultWrapper>,
        mut trait_member: Option<CheckedTraitMember>,
    ) -> Result<Self, String> {
        let tables = tables.reindexed();
        let rebuilt = |roots: &[CheckedStmtId]| {
            roots
                .iter()
                .map(|root| tables.statement_syntax_of(*root))
                .collect::<Option<Vec<_>>>()
                .ok_or("a unit's root names no statement node")
        };
        let statements = rebuilt(tables.roots())?;
        if let Some(wrapper) = &mut wrapper {
            wrapper.body = rebuilt(tables.wrapper_roots(0))?;
        }
        if let Some(member) = &mut trait_member {
            member.declaration = statements
                .first()
                .cloned()
                .ok_or("a trait member's unit has no declaration")?;
        }
        let mut overload_targets = HashMap::new();
        let mut call_transfers = HashMap::new();
        let mut implicit_conversions = HashMap::new();
        let mut implicit_conversion_types = HashMap::new();
        for node in &tables.expressions {
            let location = node.location();
            if !node.call_transfers.is_empty() {
                call_transfers.insert(location.clone(), node.call_transfers.clone());
            }
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
            origin_parameters,
        } = sites;
        Ok(Self {
            statements,
            compatibility_overload_targets: overload_targets,
            generic_instantiations: HashMap::new(),
            method_instantiations: HashMap::new(),
            struct_instantiations: Vec::new(),
            call_transfers,
            compatibility_implicit_conversions: implicit_conversions,
            implicit_conversion_types,
            checked_types,
            generic_parameters,
            origin_parameters,
            expression_index: tables.expressions_by_span.clone(),
            tables: Arc::new(tables),
            explicit_destroy_types: HashMap::new(),
            struct_destructors: HashMap::new(),
            conformances: ConformanceFacts::default(),
            declaration_effects,
            overloads,
            constants,
            default_wrappers: wrapper.into_iter().collect(),
            trait_members: trait_member.into_iter().collect(),
        })
    }

    /// Elaborated statements accepted by semantic checking.
    pub fn statements(&self) -> &[Stmt] {
        &self.statements
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

    /// The caller-substituted loan transfers per call occurrence.
    pub const fn call_transfers(&self) -> &HashMap<SourceSpan, Vec<CheckedCallTransfer>> {
        &self.call_transfers
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

    /// The `Origin` and `OriginSet` parameters of the struct `site`
    /// names, in declaration order.
    pub fn origin_parameters_at(&self, site: &GenericSite) -> &[String] {
        self.origin_parameters
            .get(site)
            .map_or(&[][..], Vec::as_slice)
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

    /// Compatibility lookup while HIR migrates from syntax to checked ids.
    /// Multiple nodes may share a source location, so callers must never use the
    /// span itself as semantic identity.
    pub fn expression_ids_at(&self, span: &SourceSpan) -> &[CheckedNodeId] {
        self.expression_index
            .get(span)
            .map_or(&[][..], Vec::as_slice)
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
}

/// The declaration-site facts a package unit's program is installed with
/// ([`CheckedProgram::installed`]).
#[derive(Debug, Default)]
pub struct InstalledSites {
    pub checked_types: HashMap<AnnotationSite, Ty>,
    pub generic_parameters: HashMap<GenericSite, Vec<mojito_types::types::ParamDecl>>,
    pub declaration_effects: HashMap<AnnotationSite, DeclarationEffect>,
    pub origin_parameters: HashMap<GenericSite, Vec<String>>,
}

/// The names of the `Origin` and `OriginSet` parameters among
/// `parameters`, in order.
pub fn origin_parameter_names<'a>(
    parameters: impl IntoIterator<Item = &'a TypeParam>,
) -> Vec<String> {
    parameters
        .into_iter()
        .filter(|parameter| {
            matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
        })
        .map(|parameter| parameter.name.clone())
        .collect()
}

/// The checked tables of one package unit, `statements` and `wrappers`'
/// bodies, built from the check's `facts` and `checked_types` as the whole
/// program's are.
pub(crate) fn unit_tables(
    statements: &[Stmt],
    wrappers: &[CheckedDefaultWrapper],
    facts: &HashMap<SourceSpan, NodeFacts>,
    checked_types: &HashMap<AnnotationSite, Ty>,
) -> CheckedTables {
    let arena = build_checked_expressions(statements, facts, wrappers, None);
    let declarations = build_checked_declarations(statements, &[], checked_types, facts);
    CheckedTables::new(arena, declarations)
}

/// The checked arena of `statements` and of `default_wrappers`' bodies, each
/// node holding the facts `facts` recorded at its occurrence, and the nodes
/// at each location; with `scan`, the traversal alone, each expression
/// reported instead of built.
fn build_checked_expressions(
    statements: &[Stmt],
    facts: &HashMap<SourceSpan, NodeFacts>,
    default_wrappers: &[CheckedDefaultWrapper],
    scan: Option<&mut dyn FnMut(&Expr)>,
) -> CheckedArena {
    let mut builder = ArenaBuilder {
        scan,
        nodes: Vec::new(),
        index: HashMap::new(),
        statements: Vec::new(),
        aliases: Vec::new(),
        statements_by_span: HashMap::new(),
        facts,
    };
    let roots = builder.block(statements);
    let wrapper_roots = default_wrappers
        .iter()
        .map(|wrapper| builder.block(&wrapper.body))
        .collect();
    CheckedArena {
        nodes: builder.nodes,
        index: builder.index,
        statements: builder.statements,
        roots,
        wrapper_roots,
        aliases: builder.aliases,
        statements_by_span: builder.statements_by_span,
    }
}

/// What [`build_checked_expressions`] builds.
struct CheckedArena {
    nodes: Vec<CheckedExpr>,
    index: HashMap<SourceSpan, Vec<CheckedNodeId>>,
    statements: Vec<CheckedStmt>,
    roots: Vec<CheckedStmtId>,
    wrapper_roots: Vec<Vec<CheckedStmtId>>,
    aliases: Vec<(SourceSpan, CheckedNodeId)>,
    statements_by_span: HashMap<SourceSpan, CheckedStmtId>,
}

/// The checked arena under construction.
struct ArenaBuilder<'a, 's> {
    /// Report each expression instead of building its node: the
    /// traversal alone, for [`DiscoveryResult::scan_expressions`].
    scan: Option<&'s mut dyn FnMut(&Expr)>,
    nodes: Vec<CheckedExpr>,
    index: HashMap<SourceSpan, Vec<CheckedNodeId>>,
    statements: Vec<CheckedStmt>,
    aliases: Vec<(SourceSpan, CheckedNodeId)>,
    statements_by_span: HashMap<SourceSpan, CheckedStmtId>,
    facts: &'a HashMap<SourceSpan, NodeFacts>,
}

impl<'a> ArenaBuilder<'a, '_> {
    fn facts(&self, span: &SourceSpan) -> &'a NodeFacts {
        self.facts.get(span).unwrap_or(&NodeFacts::EMPTY)
    }

    fn expr(&mut self, expression: &Expr) -> CheckedNodeId {
        use ExprKind::{Identifier, Index, Member, TypeApply, TypeValue};
        if let Some(node) = self.spelled_construction(expression) {
            return node;
        }
        let op = self.op(expression);
        if let Some(scan) = self.scan.as_mut() {
            scan(expression);
            return CheckedNodeId(0);
        }
        let span = expression.source_span();
        let facts = self.facts(&span);
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
        self.index.entry(span).or_default().push(id);
        id
    }

    /// The node of the construction the check spelled for `expression`, if
    /// it is a spelled sugar: the node answers at the sugar's location too,
    /// so HIR lowers the construction where the sugar was written (an
    /// initializer list, a t-string, a value pack read as its
    /// `ParameterList`, a scalar `range` call).
    fn spelled_construction(&mut self, expression: &Expr) -> Option<CheckedNodeId> {
        if !matches!(
            expression.kind,
            ExprKind::BraceLit(_)
                | ExprKind::TString { .. }
                | ExprKind::Identifier(_)
                | ExprKind::Member { .. }
                | ExprKind::Call { .. }
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
        if self.scan.is_none() {
            self.index
                .entry(expression.source_span())
                .or_default()
                .push(node);
            self.aliases.push((expression.source_span(), node));
            // A `for` loop iterates the construction it spells.
            if let Some(protocol) = &self.facts(&expression.source_span()).iteration_protocol {
                self.nodes[node.0 as usize]
                    .adjustments
                    .push(SemanticAdjustment::Iterate((**protocol).clone()));
            }
        }
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
            ExprKind::TypeValue(ty) => {
                self.type_applications(ty);
                CheckedOp::TypeValue(ty.clone())
            }
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
                param_args: self.param_args(param_args),
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
                param_args: self.param_args(param_args),
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
                args: self.param_args(args),
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
                explicit_step,
            } => CheckedOp::Slice {
                object: self.expr(object),
                lower: self.optional(lower.as_deref()),
                upper: self.optional(upper.as_deref()),
                step: self.optional(step.as_deref()),
                explicit_step: *explicit_step,
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
                        SubscriptArg::Slice {
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => CheckedSubscriptArg::Slice {
                            lower: self.optional(lower.as_deref()),
                            upper: self.optional(upper.as_deref()),
                            step: self.optional(step.as_deref()),
                            explicit_step: *explicit_step,
                        },
                        SubscriptArg::KeywordSlice {
                            name,
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => CheckedSubscriptArg::KeywordSlice {
                            name: name.clone(),
                            lower: self.optional(lower.as_deref()),
                            upper: self.optional(upper.as_deref()),
                            step: self.optional(step.as_deref()),
                            explicit_step: *explicit_step,
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

    /// A type argument builds the nodes of its lifted applications only.
    fn param_args(&mut self, arguments: &[ParamArg]) -> Vec<CheckedParamArg> {
        arguments
            .iter()
            .map(|argument| self.param_arg(argument))
            .collect()
    }

    fn param_arg(&mut self, argument: &ParamArg) -> CheckedParamArg {
        match argument {
            ParamArg::Type(ty) => {
                self.type_applications(ty);
                CheckedParamArg::Type(ty.clone())
            }
            ParamArg::Value(value) => CheckedParamArg::Value(self.expr(value)),
            ParamArg::Named { name, value } => CheckedParamArg::Named {
                name: name.clone(),
                value: Box::new(self.param_arg(value)),
            },
        }
    }

    /// Build the node of each expression inside `ty` the check named a
    /// lifted function for ([`SemanticAdjustment::ComptimeApplication`]):
    /// MIR lowers it in that function, away from the annotation that
    /// spells it. Any other expression of an annotation has no node.
    fn type_applications(&mut self, ty: &mojito_ast::ast::Type) {
        struct Applied<'a> {
            facts: &'a HashMap<SourceSpan, NodeFacts>,
            found: Vec<Expr>,
        }
        impl mojito_ast::visit::Visitor for Applied<'_> {
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
        let mut applied = Applied {
            facts: self.facts,
            found: Vec::new(),
        };
        mojito_ast::visit::walk_type(&mut applied, ty);
        for expression in &applied.found {
            self.expr(expression);
        }
    }

    fn block(&mut self, statements: &[Stmt]) -> Vec<CheckedStmtId> {
        statements
            .iter()
            .map(|statement| self.stmt(statement))
            .collect()
    }

    /// `statement`'s node, built after its operands' and bodies' nodes; in
    /// scan mode, the traversal alone.
    fn stmt(&mut self, statement: &Stmt) -> CheckedStmtId {
        let op = self.stmt_op(statement);
        if self.scan.is_some() {
            return CheckedStmtId(0);
        }
        let span = statement.source_span();
        let facts = self.facts(&span);
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
        self.statements_by_span.insert(span, id);
        id
    }

    /// `statement`'s own operation, its expressions' and bodies' nodes built
    /// in their structural order.
    fn stmt_op(&mut self, statement: &Stmt) -> CheckedStmtOp {
        use mojito_ast::ast::StmtKind;
        let values = |this: &mut Self, values: &[Expr]| {
            values
                .iter()
                .map(|value| this.expr(value))
                .collect::<Vec<_>>()
        };
        let optional_block =
            |this: &mut Self, body: &Option<Vec<Stmt>>| body.as_ref().map(|body| this.block(body));
        let branches = |this: &mut Self, branches: &[(Expr, Vec<Stmt>)]| {
            branches
                .iter()
                .map(|(condition, body)| (this.expr(condition), this.block(body)))
                .collect::<Vec<_>>()
        };
        match &statement.kind {
            StmtKind::VarDecl { name, ty, value } => {
                if let Some(ty) = ty {
                    self.type_applications(ty);
                }
                CheckedStmtOp::VarDecl {
                    name: name.clone(),
                    ty: ty.clone(),
                    value: self.expr(value),
                }
            }
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
                type_params,
                ty,
                where_clauses,
                value,
            } => CheckedStmtOp::Comptime {
                name: name.clone(),
                type_params: type_params.clone(),
                ty: ty.clone(),
                where_clauses: values(self, where_clauses),
                value: self.expr(value),
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
            StmtKind::Unpack {
                targets,
                value,
                declares,
            } => CheckedStmtOp::Unpack {
                targets: values(self, targets),
                value: self.expr(value),
                declares: *declares,
            },
            StmtKind::Def {
                name,
                decorators,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures,
                raises,
                raises_type,
                ret,
                where_clauses,
                body,
            } => CheckedStmtOp::Def(Box::new(CheckedDef {
                name: name.clone(),
                decorators: decorators.clone(),
                type_params: type_params.clone(),
                params: self.fn_params(params),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                captures: captures.clone(),
                raises: *raises,
                raises_type: raises_type.clone(),
                ret: ret.clone(),
                where_clauses: values(self, where_clauses),
                body: self.block(body),
            })),
            StmtKind::Struct {
                name,
                decorators,
                type_params,
                conforms,
                callable_conformance,
                conformance_conditions,
                where_clauses,
                fields,
                associated,
                methods,
                fieldwise_init,
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
                        params: member.params.clone(),
                        ty: member.ty.clone(),
                        where_clauses: values(self, &member.where_clauses),
                        value: self.expr(&member.value),
                    })
                    .collect();
                let methods = methods
                    .iter()
                    .map(|method| CheckedMethod {
                        name: method.name.clone(),
                        type_params: method.type_params.clone(),
                        has_self: method.has_self,
                        self_convention: method.self_convention,
                        self_origin: method.self_origin.clone(),
                        decorators: method.decorators.clone(),
                        params: self.fn_params(&method.params),
                        positional_only: method.positional_only,
                        keyword_only: method.keyword_only,
                        raises: method.raises,
                        raises_type: method.raises_type.clone(),
                        ret: method.ret.clone(),
                        where_clauses: values(self, &method.where_clauses),
                        body: self.block(&method.body),
                        provenance: method.provenance,
                    })
                    .collect();
                CheckedStmtOp::Struct(Box::new(CheckedStruct {
                    name: name.clone(),
                    decorators: decorators.clone(),
                    type_params: type_params.clone(),
                    conforms: conforms.clone(),
                    callable_conformance: callable_conformance.clone(),
                    conformance_conditions,
                    where_clauses,
                    fields: fields.clone(),
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
                    .map(|method| CheckedTraitMethod {
                        name: method.name.clone(),
                        type_params: method.type_params.clone(),
                        self_convention: method.self_convention,
                        self_origin: method.self_origin.clone(),
                        params: self.fn_params(&method.params),
                        positional_only: method.positional_only,
                        keyword_only: method.keyword_only,
                        raises: method.raises,
                        raises_type: method.raises_type.clone(),
                        ret: method.ret.clone(),
                        where_clauses: values(self, &method.where_clauses),
                        default_body: optional_block(self, &method.default_body),
                    })
                    .collect();
                let comptime_members = comptime_members
                    .iter()
                    .map(|member| CheckedTraitComptime {
                        name: member.name.clone(),
                        params: member.params.clone(),
                        ty: member.ty.clone(),
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
            StmtKind::Scope(body) => CheckedStmtOp::Scope(self.block(body)),
            StmtKind::While { cond, body, orelse } => CheckedStmtOp::While {
                cond: self.expr(cond),
                body: self.block(body),
                orelse: optional_block(self, orelse),
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
                ty: param.ty.clone(),
                default: self.optional(param.default.as_ref()),
                kind: param.kind,
                convention: param.convention,
                origin: param.origin.clone(),
            })
            .collect()
    }
}

/// The adjustments the check's `facts` at `expression` record, in the
/// order lowering applies them.
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
    if facts.implicitly_copied_consuming_receiver {
        adjustments.push(SemanticAdjustment::ImplicitlyCopyConsumingReceiver);
    }
    if facts.truthiness_condition {
        adjustments.push(SemanticAdjustment::Truthiness);
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
fn build_checked_declarations(
    statements: &[Stmt],
    trait_members: &[CheckedTraitMember],
    annotation_types: &HashMap<AnnotationSite, Ty>,
    facts: &HashMap<SourceSpan, NodeFacts>,
) -> Vec<CheckedDeclaration> {
    fn block(
        statements: &[Stmt],
        out: &mut Vec<CheckedDeclaration>,
        annotation_types: &HashMap<AnnotationSite, Ty>,
        facts: &HashMap<SourceSpan, NodeFacts>,
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
                    binding: facts
                        .get(&location)
                        .and_then(|facts| facts.statement_binding),
                    captures: facts
                        .get(&location)
                        .and_then(|facts| facts.captures.clone())
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
                let children = block(def_body, out, annotation_types, facts);
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
                StmtKind::For { var, .. } | StmtKind::ComptimeFor { var, .. } => {
                    (CheckedDeclKind::Binding, var.clone())
                }
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
                        StmtKind::With { body, .. } | StmtKind::Scope(body) => vec![body],
                        _ => Vec::new(),
                    };
                    for body in nested {
                        ids.extend(block(body, out, annotation_types, facts));
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
                CheckedDeclKind::Binding | CheckedDeclKind::CompileTime => facts
                    .get(&location)
                    .and_then(|facts| facts.binding_ty.clone()),
                CheckedDeclKind::Struct | CheckedDeclKind::Trait => None,
            };
            out.push(CheckedDeclaration {
                id,
                kind,
                name,
                binding: facts
                    .get(&location)
                    .and_then(|facts| facts.statement_binding),
                captures: facts
                    .get(&location)
                    .and_then(|facts| facts.captures.clone())
                    .unwrap_or_default(),
                location,
                ty,
                children: Vec::new(),
            });
            let children = match &statement.kind {
                StmtKind::Def { body, .. } => block(body, out, annotation_types, facts),
                StmtKind::Struct { methods, .. } => methods
                    .iter()
                    .flat_map(|method| block(&method.body, out, annotation_types, facts))
                    .collect(),
                StmtKind::For { body, orelse, .. } => block(body, out, annotation_types, facts)
                    .into_iter()
                    .chain(
                        orelse
                            .iter()
                            .flat_map(|body| block(body, out, annotation_types, facts)),
                    )
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
                    .flat_map(|body| block(body, out, annotation_types, facts))
                    .collect(),
                StmtKind::ComptimeFor { body, .. } => block(body, out, annotation_types, facts),
                _ => Vec::new(),
            };
            out[id.0 as usize].children = children;
            ids.push(id);
        }
        ids
    }
    let mut declarations = Vec::new();
    let _ = block(statements, &mut declarations, annotation_types, facts);
    let members: Vec<Stmt> = trait_members
        .iter()
        .map(|member| member.declaration.clone())
        .collect();
    let _ = block(&members, &mut declarations, annotation_types, facts);
    declarations
}

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
        } => Some(*owner),
        _ => None,
    })
}

/// The declaration of a decoded trait member before
/// [`CheckedProgram::installed`] rebuilds it.
fn unrebuilt_declaration() -> Stmt {
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
        facts { $($(#[$doc:meta])* $field:ident: $value:ty => $variant:ident,)* }
        flags { $($(#[$flag_doc:meta])* $flag:ident => $flag_variant:ident,)* }
    ) => {
        /// Every fact the check recorded at one occurrence, as an op holds
        /// its own attributes.
        ///
        /// It encodes as the list of the facts present, so an occurrence
        /// holding one fact costs that fact alone.
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

        /// One fact of a [`NodeFacts`], as it encodes.
        #[derive(serde::Serialize)]
        enum NodeFactRef<'a> {
            $($variant(&'a $value),)*
            $($flag_variant,)*
        }

        /// One fact of a [`NodeFacts`], as it decodes.
        #[derive(serde::Deserialize)]
        enum NodeFact {
            $($variant($value),)*
            $($flag_variant,)*
        }

        impl serde::Serialize for NodeFacts {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let facts = std::iter::empty()
                    $(.chain(self.$field.as_ref().map(NodeFactRef::$variant)))*
                    $(.chain(self.$flag.then_some(NodeFactRef::$flag_variant)))*;
                serializer.collect_seq(facts.collect::<Vec<_>>())
            }
        }

        impl<'de> serde::Deserialize<'de> for NodeFacts {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let mut facts = Self::default();
                for fact in <Vec<NodeFact> as serde::Deserialize>::deserialize(deserializer)? {
                    match fact {
                        $(NodeFact::$variant(value) => facts.$field = Some(value),)*
                        $(NodeFact::$flag_variant => facts.$flag = true,)*
                    }
                }
                Ok(facts)
            }
        }
    };
}

node_facts! {
    facts {
        /// The callable a call or name resolved to.
        overload_target: String => OverloadTarget,
        /// The base type a `$contextual` leading-dot sentinel resolved to.
        contextual_base: String => ContextualBase,
        generic_instantiation: GenericInstantiation => GenericInstantiation,
        method_instantiation: MethodInstantiation => MethodInstantiation,
        /// Caller-substituted loan transfers of a call.
        call_transfers: Vec<CheckedCallTransfer> => CallTransfers,
        implicit_conversion: String => ImplicitConversion,
        /// The converted-to type of an implicit conversion.
        implicit_conversion_type: Ty => ImplicitConversionType,
        conversion_source_borrow: bool => ConversionSourceBorrow,
        conversion_raises: Ty => ConversionRaises,
        ty: Ty => Type,
        binding: mojito_types::origin::OwnerId => Binding,
        /// The binding a declaring statement introduces.
        statement_binding: mojito_types::origin::OwnerId => StatementBinding,
        /// A nested declaration's explicit captures.
        captures: Vec<CheckedCapture> => Captures,
        comprehension_bindings: Vec<CheckedComprehensionBinding> => ComprehensionBindings,
        place_ty: Ty => PlaceType,
        binding_ty: Ty => BindingType,
        effects: EffectFacts => Effects,
        selected_call: Box<CheckedCallContract> => SelectedCall,
        subscript_descriptor: (Vec<Option<mojito_types::types::SliceKind>>, bool) => SubscriptDescriptor,
        iteration_protocol: Box<IterationProtocol> => IterationProtocol,
        simd_construction: (SimdDtype, SimdWidth) => SimdConstruction,
        /// The one operation record of the occurrence.
        adjustment: Box<SemanticAdjustment> => Adjustment,
        parameterized_method_call: Vec<mojito_types::types::ParamDecl> => ParameterizedMethodCall,
        tuple_unpack_plan: Vec<CheckedTupleUnpackElement> => TupleUnpackPlan,
        interior_reference: mojito_types::origin::OriginPlace => InteriorReference,
        interior_invalidations: Vec<InteriorInvalidation> => InteriorInvalidations,
        /// A reference read as a value: whether it is writable.
        reference_value_use: bool => ReferenceValueUse,
        rebind_assertion: RebindAssertion => RebindAssertion,
    }
    flags {
        explicit_destroy_call => ExplicitDestroyCall,
        copy_place_value_use => CopyPlaceValueUse,
        call_place_use => CallPlaceUse,
        borrowed_read_call_place => BorrowedReadCallPlace,
        read_temporary_argument => ReadTemporaryArgument,
        implicitly_copied_consuming_receiver => ImplicitlyCopiedConsumingReceiver,
        truthiness_condition => TruthinessCondition,
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
