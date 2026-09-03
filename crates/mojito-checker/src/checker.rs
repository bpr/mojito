//! Static semantic checker: the authoritative handoff between elaborated AST and
//! compiler lowering. It resolves annotations, calls, traits, and conventions
//! into [`CheckedProgram`](mojito_checked::checked::CheckedProgram). It is a *sound*
//! approximation: if [`check`] succeeds, compiled execution will not raise
//! `UndefinedVariable`, `TypeError`, `NotCallable`, `ArityMismatch`, or
//! `ClosureEscape`. It is deliberately not *complete* — see the forward-reference
//! note below — so a few valid Mojo programs are rejected.
//!
//! ## Scoping
//! A stack of scopes (`Vec<HashMap<String, Ty>>`) models lexical name lookup.
//! Names are bound *sequentially* in source order, and a nested `def` body is checked at its definition
//! site with the enclosing scopes still on the stack (so capture is lexical).
//! One consequence: a function body may not forward-reference a sibling `def`
//! declared later in the same block (mutual recursion). Choosing soundness over completeness here keeps
//! the checker simple; hoisting `def` signatures per block is future work.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use mojito_ast::ast::{
    ArgConvention, Dtype, Expr, ExprKind, FnParam, InfixOp, Method, PrefixOp, Stmt, StmtKind,
    StructComptime, SubscriptArg, TStringPart, TraitComptime, Type as SourceType,
};
use mojito_ast::call::{
    ArgSlot, CallVariadics, effective_keyword_only_index, match_call_slots, regular_marker_index,
};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::ct::{CtExpr, CtValue};
use mojito_types::types::{
    CallableDefault, ConstraintOperand, DependentType, GenericConstraint, ParamDecl, SliceKind, Ty,
    TyArg, array_element, array_parts, array_type, contains_infer, dict_elements, dict_type,
    list_element, list_type, range_type, set_element, set_type, tuple_elements,
    tuple_type as nominal_tuple_type,
};

/// Type-check a whole program. Convenience wrapper over [`Checker`].
pub fn check(stmts: &[Stmt]) -> Result<(), TypeError> {
    check_program(stmts).map(|_| ())
}

/// Type-check and retain the semantic facts consumed by lowering/backends.
pub fn check_program(stmts: &[Stmt]) -> Result<mojito_checked::checked::CheckedProgram, TypeError> {
    check_program_with_materialized_callables(stmts, HashMap::new())
}

/// Check compiler-generated Tuple declarations with the exact callable types
/// referenced by their opaque, parser-unconstructible annotation ids.
pub fn check_program_with_materialized_callables(
    stmts: &[Stmt],
    materialized_callables: HashMap<String, Ty>,
) -> Result<mojito_checked::checked::CheckedProgram, TypeError> {
    let mut expanded = expand_trait_defaults(stmts)?;
    // Source locations survive elaboration clones and therefore cannot identify
    // semantic occurrences. Re-key the final checked tree after the last
    // checker-side cloning transform, before any fact table is populated.
    mojito_ast::ast::rekey_syntax(&mut expanded);
    // Two-phase transfer effects: a call site checked before its callee's
    // body only sees effects already committed, so the check reruns — seeded
    // with the prior round's committed map — whenever some call site
    // observed a stale (since-grown) callee entry. Effects grow
    // monotonically over a finite lattice, so the fixpoint is small; the cap
    // guards checker defects, not user programs.
    const TRANSFER_EFFECT_ROUNDS: usize = 4;
    let mut transfer_seed: HashMap<String, Vec<mojito_checked::checked::TransferEffect>> =
        HashMap::new();
    let mut call_through_seed: HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>> =
        HashMap::new();
    let mut rounds = 0;
    let checker = loop {
        let mut checker = Checker::new_with_materialized_callables(
            materialized_callables.clone(),
            std::mem::take(&mut transfer_seed),
            std::mem::take(&mut call_through_seed),
        );
        checker.check_program(&expanded)?;
        checker.check_reference_result_reads()?;
        fn first_stale<E: PartialEq + Clone>(
            committed: &HashMap<String, Vec<E>>,
            observations: &HashMap<String, Vec<E>>,
        ) -> Option<String> {
            observations
                .iter()
                .find(|(name, seen)| {
                    let now = committed.get(*name).cloned().unwrap_or_default();
                    now.len() != seen.len() || now.iter().any(|effect| !seen.contains(effect))
                })
                .map(|(name, _)| name.clone())
        }
        let stale = first_stale(
            &checker.transfer_effects.borrow(),
            &checker.effect_observations.borrow(),
        )
        .or_else(|| {
            first_stale(
                &checker.call_through_effects.borrow(),
                &checker.call_through_observations.borrow(),
            )
        });
        let Some(callable) = stale else {
            break checker;
        };
        rounds += 1;
        if rounds > TRANSFER_EFFECT_ROUNDS {
            return Err(TypeError::TransferEffectDivergence {
                rounds: TRANSFER_EFFECT_ROUNDS,
                callable,
            });
        }
        transfer_seed = checker.transfer_effects.borrow().clone();
        call_through_seed = checker.call_through_effects.borrow().clone();
    };
    let explicit_destroy_types = checker
        .structs
        .iter()
        .filter_map(|(name, info)| {
            let self_ty = Ty::Struct(name.clone(), info.decls.iter().map(param_as_arg).collect());
            (!checker.is_deinitable(&self_ty)).then(|| {
                (
                    name.clone(),
                    mojito_checked::checked::ExplicitDestroyInfo {
                        message: info.explicit_destroy_message.clone().unwrap_or_else(|| {
                            "value is not implicitly deletable and must be explicitly destroyed"
                                .to_string()
                        }),
                        destructors: info.explicit_destructors.clone(),
                        fields: info
                            .fields
                            .iter()
                            .filter_map(|(field, ty)| match ty {
                                Ty::Struct(field_ty, _) if !checker.is_deinitable(ty) => {
                                    Some((field.clone(), field_ty.clone()))
                                }
                                _ => None,
                            })
                            .collect(),
                    },
                )
            })
        })
        .collect();
    {
        let binding_types = checker.binding_types.borrow();
        let comprehension_bindings = checker.comprehension_bindings.borrow();
        let deletability = checker.explicit_destroy_deletability.borrow();
        crate::explicit_destroy::check(
            &expanded,
            &binding_types,
            &comprehension_bindings,
            &deletability,
            &explicit_destroy_types,
        )?;
    }
    Ok(mojito_checked::checked::CheckedProgram::new(
        expanded,
        checker.overload_targets.into_inner(),
        checker.contextual_bases.into_inner(),
        checker.generic_instantiations.into_inner(),
        checker.call_transfers.into_inner(),
        checker.implicit_conversions.into_inner(),
        checker.conversion_source_borrows.into_inner(),
        checker.declaration_types.into_inner(),
        checker.generic_parameters.into_inner(),
        checker.expression_types.into_inner(),
        checker.expression_bindings.into_inner(),
        checker.statement_bindings.into_inner(),
        checker.declaration_captures.into_inner(),
        checker.comprehension_bindings.into_inner(),
        checker.expression_place_types.into_inner(),
        checker.binding_types.into_inner(),
        checker.expression_effects.into_inner(),
        checker.selected_calls.into_inner(),
        checker.subscript_descriptors.into_inner(),
        checker.iteration_protocols.into_inner(),
        checker.simd_constructions.into_inner(),
        checker.operation_adjustments.into_inner(),
        checker.tuple_unpack_plans.into_inner(),
        checker.interior_references.into_inner(),
        checker.interior_invalidations.into_inner(),
        explicit_destroy_types,
        checker.explicit_destroy_calls.into_inner(),
        checker.reference_value_uses.into_inner(),
        checker.copy_place_value_uses.into_inner(),
        checker.call_place_uses.into_inner(),
        checker.borrowed_read_call_places.into_inner(),
        checker.implicitly_copied_consuming_receivers.into_inner(),
        checker.declaration_effects.into_inner(),
    ))
}

/// A declaration-only view of the checker's conformance registry for phases
/// that necessarily run before whole-program type checking.  Compile-time
/// specialization uses this to validate an inferred heterogeneous type pack at
/// its call site; it must not grow a second, subtly different implementation of
/// trait conformance.
///
/// The oracle records trait refinement, nominal struct conformances,
/// conformance conditions, field types, and lifecycle method presence.  Method
/// bodies and full requirement signatures remain the ordinary checker's job
/// after elaboration.
pub struct ConformanceOracle {
    checker: Checker,
}

/// Evidence retained when a pre-check conformance query fails.
pub struct ConformanceFailure {
    pub reason: Option<String>,
}

mod conformance;
mod overload_support;
mod traits_support;

use overload_support::*;
use traits_support::*;

/// Type-check a program and return the concrete lowered callee chosen for every
/// overloaded call site. MIR lowering uses this side table so source calls like
/// `f(x)` can lower to a signature-specific function even when overloads share
/// the same arity.
pub fn resolve_overload_targets(stmts: &[Stmt]) -> Result<HashMap<SourceSpan, String>, TypeError> {
    Ok(check_program(stmts)?.overload_targets().clone())
}

/// A static type checker over the parsed AST. Top-level struct and trait
/// declarations register order-independently (shells, member types, method
/// signatures) before the source-order walk checks conformance and bodies;
/// everything else checks in a single source-order pass.
pub struct Checker {
    /// Lexical scope chain, innermost last. Starts with the global scope.
    scopes: Vec<HashMap<String, Ty>>,
    /// Binding mutability, parallel to `scopes`. `var` locals are writable;
    /// ordinary function parameters are not.
    mutable_scopes: Vec<HashMap<String, bool>>,
    /// Stable identities for value bindings, parallel to `scopes`. Origin and
    /// loan facts use these identities so a shadowing declaration cannot be
    /// confused with the binding of the same source name in an outer scope.
    owner_scopes: Vec<HashMap<String, mojito_types::origin::OwnerId>>,
    /// Origins retained inside reference-bearing aggregate bindings, parallel
    /// to the lexical value scopes.  Unlike `Ty::Struct`, this preserves the
    /// use-site owner identity needed for escape checking.
    aggregate_origin_scopes: Vec<HashMap<String, Vec<mojito_types::origin::Origin>>>,
    /// Field-specific projection of `aggregate_origin_scopes`. Keeping direct
    /// reference fields separate prevents a write through one stored handle
    /// from invalidating interiors reached through an unrelated field.
    aggregate_field_origin_scopes:
        Vec<HashMap<String, HashMap<String, Vec<mojito_types::origin::Origin>>>>,
    /// Reference-parameter handle types. Parameter expression typing still
    /// reads through to the declared referent, while storage contexts can ask
    /// for the handle explicitly.
    reference_parameter_scopes: Vec<HashMap<String, mojito_types::origin::RefTy>>,
    /// The enclosing struct's origin binder a `ref[Self.o]` parameter's clause
    /// names, by the parameter's owner: `Pointer(to=param)` mints that binder
    /// (upstream's iterator-storage shape `self.src = Pointer(to=xs)` into a
    /// `Pointer[T, Self.o]` field) instead of the parameter slot's place.
    reference_parameter_binders:
        HashMap<mojito_types::origin::OwnerId, mojito_types::origin::PointerOrigin>,
    /// Origin-parameter declarations for callable values, parallel to the
    /// lexical value scopes. The outer vector stored per name has one entry per
    /// overload declaration. Each entry also retains the original compile-time
    /// parameter order so erased Origin arguments can participate in overload
    /// and generic candidate selection.
    callable_origin_scopes: Vec<HashMap<String, Vec<CallableOriginSignature>>>,
    /// Monotonic binding-identity source. A `Cell` because materialized
    /// borrow-source temporaries mint anonymous owners from shared-borrow
    /// inference contexts.
    next_owner: std::cell::Cell<u32>,
    /// How many leading entries of `enclosing_type_params` are the enclosing
    /// STRUCT's own parameters (method-own parameters are appended after
    /// them while a method checks). Member origin clauses reject bare
    /// references into this prefix; later (method-own) binders stay bare.
    enclosing_struct_type_params: std::cell::Cell<usize>,
    /// True while resolving a signature (return) annotation: an origin
    /// argument that is syntactically origin-shaped but names places only the
    /// body can resolve (`origin_of(self.entries)`) is accepted and erased
    /// instead of failing place resolution.
    signature_origin_leniency: std::cell::Cell<bool>,
    /// Index of the local scope for each function currently being checked.
    function_bases: Vec<usize>,
    /// Function/method scope base and caller-owned inputs which may legally
    /// appear inside a returned reference-bearing aggregate.
    aggregate_escape_contexts: Vec<(usize, HashSet<mojito_types::origin::OwnerId>)>,
    /// Explicit capture policy for each nested function body being checked.
    capture_contexts: RefCell<Vec<CapturePolicy>>,
    /// Defined structs, by name (a separate namespace from value bindings).
    structs: HashMap<String, StructInfo>,
    /// Top-level struct symbols in the checked program, collected before body
    /// checking. Concrete Tuple types can therefore select their generated
    /// implementation independent of declaration order.
    declared_structs: HashSet<String>,
    /// Top-level structs whose shell, member types, and method signatures were
    /// registered by `check_program`'s order-independent pre-passes. The
    /// source-order walk removes each entry and runs only the completion phase
    /// (conformance verification and method bodies).
    predeclared_structs: HashSet<String>,
    /// Top-level traits registered by `check_program`'s pre-pass; the walk
    /// removes each entry instead of re-registering.
    predeclared_traits: HashSet<String>,
    /// Fixed semantic arguments for every compiler-generated public Tuple in
    /// the final program. This signature-only predeclaration is populated as a
    /// closed set before sequential member checking, so reciprocal transforms
    /// can name each other's complete nominal type without exposing ordinary
    /// source forward references.
    predeclared_generated_tuple_arguments: HashMap<String, Vec<TyArg>>,
    /// Exact semantic callable contracts named by compiler-only opaque type
    /// ids in generated Tuple declarations. Parsed source cannot populate this
    /// namespace.
    materialized_callables: HashMap<String, Ty>,
    /// Generated Tuple implementations are emitted at the stdlib template
    /// position, which may precede user element declarations. While checking
    /// that compiler-owned specialization only, permit nominal forward type
    /// identities; ordinary source declarations retain sequential visibility.
    allow_generated_tuple_forward_types: bool,
    /// Defined traits, by name (their method requirements).
    traits: HashMap<String, TraitInfo>,
    /// Stack of a generic `def`'s checked type parameters, innermost last. A
    /// bare `T` annotation resolves to the complete `Ty::Param`, including any
    /// anonymous callable-trait contract.
    /// (A `def`'s *value* parameters are ordinary `Int` locals, not here.)
    tparams: Vec<HashMap<String, Ty>>,
    /// The enclosing struct's parameters while checking its fields and methods,
    /// so `Self.T` resolves to `Ty::Param` and `Self.n` to a value parameter.
    /// Saved/restored around a (possibly nested) struct definition.
    self_decls: Vec<ParamDecl>,
    /// Positive conformance facts guaranteed by the active method's `where`
    /// clause. These are deliberately kept separate from `self_decls`: adding
    /// a temporary bound to the declaration changes the identity of `Ty::Param`
    /// and can make an otherwise identical return type fail to match. The
    /// facts refine capability queries only while that method body is checked.
    assumed_conformances: Vec<HashSet<(String, String)>>,
    enclosing_type_params: Vec<mojito_ast::ast::TypeParam>,
    /// The `Ty` denoted by a bare `Self` while checking a struct's members (the
    /// struct type) or a trait's requirements (`Ty::SelfType`). `None` elsewhere.
    self_ty: Option<Ty>,
    /// Trait-associated comptime requirements in scope while checking a trait's
    /// own method requirement signatures, so `Self.Element` can resolve.
    trait_self_comptime: Vec<HashMap<String, CtMemberReq>>,
    /// Exact integer constants declared by `comptime NAME = value`.
    comptimes: HashMap<String, mojito_common::literal::IntLiteral>,
    /// Generic top-level type aliases declared by `comptime NAME[params] = Type`,
    /// expanded per application during type resolution. See [`ComptimeAlias`].
    comptime_aliases: HashMap<String, ComptimeAlias>,
    /// Whether `self` is writable in the method body being checked — set while
    /// checking a `mut self` method's body (so `self.x = e` is allowed there).
    self_mutable: bool,
    /// An `out self` lifecycle initializer is establishing field storage.  For
    /// a reference-valued field, assigning a reference here stores its handle;
    /// later assignments write through the established handle instead.
    self_initializing: bool,
    /// Per-method accumulation of symbolic origin write requirements inherited
    /// from calls through generic receiver fields. The completed frame is
    /// merged into that method's registered signature.
    parametric_write_frames: RefCell<Vec<Vec<mojito_types::origin::OriginParamId>>>,
    /// The declaration currently being checked comes from the bundled
    /// standard-library crossing module (`stdlib/std/memory.mojo`). Only such
    /// declarations may name compiler-private storage types (`__UninitStorage`)
    /// in sourceless type-annotation positions.
    bundled_stdlib_declaration: bool,
    /// Source-span to lowered callee for calls whose source name denotes an
    /// overload set. Interior mutability keeps expression inference usable from
    /// read-only helper methods while still recording resolution facts.
    overload_targets: RefCell<HashMap<SourceSpan, String>>,
    /// Checker-resolved base type name per `$contextual` leading-dot sentinel
    /// (keyed by the sentinel identifier's span); HIR substitutes the name.
    contextual_bases: RefCell<HashMap<SourceSpan, String>>,
    /// The resolved generic application per bound-generic call site (callee +
    /// exact compile-time arguments), retained for instantiation discovery.
    generic_instantiations:
        RefCell<HashMap<SourceSpan, mojito_checked::checked::GenericInstantiation>>,
    /// Per-body accumulation frames for inferred loan-transfer effects.
    transfer_frames: RefCell<Vec<TransferFrame>>,
    /// Inferred per-callable transfer effects, keyed by callable name
    /// (`name` / `Struct.method`); consulted at later call sites.
    transfer_effects: RefCell<HashMap<String, Vec<mojito_checked::checked::TransferEffect>>>,
    /// Caller-substituted transfers per call occurrence, handed to MIR.
    call_transfers: RefCell<HashMap<SourceSpan, Vec<mojito_checked::checked::CheckedCallTransfer>>>,
    /// First-seen callee effects per `apply_transfer_effects` lookup. The
    /// two-phase pass reruns the check when a callee's final committed
    /// effects differ from what its stalest call-site query observed.
    effect_observations: RefCell<HashMap<String, Vec<mojito_checked::checked::TransferEffect>>>,
    /// Inferred higher-order call-through residues per callable, keyed like
    /// `transfer_effects`; each call site resolves them against the concrete
    /// callable it supplies.
    call_through_effects: RefCell<HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>>>,
    /// First-seen call-through observations, mirroring `effect_observations`.
    call_through_observations:
        RefCell<HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>>>,
    /// Origins transferred into a binding by a callee's store (keyed by the
    /// binding's owner) — an interior-mutability overlay over the
    /// aggregate-origin scopes, merged on lookup.
    transferred_origins:
        RefCell<HashMap<mojito_types::origin::OwnerId, Vec<mojito_types::origin::Origin>>>,
    /// Innermost-last observation frames recording whether a raising
    /// operation that escapes the bracketed region was checked inside it.
    /// Owned iteration over linear elements pushes one around its body —
    /// such a call aborts the loop and abandons residual explicit-destroy
    /// obligations — and every callable body pushes a barrier frame so a
    /// nested `def`'s raising calls never mark an enclosing loop. The
    /// `usize` is the `handled_raise_depth` at region entry: a call under a
    /// deeper depth is contained by a `try` inside the region and does not
    /// escape it, while a handler outside the region still lets the error
    /// abort the region itself.
    raise_observation_frames: RefCell<Vec<(usize, bool)>>,
    implicit_conversions: RefCell<HashMap<SourceSpan, String>>,
    /// Sites in `implicit_conversions` whose selected constructor borrows its
    /// single argument through a `ref [origin]` parameter: the conversion
    /// result borrows the source place (temporary-origin inference). The
    /// value is the loan mutability, solved like the explicit construction
    /// path's `BorrowRefArguments`.
    conversion_source_borrows: RefCell<HashMap<SourceSpan, bool>>,
    simd_constructions: RefCell<HashMap<SourceSpan, (Dtype, i64)>>,
    /// Checked operation decisions — `Variant` construction/tag/projection/
    /// update and origin-bearing pointer construction — keyed by the source
    /// expression.  These cross the typed boundary so MIR never reinterprets
    /// syntax.
    operation_adjustments:
        RefCell<HashMap<SourceSpan, mojito_checked::checked::SemanticAdjustment>>,
    /// Whether type resolution is inside a storage annotation (a struct field
    /// or local `var` type) — the positions where explicit origin slots must
    /// be bound. An initialized local may still leave a BARE generic to infer
    /// wholly from its initializer (upstream-attested via `StringSlice`);
    /// fields and uninitialized locals may not.
    strict_storage_annotation: std::cell::Cell<StorageStrictness>,
    /// Synthetic tuple element reads introduced by unpacking have no source
    /// expression nodes. Retain their checked types and exact generated
    /// accessors on the RHS expression for HIR/MIR lowering.
    tuple_unpack_plans:
        RefCell<HashMap<SourceSpan, Vec<mojito_checked::checked::CheckedTupleUnpackElement>>>,
    /// Place expressions that define a fresh interior-reference generation.
    /// Kept separate from operation adjustments because a Variant projection,
    /// for example, carries both facts at the same checked node.
    interior_references: RefCell<HashMap<SourceSpan, mojito_types::origin::OriginPlace>>,
    /// Mutations which invalidate interior generations below checked bases.
    interior_invalidations:
        RefCell<HashMap<SourceSpan, Vec<mojito_checked::checked::InteriorInvalidation>>>,
    declaration_types: RefCell<HashMap<mojito_checked::checked::AnnotationSite, Ty>>,
    generic_parameters:
        RefCell<HashMap<mojito_checked::checked::GenericSite, Vec<mojito_types::types::ParamDecl>>>,
    /// Checked raising contract and reference-return fact per callable
    /// declaration; lowering never re-reads source `raises`/return syntax.
    declaration_effects: RefCell<
        HashMap<
            mojito_checked::checked::AnnotationSite,
            mojito_checked::checked::DeclarationEffect,
        >,
    >,
    expression_types: RefCell<HashMap<SourceSpan, Ty>>,
    expression_bindings: RefCell<HashMap<SourceSpan, mojito_types::origin::OwnerId>>,
    /// Stable identities assigned by declarations and other binding statements.
    /// HIR uses these facts to map checked owners to runtime slots without
    /// recovering a binding from its source spelling.
    statement_bindings: RefCell<HashMap<SourceSpan, mojito_types::origin::OwnerId>>,
    /// Explicit capture entries resolved at the nested declaration site. Keeping
    /// unused entries is essential: a move capture still transfers at declaration.
    declaration_captures:
        RefCell<HashMap<SourceSpan, Vec<mojito_checked::checked::CheckedCapture>>>,
    /// Stable identities/types for the lexical binders introduced by each
    /// comprehension, retained for checked HIR and explicit-destroy analysis.
    comprehension_bindings:
        RefCell<HashMap<SourceSpan, Vec<mojito_checked::checked::CheckedComprehensionBinding>>>,
    expression_place_types: RefCell<HashMap<SourceSpan, Ty>>,
    binding_types: RefCell<HashMap<SourceSpan, Ty>>,
    /// Positive site-sensitive drop facts retained for the later explicit-
    /// destroy CFG pass. Conditional conformances are meaningful only in the
    /// constraint environment in which a binding was checked.
    explicit_destroy_deletability: RefCell<crate::explicit_destroy::CheckedDeletability>,
    /// Selected call effects keyed by the checked call expression. This records
    /// the contract chosen during overload/bounded dispatch so later phases do
    /// not have to rediscover it from source syntax.
    expression_effects: RefCell<HashMap<SourceSpan, mojito_checked::checked::EffectFacts>>,
    /// Complete overload/origin/effect contract for a selected method-like
    /// call.  Nominal subscripts and ordinary method syntax share this fact.
    selected_calls: RefCell<HashMap<SourceSpan, mojito_checked::checked::CheckedCallContract>>,
    /// Subscript descriptor construction is orthogonal to call selection and
    /// may coexist with a reference-result adjustment at the same expression.
    subscript_descriptors: RefCell<HashMap<SourceSpan, SubscriptDescriptorPlan>>,
    /// Exact iterator protocol selected for each loop/comprehension iterable.
    /// Lowering consumes this fact instead of re-selecting `__iter__` by name.
    iteration_protocols: RefCell<HashMap<SourceSpan, mojito_checked::checked::IterationProtocol>>,
    explicit_destroy_calls: RefCell<std::collections::HashSet<SourceSpan>>,
    /// Expressions whose reference handle, rather than referent value, is
    /// required by a reference binding or origin-bearing aggregate operation.
    /// The bool records whether the resulting capability is writable.
    reference_value_uses: RefCell<HashMap<SourceSpan, bool>>,
    /// Reference-result reads proven copyable in their site-sensitive generic
    /// constraint environment. Final validation runs after those method scopes
    /// have been popped, so it must retain rather than recompute this fact.
    copyable_reference_result_reads: RefCell<HashSet<SourceSpan>>,
    /// Reference-result expressions used as method-call receivers. The call
    /// borrows the referent for its `self` convention (consuming receivers
    /// are gated separately), so the result is never read out as a value.
    borrowed_reference_receivers: RefCell<HashSet<SourceSpan>>,
    /// Place expressions selected for an independent value copy at a consuming
    /// boundary. This stays checker-owned because conditional Copyable
    /// conformance can depend on the active generic constraint environment.
    copy_place_value_uses: RefCell<HashSet<SourceSpan>>,
    /// Actual arguments whose caller place must remain live through a selected
    /// `mut`/`ref` call. This checker-owned fact keeps MIR lowering from
    /// retaining ordinary copied arguments merely because they are syntactic
    /// places.
    call_place_uses: RefCell<HashSet<SourceSpan>>,
    /// Read-convention place arguments the selected call may bind by borrow
    /// instead of the implicit `__copyinit__` read: shared reads whose place no
    /// exclusive access overlaps within the call. Checker-owned because the
    /// effective conventions and within-call exclusivity are resolved here.
    borrowed_read_call_places: RefCell<HashSet<SourceSpan>>,
    /// Consuming method calls whose place receiver is implicitly copied. Kept
    /// separate from the single operation-adjustment slot so parameterized
    /// method metadata can coexist at the same expression.
    implicitly_copied_consuming_receivers: RefCell<HashSet<SourceSpan>>,
    return_ref_contracts: Vec<Option<ReturnRefContract>>,
    named_result_context: Vec<bool>,
    raising_context: Vec<Option<Ty>>,
    handled_raise_depth: usize,
    handled_raise_types: RefCell<Vec<Vec<Ty>>>,
    uninitialized: RefCell<HashSet<mojito_types::origin::OwnerId>>,
}

impl Checker {
    pub fn new() -> Self {
        Self::new_with_materialized_callables(HashMap::new(), HashMap::new(), HashMap::new())
    }

    fn new_with_materialized_callables(
        materialized_callables: HashMap<String, Ty>,
        transfer_seed: HashMap<String, Vec<mojito_checked::checked::TransferEffect>>,
        call_through_seed: HashMap<String, Vec<mojito_checked::checked::CallThroughEffect>>,
    ) -> Self {
        // The bundled seeds are never inferred from bodies, so a prior
        // round's committed map overlays them rather than replacing them.
        let mut transfer_effects = seeded_transfer_effects();
        for (callable, effects) in transfer_seed {
            let entry = transfer_effects.entry(callable).or_default();
            for effect in effects {
                if !entry.contains(&effect) {
                    entry.push(effect);
                }
            }
        }
        Self {
            scopes: vec![HashMap::new()],
            mutable_scopes: vec![HashMap::new()],
            owner_scopes: vec![HashMap::new()],
            aggregate_origin_scopes: vec![HashMap::new()],
            aggregate_field_origin_scopes: vec![HashMap::new()],
            reference_parameter_scopes: vec![HashMap::new()],
            reference_parameter_binders: HashMap::new(),
            callable_origin_scopes: vec![HashMap::new()],
            next_owner: std::cell::Cell::new(0),
            signature_origin_leniency: std::cell::Cell::new(false),
            enclosing_struct_type_params: std::cell::Cell::new(0),
            function_bases: Vec::new(),
            aggregate_escape_contexts: Vec::new(),
            capture_contexts: RefCell::new(Vec::new()),
            structs: HashMap::new(),
            declared_structs: HashSet::new(),
            predeclared_structs: HashSet::new(),
            predeclared_traits: HashSet::new(),
            predeclared_generated_tuple_arguments: HashMap::new(),
            materialized_callables,
            allow_generated_tuple_forward_types: false,
            traits: HashMap::new(),
            tparams: Vec::new(),
            self_decls: Vec::new(),
            assumed_conformances: Vec::new(),
            enclosing_type_params: Vec::new(),
            self_ty: None,
            trait_self_comptime: Vec::new(),
            comptimes: HashMap::new(),
            comptime_aliases: HashMap::new(),
            self_mutable: false,
            self_initializing: false,
            parametric_write_frames: RefCell::new(Vec::new()),
            bundled_stdlib_declaration: false,
            overload_targets: RefCell::new(HashMap::new()),
            contextual_bases: RefCell::new(HashMap::new()),
            generic_instantiations: RefCell::new(HashMap::new()),
            transfer_frames: RefCell::new(Vec::new()),
            transfer_effects: RefCell::new(transfer_effects),
            call_transfers: RefCell::new(HashMap::new()),
            effect_observations: RefCell::new(HashMap::new()),
            call_through_effects: RefCell::new(call_through_seed),
            call_through_observations: RefCell::new(HashMap::new()),
            transferred_origins: RefCell::new(HashMap::new()),
            raise_observation_frames: RefCell::new(Vec::new()),
            implicit_conversions: RefCell::new(HashMap::new()),
            conversion_source_borrows: RefCell::new(HashMap::new()),
            simd_constructions: RefCell::new(HashMap::new()),
            operation_adjustments: RefCell::new(HashMap::new()),
            strict_storage_annotation: std::cell::Cell::new(StorageStrictness::Off),
            tuple_unpack_plans: RefCell::new(HashMap::new()),
            interior_references: RefCell::new(HashMap::new()),
            interior_invalidations: RefCell::new(HashMap::new()),
            declaration_types: RefCell::new(HashMap::new()),
            generic_parameters: RefCell::new(HashMap::new()),
            declaration_effects: RefCell::new(HashMap::new()),
            expression_types: RefCell::new(HashMap::new()),
            expression_bindings: RefCell::new(HashMap::new()),
            statement_bindings: RefCell::new(HashMap::new()),
            declaration_captures: RefCell::new(HashMap::new()),
            comprehension_bindings: RefCell::new(HashMap::new()),
            expression_place_types: RefCell::new(HashMap::new()),
            binding_types: RefCell::new(HashMap::new()),
            explicit_destroy_deletability: RefCell::new(
                crate::explicit_destroy::CheckedDeletability::default(),
            ),
            expression_effects: RefCell::new(HashMap::new()),
            selected_calls: RefCell::new(HashMap::new()),
            subscript_descriptors: RefCell::new(HashMap::new()),
            iteration_protocols: RefCell::new(HashMap::new()),
            explicit_destroy_calls: RefCell::new(std::collections::HashSet::new()),
            reference_value_uses: RefCell::new(HashMap::new()),
            copyable_reference_result_reads: RefCell::new(HashSet::new()),
            borrowed_reference_receivers: RefCell::new(HashSet::new()),
            copy_place_value_uses: RefCell::new(HashSet::new()),
            call_place_uses: RefCell::new(HashSet::new()),
            borrowed_read_call_places: RefCell::new(HashSet::new()),
            implicitly_copied_consuming_receivers: RefCell::new(HashSet::new()),
            return_ref_contracts: Vec::new(),
            named_result_context: Vec::new(),
            raising_context: Vec::new(),
            handled_raise_depth: 0,
            handled_raise_types: RefCell::new(Vec::new()),
            uninitialized: RefCell::new(HashSet::new()),
        }
    }

    fn raising_allowed(&self) -> bool {
        self.handled_raise_depth > 0
            || self
                .raising_context
                .last()
                .is_some_and(|error| error.as_ref().is_some_and(|ty| *ty != Ty::Never))
    }

    fn require_error(&self, operation: impl Into<String>, error: Ty) -> Result<(), TypeError> {
        // A raising operation escapes the innermost observed region (a
        // linear owned-iteration body) unless a `try` INSIDE that region
        // contains it — a handler outside the region still aborts the region.
        if let Some((baseline, flag)) = self.raise_observation_frames.borrow_mut().last_mut()
            && self.handled_raise_depth <= *baseline
        {
            *flag = true;
        }
        if self.handled_raise_depth > 0 {
            if let Some(types) = self.handled_raise_types.borrow_mut().last_mut() {
                types.push(error);
            }
            return Ok(());
        }
        if let Some(Some(expected)) = self.raising_context.last() {
            if *expected == error {
                return Ok(());
            }
            return Err(TypeError::RaiseTypeMismatch {
                expected: expected.to_string(),
                found: error.to_string(),
            });
        }
        if self.raising_allowed() {
            Ok(())
        } else {
            Err(TypeError::UnhandledRaise(operation.into()))
        }
    }

    fn record_call_effect(&self, span: SourceSpan, error: Ty) {
        self.expression_effects.borrow_mut().insert(
            span,
            mojito_checked::checked::EffectFacts {
                raises: Some(error),
                may_suspend: false,
                diverges: false,
            },
        );
    }

    fn concrete_callable_captures(&self, ty: &Ty) -> Vec<mojito_types::origin::CaptureOrigin> {
        use mojito_types::origin::{CallableEnvironment, CaptureOriginSet};
        let callable = match ty {
            Ty::Struct(name, _) => self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref()),
            other => Some(other),
        };
        match callable {
            Some(Ty::Func {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            })
            | Some(Ty::GenericFunc {
                environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(captures)),
                ..
            }) => captures.clone(),
            _ => Vec::new(),
        }
    }

    /// Retain every concrete environment access that may happen while a call is
    /// active. Callable arguments are included because Mojito supports only
    /// non-escaping downward funargs: their environments may be invoked by the
    /// callee before it returns.
    fn call_capture_effects<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<mojito_types::origin::CaptureOrigin> {
        use mojito_types::origin::CaptureOriginSet;
        let captures = types
            .into_iter()
            .flat_map(|ty| self.concrete_callable_captures(ty))
            .collect::<Vec<_>>();
        let CaptureOriginSet::Concrete(captures) = CaptureOriginSet::concrete(captures) else {
            unreachable!("concrete capture canonicalization stays concrete")
        };
        captures
    }

    fn record_call_capture_effects<'a>(
        &self,
        span: SourceSpan,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Vec<mojito_types::origin::CaptureOrigin> {
        let captures = self.call_capture_effects(types);
        if captures.is_empty() {
            return captures;
        }
        self.operation_adjustments.borrow_mut().insert(
            span,
            mojito_checked::checked::SemanticAdjustment::CallableCaptureAccesses(captures.clone()),
        );
        captures
    }

    fn record_call_environment_effects(
        &self,
        span: SourceSpan,
        callable: &Ty,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<(), TypeError> {
        // Call checking may have supplied essential context to an argument
        // before capture-effect collection runs (notably an empty collection
        // display, an exact literal, or an overloaded callable value). Reuse
        // that checked type instead of independently re-inferring the syntax
        // without its selected parameter contract.
        let checked_argument_type = |expression: &Expr| {
            let checked = {
                self.expression_types
                    .borrow()
                    .get(&expression.source_span())
                    .cloned()
            };
            match checked {
                Some(ty) => Ok(ty),
                None => self.infer(expression),
            }
        };
        let mut types = Vec::with_capacity(1 + param_args.len() + args.len() + kwargs.len());
        types.push(callable.clone());
        for argument in param_args {
            let expression = match argument {
                mojito_ast::ast::ParamArg::Value(expression) => Some(expression),
                mojito_ast::ast::ParamArg::Named { value, .. } => match &**value {
                    mojito_ast::ast::ParamArg::Value(expression) => Some(expression),
                    mojito_ast::ast::ParamArg::Type(_)
                    | mojito_ast::ast::ParamArg::Named { .. } => None,
                },
                mojito_ast::ast::ParamArg::Type(_) => None,
            };
            if let Some(expression) = expression {
                // A bare type argument (`hash[MyHasher](x)`) is a value
                // expression syntactically; binding already resolved it as a
                // type and erased it, so it carries no runtime effect.
                let erased = self
                    .operation_adjustments
                    .borrow()
                    .get(&expression.source_span())
                    .is_some_and(|adjustment| {
                        matches!(
                            adjustment,
                            mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument
                                | mojito_checked::checked::SemanticAdjustment::ReifyTypeArgument { .. }
                        )
                    });
                if erased {
                    continue;
                }
                types.push(checked_argument_type(expression)?);
            }
        }
        for argument in args {
            types.push(checked_argument_type(argument)?);
        }
        for argument in kwargs {
            types.push(checked_argument_type(&argument.value)?);
        }
        self.record_call_capture_effects(span, &types);
        Ok(())
    }

    fn declared_error(
        &self,
        raises: bool,
        raises_type: Option<&SourceType>,
    ) -> Result<Option<Ty>, TypeError> {
        if !raises {
            return Ok(None);
        }
        Ok(Some(match raises_type {
            Some(error) => self.ty_from_anno(error)?,
            None => Ty::Error,
        }))
    }

    fn lower_callable_environment(
        &self,
        type_params: &[mojito_ast::ast::TypeParam],
        thin: bool,
        capturing: Option<&mojito_ast::ast::OriginSpec>,
    ) -> Result<mojito_types::origin::CallableEnvironment, TypeError> {
        use mojito_types::origin::{CallableEnvironment, CaptureOriginSet, CaptureSetParamId};
        if thin && capturing.is_some() {
            return Err(TypeError::Unsupported(
                "a callable type cannot be both 'thin' and 'capturing'".to_string(),
            ));
        }
        if thin {
            return Ok(CallableEnvironment::Thin);
        }
        let Some(origins) = capturing else {
            return Ok(CallableEnvironment::Default);
        };
        match origins.as_slice() {
            [] => Ok(CallableEnvironment::Capturing(CaptureOriginSet::empty())),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] if name == "_" => Ok(CallableEnvironment::Capturing(CaptureOriginSet::Infer)),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] => {
                let index = type_params
                    .iter()
                    .position(|parameter| {
                        parameter.name == *name && parameter.bounds.as_slice() == ["OriginSet"]
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?;
                Ok(CallableEnvironment::Capturing(CaptureOriginSet::Param(
                    CaptureSetParamId(index as u32),
                )))
            }
            _ => Err(TypeError::Unsupported(
                "capturing[...] requires exactly one OriginSet parameter or '_'".to_string(),
            )),
        }
    }

    /// Lower a callable annotation's origin contract. Declaration contracts
    /// normally refer to their own parameters and use `lower_ref_sig`; a
    /// contextual function-value type may additionally bind `origin_of(local)`
    /// directly to the checked local owner.
    fn lower_callable_ref_sig(
        &self,
        spec: &mojito_ast::ast::OriginSpec,
        type_params: &[mojito_ast::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<mojito_types::origin::RefSig, TypeError> {
        use mojito_types::origin::{Mutability, RefSig, SigMutability, SigOrigin};

        if let Ok(signature) = lower_ref_sig(spec, type_params, params, 0) {
            return Ok(signature);
        }
        let mut members = Vec::new();
        let mut bound_mutability = Vec::new();
        for expression in spec {
            match &expression.kind {
                ExprKind::Call {
                    name,
                    args,
                    kwargs,
                    param_args,
                } if name == "origin_of" && kwargs.is_empty() && param_args.is_empty() => {
                    if args.is_empty() {
                        return Err(TypeError::Unsupported(
                            "origin_of requires at least one place".to_string(),
                        ));
                    }
                    for argument in args {
                        let (root, path) = place_path(argument).ok_or_else(|| {
                            TypeError::Unsupported("origin_of requires places".to_string())
                        })?;
                        if root == "self" {
                            members.push(project_sig_origin(SigOrigin::Self_, &path));
                        } else if let Some(index) =
                            params.iter().position(|parameter| parameter.name == root)
                        {
                            members.push(project_sig_origin(SigOrigin::Param(index), &path));
                        } else {
                            let actual = self.reference_actual(argument)?;
                            bound_mutability.push(actual.mutability);
                            members.push(SigOrigin::Bound(actual.origin));
                        }
                    }
                }
                _ => members.push(lower_sig_origin_expression(
                    expression,
                    type_params,
                    params,
                    0,
                )?),
            }
        }
        let origin = match members.as_slice() {
            [] => SigOrigin::Infer,
            [single] => single.clone(),
            _ => SigOrigin::union(members),
        };
        let mutability = if bound_mutability.is_empty() {
            SigMutability::Infer
        } else if bound_mutability
            .iter()
            .all(|mutability| *mutability == Mutability::Immutable)
        {
            SigMutability::Immutable
        } else {
            SigMutability::Mutable
        };
        Ok(RefSig { origin, mutability })
    }

    fn lower_callable_ref_param_sigs(
        &self,
        type_params: &[mojito_ast::ast::TypeParam],
        params: &[&FnParam],
    ) -> Result<Vec<Option<mojito_types::origin::RefSig>>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if parameter.convention != Some(ArgConvention::Ref) {
                    return Ok(None);
                }
                match &parameter.origin {
                    Some(spec) => self
                        .lower_callable_ref_sig(spec, type_params, params)
                        .map(Some),
                    None => Ok(Some(mojito_types::origin::RefSig {
                        origin: mojito_types::origin::SigOrigin::Infer,
                        mutability: mojito_types::origin::SigMutability::Infer,
                    })),
                }
            })
            .collect()
    }

    /// Contextually instantiate a generic function value when a monomorphic
    /// callable type supplies all of its type information. Runtime execution is
    /// still type-erased; this produces the checked callable view used by the
    /// binding or argument site.
    fn value_coerces(&self, from: &Ty, to: &Ty) -> bool {
        if coerces(from, to) {
            return true;
        }
        if let Ty::Param { bounds, .. } = to
            && bounds.iter().all(|bound| self.conforms_to(from, bound))
        {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. }))
            && callable_bound_accepts(from, to)
        {
            return true;
        }
        if matches!((from, to), (Ty::GenericFunc { .. }, Ty::GenericFunc { .. })) {
            return callable_bound_accepts(from, to);
        }
        if let Ty::Struct(name, _) = from
            && let Some(callable) = self
                .structs
                .get(name)
                .and_then(|info| info.callable_conformance.as_ref())
        {
            return coerces(callable, to);
        }
        let (
            Ty::GenericFunc {
                environment,
                decls,
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only,
                keyword_only,
                raises,
                error,
                conventions,
                ref_params,
                ref_return,
                ..
            },
            Ty::Func {
                params: expected_params,
                ret: expected_ret,
                ..
            },
        ) = (from, to)
        else {
            return false;
        };
        let mut patterns = params.clone();
        patterns.push((**ret).clone());
        let mut actuals = expected_params.clone();
        actuals.push((**expected_ret).clone());
        let Ok((subst, _)) =
            self.resolve_use_params("<generic callable>", decls, &[], &patterns, &actuals)
        else {
            return false;
        };
        let instantiated = Ty::Func {
            environment: environment.clone(),
            params: params.iter().map(|ty| substitute(ty, &subst)).collect(),
            names: (0..params.len())
                .map(|index| format!("arg{index}"))
                .collect(),
            ret: Box::new(substitute(ret, &subst)),
            required: required.clone(),
            variadic: variadic.as_ref().map(|ty| Box::new(substitute(ty, &subst))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| Box::new(substitute(ty, &subst))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute(error, &subst))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: Default::default(),
        };
        coerces(&instantiated, to)
    }

    /// Checker-selected nominal target when the concrete callable type is
    /// known, otherwise the signature-qualified abstract dispatch target for a
    /// `def(...)` value. Runtime retargeting uses the latter only when the value
    /// is a callable struct.
    fn indirect_callable_target(&self, callable: &Ty) -> Option<String> {
        if let Ty::Struct(name, _) = callable {
            return self
                .structs
                .get(name)
                .and_then(|info| info.callable_target.clone());
        }
        callable_contract_target(callable)
    }

    /// Resolve the callable contract an actual type is *declared* to provide.
    /// Structs participate only through nominal `struct S(def(...))`
    /// conformance; merely defining a shape-compatible `__call__` is not enough.
    fn declared_callable_contract(&self, actual: &Ty) -> Option<Ty> {
        match actual {
            Ty::Func { .. } => Some(actual.clone()),
            Ty::Param {
                callable_bound: Some(bound),
                ..
            } => Some((**bound).clone()),
            Ty::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let contract = info.callable_conformance.as_ref()?;
                Some(substitute(contract, &struct_subst(&info.decls, arguments)))
            }
            // A generic function has a callable identity only for a matching
            // generic anonymous contract. `callable_bound_accepts` rejects it
            // when the required contract is monomorphic.
            Ty::GenericFunc { .. } => Some(actual.clone()),
            // An overload set has no unique callable identity.
            Ty::Overload(_) => None,
            _ => None,
        }
    }

    fn validate_callable_parameter_bounds(
        &self,
        generic_name: &str,
        decls: &[ParamDecl],
        arguments: &[TyArg],
    ) -> Result<(), TypeError> {
        let subst = struct_subst(decls, arguments);
        for (decl, argument) in decls.iter().zip(arguments) {
            let ParamDecl::Type {
                name,
                callable_bound: Some(bound),
                ..
            } = decl
            else {
                continue;
            };
            let TyArg::Ty(actual) = argument else {
                return Err(TypeError::Unsupported(format!(
                    "variadic callable type parameter '{}' in '{generic_name}'",
                    name.trim_start_matches('*')
                )));
            };
            let contract = substitute(bound, &subst);
            let Some(actual_contract) = self.declared_callable_contract(actual) else {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(
                        "requires one monomorphic callable type or nominal callable conformance"
                            .to_string(),
                    ),
                });
            };
            if !callable_bound_accepts(&actual_contract, &contract) {
                return Err(TypeError::TraitNotSatisfied {
                    param: name.clone(),
                    ty: actual.to_string(),
                    trait_name: contract.to_string(),
                    reason: Some(format!(
                        "declared callable contract '{}' is incompatible",
                        actual_contract
                    )),
                });
            }
        }
        Ok(())
    }

    /// The selected `@implicit` constructor for a conversion, when one
    /// applies. The second component is `Some(loan mutability)` when that
    /// constructor borrows its argument through a `ref [origin]` parameter (a
    /// view construction): the caller records the source borrow so the
    /// conversion result's origin refines to the source expression's place
    /// (temporary-origin inference).
    fn implicit_conversion_target(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<(String, Option<bool>)>, TypeError> {
        Ok(self
            .implicit_conversion_constructor(from, to)?
            .map(|(target, source_borrow, _)| (target, source_borrow)))
    }

    /// Whether storing a `found` value where `expected` is declared goes
    /// through an implicit converting constructor that borrows its source
    /// (a view construction such as `var s: Span[Int, _] = xs`). The value
    /// consumed at that storage boundary is the conversion temporary, so the
    /// source place is not implicitly copied.
    pub(super) fn storage_conversion_borrows_source(
        &self,
        found: &Ty,
        expected: Option<&Ty>,
    ) -> bool {
        let Some(expected) = expected else {
            return false;
        };
        if self.value_coerces(found, expected) {
            return false;
        }
        matches!(
            self.implicit_conversion_constructor(found, expected),
            Ok(Some((_, _, false)))
        )
    }

    /// The selected implicit converting constructor from `from` to `to`, its
    /// `ref`-parameter source-borrow mutability, and whether it consumes its
    /// source (a `var`/`deinit` parameter).
    fn implicit_conversion_constructor(
        &self,
        from: &Ty,
        to: &Ty,
    ) -> Result<Option<(String, Option<bool>, bool)>, TypeError> {
        let Ty::Struct(name, args) = to else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        if info.decls.len() != args.len() {
            return Ok(None);
        }
        let subst = struct_subst(&info.decls, args);
        let Some(constructors) = info.methods.get("__init__") else {
            return Ok(None);
        };
        let matches = constructors
            .iter()
            .filter(|sig| {
                sig.implicit
                    && sig.params.len() == 1
                    && coerces(from, &substitute(&sig.params[0], &subst))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [sig] => {
                let target = if constructors.len() == 1 {
                    name.clone()
                } else {
                    method_lowered_name(name, "__init__", sig, self.self_instance_ty(name).as_ref())
                };
                let source_borrow = sig
                    .ref_params
                    .first()
                    .and_then(|signature| signature.as_ref())
                    .map(|signature| {
                        signature.mutability == mojito_types::origin::SigMutability::Mutable
                    });
                let consumes_source = matches!(
                    sig.conventions.first().copied().flatten(),
                    Some(
                        mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit
                    )
                );
                Ok(Some((target, source_borrow, consumes_source)))
            }
            _ => Err(TypeError::BadCall {
                func: name.clone(),
                reason: format!("ambiguous implicit conversion from '{from}' to '{to}'"),
            }),
        }
    }

    /// Record only a constructor-based implicit conversion (the `@implicit`
    /// single-parameter constructor), never a plain value coercion. The
    /// construction/storage checks use this so a failure of the stricter
    /// storage rule cannot be masked by a permissive value coercion (for
    /// example a capturing closure erasing into a plain `def` field).
    fn record_constructor_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        let Some((target, source_borrow)) = self.implicit_conversion_target(from, to)? else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, source_borrow);
        Ok(true)
    }

    /// Install one selected converting constructor plus, for a `ref`-parameter
    /// (view) constructor, the source-borrow fact that refines the conversion
    /// temporary's origin to the source expression's place.
    fn record_selected_conversion(
        &self,
        expression: &Expr,
        target: String,
        source_borrow: Option<bool>,
    ) {
        let span = expression.source_span();
        if let Some(mutable) = source_borrow {
            self.conversion_source_borrows
                .borrow_mut()
                .insert(span.clone(), mutable);
        }
        self.implicit_conversions.borrow_mut().insert(span, target);
    }

    fn record_implicit_conversion(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<bool, TypeError> {
        if let Ty::Overload(candidates) = from {
            let matches: Vec<_> = candidates
                .iter()
                .filter(|candidate| self.value_coerces(candidate, to))
                .filter_map(|candidate| {
                    let ExprKind::Identifier(name) = &expression.kind else {
                        return None;
                    };
                    callable_lowered_name(name, candidate).map(|target| (candidate, target))
                })
                .collect();
            return match matches.as_slice() {
                [(_, target)] => {
                    self.overload_targets
                        .borrow_mut()
                        .insert(expression.source_span(), target.clone());
                    Ok(true)
                }
                [] => Ok(false),
                _ => Err(TypeError::BadCall {
                    func: "overloaded callable value".to_string(),
                    reason: format!("multiple overloads fit expected type '{to}'"),
                }),
            };
        }
        if let Ty::Simd { dtype, width: 1 } = to
            && splats_to(from, *dtype)
        {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        if self.value_coerces(from, to) {
            self.record_literal_materializations(expression, from, to)?;
            return Ok(true);
        }
        let Some((target, source_borrow)) = self.implicit_conversion_target(from, to)? else {
            return Ok(false);
        };
        self.record_selected_conversion(expression, target, source_borrow);
        Ok(true)
    }

    /// Retain every exact-literal boundary selected by contextual typing.  The
    /// recursion matters for aggregates: `(1, 2.0)` materialized as
    /// `Tuple[Int, Float64]` has two scalar boundaries, not one tuple cast.
    fn record_literal_materializations(
        &self,
        expression: &Expr,
        from: &Ty,
        to: &Ty,
    ) -> Result<(), TypeError> {
        let scalar_boundary = matches!(from, Ty::IntLiteral | Ty::FloatLiteral)
            && matches!(
                to,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            );
        if scalar_boundary {
            if let Some(value) = self.exact_literal_value(expression)
                && !self.literal_value_fits_target(&value, to)
            {
                return Err(TypeError::TypeMismatch {
                    expected: to.to_string(),
                    found: from.to_string(),
                    context: format!("numeric literal materialization of '{value}'"),
                });
            }
            self.operation_adjustments.borrow_mut().insert(
                expression.source_span(),
                mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(to.clone()),
            );
            return Ok(());
        }

        // A public Tuple literal is discovered under the canonical nominal
        // name and checked again under its generated specialization name. The
        // names are implementation identities; literal materialization follows
        // the retained element types across that handoff.
        if let (Some(actual), Some(expected)) = (tuple_elements(from), tuple_elements(to))
            && actual.len() == expected.len()
        {
            let values = match &expression.kind {
                ExprKind::TupleLit(values) => Some(values.as_slice()),
                ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                _ => None,
            };
            if let Some(values) = values {
                for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                    self.record_literal_materializations(value, actual, expected)?;
                }
            }
            return Ok(());
        }

        match (from, to) {
            (Ty::Tuple(actual), Ty::Tuple(expected)) if actual.len() == expected.len() => {
                let values = match &expression.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for ((value, actual), expected) in values.iter().zip(actual).zip(expected) {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            (Ty::ComptimeList(actual), Ty::ComptimeList(expected)) => {
                let values = match &expression.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.record_literal_materializations(value, actual, expected)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn exact_literal_value(&self, expression: &Expr) -> Option<CtValue> {
        match self.eval_associated_ct(expression, &HashMap::new()).ok()? {
            value @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)) => Some(value),
            _ => None,
        }
    }

    fn literal_value_fits_target(&self, value: &CtValue, target: &Ty) -> bool {
        match (value, target) {
            (CtValue::IntLiteral(_), Ty::Simd { dtype, width: 1 }) => {
                int_literal_materializes_to_dtype(*dtype)
            }
            (CtValue::FloatLiteral(_), Ty::Simd { dtype, width: 1 }) => dtype.is_float(),
            (value, Ty::Int | Ty::UInt | Ty::Float64) => {
                value.clone().materialize_as(target).is_some()
            }
            _ => false,
        }
    }

    /// An ordinary value use of a reference-returning expression owns an
    /// independent value: MIR lowers it to `ReadRef` followed by `CopyValue`.
    /// Reference-bearing contexts are recorded in `reference_value_uses` and
    /// retain the handle instead. Validate the remaining reads after the whole
    /// tree has been checked so a later enclosing context can mark a nested
    /// result as a retained handle before this distinction is enforced.
    fn check_reference_result_reads(&self) -> Result<(), TypeError> {
        let operations = self.operation_adjustments.borrow();
        let retained_handles = self.reference_value_uses.borrow();
        let copyable_reads = self.copyable_reference_result_reads.borrow();
        let borrowed_call_reads = self.borrowed_read_call_places.borrow();
        let borrowed_receivers = self.borrowed_reference_receivers.borrow();
        for (span, adjustment) in operations.iter() {
            let mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } =
                adjustment
            else {
                continue;
            };
            if retained_handles.contains_key(span)
                || copyable_reads.contains(span)
                || borrowed_call_reads.contains(span)
                || borrowed_receivers.contains(span)
                || self.is_implicitly_copyable(&reference.referent)
            {
                continue;
            }
            let context = "ordinary value read through a reference result".to_string();
            if !self.is_copyable(&reference.referent) {
                return Err(TypeError::NonCopyable {
                    ty: reference.referent.to_string(),
                    context,
                });
            }
            return Err(TypeError::ImplicitCopy {
                ty: reference.referent.to_string(),
                context,
                transferable: false,
                copyable: true,
            });
        }
        Ok(())
    }

    /// Compiler-generated keyword collectors use the bundled self-hosted
    /// `StringDict`. Its current `List`/`DictEntry` implementation is copy-based,
    /// so accept only element types that it can store without duplicating a
    /// linear value. Reference-bearing values also wait for collector origin
    /// metadata instead of silently losing their loans at the call boundary.
    fn kwargs_collector_ty(&self, element: Ty, context: &str) -> Result<Ty, TypeError> {
        if !self.is_copyable(&element) {
            return Err(TypeError::TraitNotSatisfied {
                param: "V".to_string(),
                ty: element.to_string(),
                trait_name: "Copyable".to_string(),
                reason: Some(format!(
                    "{context} is materialized as StringDict[V], whose current storage is copy-based"
                )),
            });
        }
        if self.type_contains_reference(&element) {
            return Err(TypeError::Unsupported(format!(
                "{context} cannot contain references until keyword collectors carry origin metadata"
            )));
        }
        Ok(Ty::Struct(
            "StringDict".to_string(),
            vec![TyArg::Ty(element)],
        ))
    }

    /// Require `expr` to have type `Bool` (used for `if`/`while` conditions).
    fn expect_bool(&self, expr: &Expr, context: &str) -> Result<(), TypeError> {
        let ty = self.infer(expr)?;
        if ty == Ty::Bool
            || matches!(
                ty,
                Ty::Simd {
                    dtype: mojito_ast::ast::Dtype::Bool,
                    width: 1
                }
            )
        {
            Ok(())
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Bool".to_string(),
                found: ty.to_string(),
                context: context.to_string(),
            })
        }
    }
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, PartialEq)]
struct MethodSig {
    decls: Vec<ParamDecl>,
    availability: Vec<GenericConstraint>,
    has_self: bool,
    /// Regular parameters only; variadic element type is stored separately.
    params: Vec<Ty>,
    names: Vec<String>,
    required: Vec<bool>,
    variadic: Option<Box<Ty>>,
    variadic_index: Option<usize>,
    kw_variadic: Option<Box<Ty>>,
    kw_variadic_index: Option<usize>,
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    conventions: Vec<Option<ArgConvention>>,
    ret: Ty,
    raises: bool,
    error: Option<Box<Ty>>,
    /// Receiver convention. `None` means plain read-only `self`; explicit
    /// conventions (`mut`, `var`, `ref`, ...) are preserved so trait
    /// requirements can compare them exactly. Today only `mut self` changes call
    /// checking behavior.
    self_convention: Option<mojito_ast::ast::ArgConvention>,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<mojito_types::origin::RefSig>,
    implicit: bool,
    /// Origin parameters the body writes through via a parametric-mut ref
    /// field subscript (`self.field[i] = v`). The write is legal only for
    /// instantiations binding the origin to a mutable source, judged at each
    /// call site against the receiver's concrete origin arguments.
    parametric_origin_writes: Vec<mojito_types::origin::OriginParamId>,
}

impl MethodSig {
    fn intrinsic(params: Vec<Ty>, ret: Ty) -> MethodSig {
        let len = params.len();
        MethodSig {
            decls: Vec::new(),
            availability: Vec::new(),
            has_self: true,
            params,
            names: (0..len).map(|i| format!("arg{i}")).collect(),
            required: vec![true; len],
            variadic: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            conventions: vec![None; len],
            ret,
            raises: false,
            error: None,
            self_convention: None,
            ref_params: vec![None; len],
            ref_return: None,
            implicit: false,
            parametric_origin_writes: Vec::new(),
        }
    }
}

/// The checked signature of a struct, kept in the checker's registry.
struct StructInfo {
    /// Compile-time parameters (type and value); empty for a non-generic struct.
    decls: Vec<ParamDecl>,
    /// Raw source parameter list as declared, including Origin/OriginSet
    /// parameters and their `mut=` Bool binders, which `decls` erases.
    /// Direct applications consult it to accept, validate, and erase explicit
    /// origin arguments (`EntryIter[K, V, some_origin]`).
    source_params: Vec<mojito_ast::ast::TypeParam>,
    /// Concrete semantic arguments retained by an erased compiler-generated
    /// specialization. Public `Tuple[*Ts]` is emitted as a parameter-free
    /// implementation struct, but its checked identity must still carry the
    /// element types selected for `Ts`.
    fixed_arguments: Option<Vec<TyArg>>,
    /// Traits this struct declares conformance to (verified at definition).
    conforms: Vec<String>,
    callable_conformance: Option<Ty>,
    /// Exact lowered `__call__` method selected by the declared callable
    /// conformance. This is distinct from the callable contract: overloaded
    /// methods with the same arity require the signature-qualified target to
    /// survive into indirect-call MIR.
    callable_target: Option<String>,
    conformance_conditions: HashMap<String, Expr>,
    /// Declared fields, in order (drives the fieldwise constructor).
    fields: Vec<(String, Ty)>,
    /// For each field whose SOURCE annotation applied origin-binder
    /// arguments (`var iter: EntryIter[Self.o2]`), the (callee origin-param
    /// index, enclosing origin-param index) pairs the application bound —
    /// both in their declaration lists' full-index (OriginParamId) domain.
    /// Origin arguments are erased from checked identity, so this is the
    /// surviving record delegated-call origin clauses resolve binder
    /// correspondences through.
    field_origin_arguments: HashMap<String, Vec<(u32, u32)>>,
    /// Associated compile-time facts declared by `comptime NAME = ...` in the
    /// struct body. These live on the type, not on runtime instances.
    associated: HashMap<String, CtValue>,
    /// Availability constraints for monomorphic associated members, one entry
    /// per trailing `where` clause. They are evaluated against the enclosing
    /// struct arguments at projection time.
    associated_constraints: HashMap<String, Vec<GenericConstraint>>,
    /// Parameterized associated types declared by `comptime NAME[params] = body`.
    /// Unlike `associated`, the body cannot be evaluated eagerly (it references
    /// the member's own parameters); it is lowered once to a symbolic template
    /// and substituted per application. See [`ParameterizedMember`].
    parameterized_associated: HashMap<String, ParameterizedMember>,
    methods: HashMap<String, Vec<MethodSig>>,
    fieldwise_init: bool,
    explicit_destroy_message: Option<String>,
    explicit_destructors: HashMap<String, bool>,
}

/// A generic top-level alias (`comptime Alias[params] = Type` or a Bool
/// proposition). The parameters are classified `ParamDecl`s — trailing `where`
/// clauses attach to the last one — so each type-bodied application validates
/// arity, bounds, defaults, and declaration constraints through the same
/// `resolve_use_params` contract as a struct application. A type body is
/// lowered once to a symbolic template (`Ty::Param` / `CtValue::Param`) and
/// substituted per application; a Bool body is lowered once to a symbolic
/// [`GenericConstraint`] and inlined into the consuming proposition per
/// application. Aliases lower sequentially at declaration, so a body may
/// reference only already-declared names: self-reference fails as an unknown
/// type, and an alias expanding an earlier alias bakes the expansion into its
/// template. Origin parameters are rejected at declaration, so no origin
/// bindings exist.
#[derive(Clone)]
struct ComptimeAlias {
    decls: Vec<ParamDecl>,
    body: AliasBody,
}

/// The lowered body of a [`ComptimeAlias`]: a symbolic type template, or a
/// symbolic Bool proposition (a predicate alias, usable exactly where
/// `conforms_to`/`IsTrivially*` propositions are — never in type positions).
#[derive(Clone)]
enum AliasBody {
    Type(Box<Ty>),
    Predicate(Box<GenericConstraint>),
}

/// A parameterized associated type a conforming struct defines
/// (`comptime Buf[n: Int] = Fixed[n]`). The body is lowered once with the
/// member's own parameters in scope, so the resulting `template` carries them
/// symbolically (`Ty::Param`, `CtValue::Param`, `Origin::Param`); concrete
/// resolution substitutes an application's arguments into it. The raw
/// `TypeParam`s are retained (rather than classified `ParamDecl`s) so the
/// argument-to-parameter binding can distinguish type, value, and origin kinds.
#[derive(Clone)]
struct ParameterizedMember {
    params: Vec<mojito_ast::ast::TypeParam>,
    template: Ty,
    /// Constraints (one per trailing `where` clause) evaluated against both
    /// enclosing struct arguments and the member application's explicit
    /// arguments.
    availability: Vec<GenericConstraint>,
    /// The index the member's parameters started at in `enclosing_type_params`
    /// while the template was lowered. An origin parameter at member position `k`
    /// therefore appears in the template as `Origin::Param(param_base + k)`, which
    /// concrete substitution uses to bind the application's origin argument.
    param_base: usize,
}

/// A struct's checked associated compile-time members: the eagerly evaluated
/// monomorphic values, and the parameterized members lowered to their symbolic
/// templates for later concrete substitution.
type StructAssociatedMembers = (
    HashMap<String, CtValue>,
    HashMap<String, Vec<GenericConstraint>>,
    HashMap<String, ParameterizedMember>,
);

#[derive(Clone, Copy)]
struct DependentIndexAccessorFamily {
    place: &'static str,
    value: &'static str,
}

/// The checked signature of a trait: required methods plus associated
/// compile-time facts. A method requirement's signature may mention
/// `Ty::SelfType` (the conforming type).
struct TraitInfo {
    refines: Vec<String>,
    methods: HashMap<String, Vec<MethodSig>>,
    comptime_members: HashMap<String, CtMemberReq>,
    /// Per-member declaration constraints, one entry per trailing `where`
    /// clause on the requirement.
    comptime_constraints: HashMap<String, Vec<GenericConstraint>>,
}

/// A concrete method candidate after receiver-type substitution and argument
/// scoring. Named fields keep overload resolution readable as it evolves.
/// One body's transfer-effect accumulation frame: the callable's key and
/// its ordered parameter owners, so accepted outliving stores abstract to
/// signature-relative effects.
struct TransferFrame {
    callable: String,
    param_owners: Vec<mojito_types::origin::OwnerId>,
    /// Whether each parameter's convention borrows caller storage
    /// (`mut`/`ref`) rather than owning a moved value.
    param_borrowed: Vec<bool>,
    self_owner: Option<mojito_types::origin::OwnerId>,
    /// Compile-time callable value parameters (decl names) in scope in this
    /// body — call-through recording keys on them.
    value_callables: Vec<String>,
    effects: Vec<mojito_checked::checked::TransferEffect>,
    call_throughs: Vec<mojito_checked::checked::CallThroughEffect>,
}

struct MethodCallResolution {
    conversion_score: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
    variadic_element: Option<Ty>,
    keyword_element: Option<Ty>,
    conventions: Vec<Option<ArgConvention>>,
    self_convention: Option<ArgConvention>,
    return_type: Ty,
    result_adapter: Option<mojito_checked::checked::CheckedResultAdapter>,
    raises: bool,
    error: Option<Box<Ty>>,
    mutates_receiver: bool,
    consumes_receiver: bool,
    lowered_name: Option<String>,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<mojito_types::origin::RefSig>,
    param_types: Vec<Ty>,
    param_decls: Vec<ParamDecl>,
    /// See [`MethodSig::parametric_origin_writes`].
    parametric_origin_writes: Vec<mojito_types::origin::OriginParamId>,
}

type SubscriptDescriptorPlan = (Vec<Option<SliceKind>>, bool);

/// How strictly a storage annotation must bind explicit origin slots.
/// `Full`: bare origin-slotted generics and partial applications both reject
/// (struct fields, uninitialized locals). `AllowBare`: a bare generic may
/// infer wholly from the binding's initializer, but a partial application
/// still cannot omit an origin slot (initialized locals — both halves
/// probe-attested against the pinned upstream). `Off`: ordinary resolution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageStrictness {
    Off,
    AllowBare,
    Full,
}

const CONVERSION_RANK: usize = 1 << 24;

const VARIADIC_RANK: usize = 1 << 16;

const SIGNATURE_LENGTH_RANK: usize = 1 << 8;

/// Source-level arguments attached to a method invocation. Keeping the runtime
/// and compile-time argument lists together prevents the two method-resolution
/// paths from slowly acquiring different call-shape parameters.
#[derive(Clone, Copy)]
struct MethodCallArguments<'a> {
    param_args: &'a [mojito_ast::ast::ParamArg],
    args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
    parameterized_syntax: bool,
    /// The caller separately records a more precise projected write, so a
    /// `mut self` call must not also invalidate every receiver interior.
    preserves_receiver_interiors: bool,
}

impl<'a> MethodCallArguments<'a> {
    fn ordinary(args: &'a [Expr], kwargs: &'a [mojito_ast::ast::KwArg]) -> Self {
        Self {
            param_args: &[],
            args,
            kwargs,
            parameterized_syntax: false,
            preserves_receiver_interiors: false,
        }
    }

    fn interior_preserving(args: &'a [Expr], kwargs: &'a [mojito_ast::ast::KwArg]) -> Self {
        Self {
            preserves_receiver_interiors: true,
            ..Self::ordinary(args, kwargs)
        }
    }

    fn parameterized(
        param_args: &'a [mojito_ast::ast::ParamArg],
        args: &'a [Expr],
        kwargs: &'a [mojito_ast::ast::KwArg],
    ) -> Self {
        Self {
            param_args,
            args,
            kwargs,
            parameterized_syntax: true,
            preserves_receiver_interiors: false,
        }
    }
}

type ReturnRefContract = (
    mojito_types::origin::RefSig,
    Vec<mojito_types::origin::OwnerId>,
    Option<mojito_types::origin::OriginPlace>,
);

#[derive(Clone)]
struct CapturePolicy {
    /// Scope index at which the nested function's own locals begin.
    base: usize,
    function_name: String,
    declaration: SourceSpan,
    entries: HashMap<String, mojito_ast::ast::CaptureKind>,
    default: Option<mojito_ast::ast::CaptureKind>,
    /// Whether this policy belongs to a lambda expression's hidden definition
    /// (selects the lambda wording for capture-convention diagnostics).
    lambda: bool,
}

/// How one source-level `Origin` parameter is represented by the callable's
/// slot-relative reference contracts. Origin parameters are erased from
/// `Ty::Func`'s ordinary compile-time parameter list, so this checker-owned fact
/// is what lets a value expression such as `borrow[origin_of(value)]` bind that
/// parameter without reconstructing it from source later in MIR lowering.
#[derive(Clone)]
struct CallableOriginParam {
    name: String,
    slots: Vec<usize>,
}

/// One source compile-time parameter in a callable declaration. `Origin`
/// parameters are erased from `Ty::GenericFunc::decls`, so retaining this
/// ordered layout is necessary to split a mixed specialization such as
/// `borrow[Int, origin_of(value)]` without shifting the ordinary type argument.
#[derive(Clone)]
struct CallableSourceParam {
    name: String,
    infer_only: bool,
    origin: Option<usize>,
    ordinary: bool,
}

/// Origin-specialization metadata for one declaration in an overload set.
/// Entries are registered in the same order as `Ty::Overload` candidates.
#[derive(Clone)]
struct CallableOriginSignature {
    origins: Vec<CallableOriginParam>,
    source: Vec<CallableSourceParam>,
    /// Declaration constraints whose only binders were erased origin
    /// metadata, one per trailing `where` clause. They are checked after
    /// call-origin solving recovers the inferred Bool mutability arguments.
    availability: Vec<GenericConstraint>,
}

/// The source-level pieces of a struct declaration passed through checking.
struct StructDeclaration<'a> {
    module: &'a Option<String>,
    name: &'a str,
    type_params: &'a [mojito_ast::ast::TypeParam],
    conforms: &'a [String],
    callable_conformance: &'a Option<SourceType>,
    conformance_conditions: &'a [(String, Expr)],
    where_clauses: &'a [Expr],
    fields: &'a [mojito_ast::ast::Param],
    associated: &'a [StructComptime],
    methods: &'a [Method],
    fieldwise_init: bool,
    decorators: &'a [mojito_ast::ast::Decorator],
}

type MethodInstantiation = (
    Vec<Ty>,
    Option<Ty>,
    Option<Ty>,
    HashMap<String, Ty>,
    HashMap<String, TyArg>,
);

struct MethodCallScore {
    rank: usize,
    slots: Vec<ArgSlot>,
    positional_overflow: Vec<usize>,
    keyword_overflow: Vec<usize>,
}

/// Interior-generation state immediately before applying one selected method
/// contract. Candidate scoring has already inferred the argument expressions at
/// this point, so subtracting this snapshot isolates callee effects from effects
/// which belong to evaluation of the source expression itself.
struct CallBoundarySnapshot {
    invalidations: HashMap<SourceSpan, Vec<mojito_checked::checked::InteriorInvalidation>>,
}

#[derive(Clone)]
struct ValueAdjustmentSnapshot {
    source: SourceSpan,
    overload_target: Option<String>,
    implicit_conversion: Option<String>,
    operation: Option<mojito_checked::checked::SemanticAdjustment>,
}

struct SubscriptResolution {
    return_type: Ty,
    lowered_name: Option<String>,
    value_keyword: bool,
}

type SplitCallableSpecialization = (
    Vec<mojito_ast::ast::ParamArg>,
    Vec<(Vec<usize>, mojito_types::origin::Origin)>,
);

mod places;

use places::*;

mod generics;

use generics::*;

mod declarations;

use declarations::*;

mod annotations;

use annotations::*;

mod calls;

use calls::*;

mod builtins;

pub use builtins::{builtin_copy_is_value_read, callable_environment_coerces};

use builtins::*;

mod operators;

use operators::*;

mod iteration;

mod type_resolution;

mod constraints;

mod scopes;

mod origins;

pub use mojito_symbol::symbol::callable_contract_target;
pub use mojito_types::types::{callable_bound_accepts, callable_contract_ty};
use origins::*;

mod traits;

mod inference;

mod indexing;

mod method_calls;

mod call_inference;

mod statements;

#[cfg(test)]
mod dependent_callable_signature_tests {
    use super::*;

    fn indexed_callable(binder: &str, offset: i64) -> Ty {
        let index = if offset == 0 {
            CtExpr::Param(binder.to_string())
        } else {
            CtExpr::Add(
                Box::new(CtExpr::Param(binder.to_string())),
                Box::new(CtExpr::Value(CtValue::Int(offset))),
            )
        };
        Ty::GenericFunc {
            environment: mojito_types::origin::CallableEnvironment::Thin,
            decls: vec![ParamDecl::Value {
                name: binder.to_string(),
                ty: Box::new(Ty::Int),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            }],
            params: vec![Ty::Dependent(DependentType::Indexed {
                elements: vec![Ty::Int, Ty::StringLiteral],
                index,
            })],
            names: vec!["element".to_string()],
            ret: Box::new(Ty::None),
            required: vec![true],
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: vec![Some(ArgConvention::Var)],
            ref_params: Box::new(vec![None]),
            ref_return: None,
            transfers: Default::default(),
        }
    }

    fn callable_with_parameter_role(role: mojito_ast::ast::ParamKind) -> Ty {
        let mut callable = indexed_callable("index", 0);
        let Ty::GenericFunc {
            params,
            names,
            required,
            variadic,
            kw_variadic,
            conventions,
            ref_params,
            ..
        } = &mut callable
        else {
            unreachable!("indexed_callable constructs a generic function")
        };
        let parameter = params.pop().expect("one regular parameter");
        names.clear();
        required.clear();
        conventions.clear();
        ref_params.clear();
        match role {
            mojito_ast::ast::ParamKind::Regular => params.push(parameter),
            mojito_ast::ast::ParamKind::Variadic => *variadic = Some(Box::new(parameter)),
            mojito_ast::ast::ParamKind::KwVariadic => *kw_variadic = Some(Box::new(parameter)),
        }
        callable
    }

    #[test]
    fn dependent_callable_binders_are_alpha_equivalent() {
        assert!(same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 0),
        ));
        assert!(!same_callable_signature(
            &indexed_callable("index", 0),
            &indexed_callable("i", 1),
        ));
    }

    #[test]
    fn generic_parameter_roles_are_not_redeclaration_equivalent() {
        let regular = callable_with_parameter_role(mojito_ast::ast::ParamKind::Regular);
        let variadic = callable_with_parameter_role(mojito_ast::ast::ParamKind::Variadic);
        let kw_variadic = callable_with_parameter_role(mojito_ast::ast::ParamKind::KwVariadic);
        assert!(!same_callable_signature(&regular, &variadic));
        assert!(!same_callable_signature(&regular, &kw_variadic));
        assert!(!same_callable_signature(&variadic, &kw_variadic));
    }
}
