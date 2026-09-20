//! Checked templates: the body-local facts a generic declaration's one
//! symbolic check produced, retained so an instantiation can inherit them by
//! substitution instead of being checked again as a clone.
//!
//! A [`CheckedTemplate`] is not executable and never reaches HIR or MIR. The
//! checker derives an instance's facts from it, installs them in its own
//! tables, and only the finished `CheckedProgram` crosses the handoff. A
//! template whose facts have no total derivation recipe carries
//! [`TemplateCoverage::Incomplete`] and its instances keep the clone check.
//!
//! The design record is `docs/notes/instantiation-from-template.md`.

use crate::checked::{EffectFacts, GenericInstantiation, SemanticAdjustment};
use mojito_common::token::{Span, SyntaxId};
use mojito_types::types::{ParamDecl, Ty};

/// The identity of one prepared generic declaration.
///
/// The byte range tells overloads of one name apart, and `owner` names the
/// enclosing struct of a method, so neither a name nor a range alone
/// identifies a template.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TemplateId {
    pub module: Option<String>,
    pub owner: Option<String>,
    pub name: String,
    pub declaration: Span,
}

impl std::fmt::Display for TemplateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(owner) = &self.owner {
            write!(f, "{owner}.")?;
        }
        write!(f, "{}", self.name)
    }
}

/// Every occurrence-keyed fact table the checker fills while inferring a body.
///
/// A derivation accounts for each: it carries the table's entries across, or
/// it refuses the body because the table is not empty there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FactTable {
    OverloadTargets,
    ContextualBases,
    GenericInstantiations,
    MethodInstantiations,
    CallTransfers,
    ImplicitConversions,
    ImplicitConversionTypes,
    ImplicitConversionRaises,
    ConversionSourceBorrows,
    SimdConstructions,
    ParameterizedMethodCalls,
    OperationAdjustments,
    ConstructionImmutableBinders,
    CallResultOrigins,
    TupleUnpackPlans,
    InteriorReferences,
    ViewResultInteriors,
    CallParameters,
    InteriorInvalidations,
    ExpressionTypes,
    ExpressionBindings,
    StatementBindings,
    WithDesugars,
    DeclarationCaptures,
    ComprehensionBindings,
    ExpressionPlaceTypes,
    BindingTypes,
    ExpressionEffects,
    SelectedCalls,
    SubscriptDescriptors,
    IterationProtocols,
    ExplicitDestroyCalls,
    ReferenceValueUses,
    CopyableReferenceResultReads,
    DiscardedReferenceResults,
    BorrowedReferenceReceivers,
    CopyPlaceValueUses,
    CallPlaceUses,
    BorrowedReadCallPlaces,
    ReadTemporaryArguments,
    UnconsumedTemporaries,
    LinearTemporaries,
    ImplicitlyCopiedConsumingReceivers,
    TruthinessConditions,
    DeletableBindings,
    LinearBindings,
    RebindAssertions,
}

impl FactTable {
    pub const ALL: [Self; 47] = [
        Self::OverloadTargets,
        Self::ContextualBases,
        Self::GenericInstantiations,
        Self::MethodInstantiations,
        Self::CallTransfers,
        Self::ImplicitConversions,
        Self::ImplicitConversionTypes,
        Self::ImplicitConversionRaises,
        Self::ConversionSourceBorrows,
        Self::SimdConstructions,
        Self::ParameterizedMethodCalls,
        Self::OperationAdjustments,
        Self::ConstructionImmutableBinders,
        Self::CallResultOrigins,
        Self::TupleUnpackPlans,
        Self::InteriorReferences,
        Self::ViewResultInteriors,
        Self::CallParameters,
        Self::InteriorInvalidations,
        Self::ExpressionTypes,
        Self::ExpressionBindings,
        Self::StatementBindings,
        Self::WithDesugars,
        Self::DeclarationCaptures,
        Self::ComprehensionBindings,
        Self::ExpressionPlaceTypes,
        Self::BindingTypes,
        Self::ExpressionEffects,
        Self::SelectedCalls,
        Self::SubscriptDescriptors,
        Self::IterationProtocols,
        Self::ExplicitDestroyCalls,
        Self::ReferenceValueUses,
        Self::CopyableReferenceResultReads,
        Self::DiscardedReferenceResults,
        Self::BorrowedReferenceReceivers,
        Self::CopyPlaceValueUses,
        Self::CallPlaceUses,
        Self::BorrowedReadCallPlaces,
        Self::ReadTemporaryArguments,
        Self::UnconsumedTemporaries,
        Self::LinearTemporaries,
        Self::ImplicitlyCopiedConsumingReceivers,
        Self::TruthinessConditions,
        Self::DeletableBindings,
        Self::LinearBindings,
        Self::RebindAssertions,
    ];
}

/// One operation adjustment for an instance, its types rewritten by
/// `substitute`.
///
/// `None` means its derivation has no recipe yet and the body keeps the clone
/// check. The match is exhaustive on purpose: a new adjustment must be given
/// a policy here before this crate builds (the enum is `non_exhaustive` only
/// to other crates).
pub fn derive_adjustment(
    adjustment: &SemanticAdjustment,
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Option<SemanticAdjustment> {
    match adjustment {
        // The literal's value is fixed by the source; its target substitutes.
        // A target that is still a parameter would owe a range check per
        // instance, so only a closed target derives.
        SemanticAdjustment::MaterializeLiteral(target) => {
            let target = substitute(target);
            (!mojito_types::types::is_symbolic(&target))
                .then_some(SemanticAdjustment::MaterializeLiteral(target))
        }
        SemanticAdjustment::ResolveCallable(..)
        | SemanticAdjustment::ConstructTypeParam { .. }
        | SemanticAdjustment::ReifyTypeArgument { .. }
        | SemanticAdjustment::SelectedCall(..)
        | SemanticAdjustment::AugmentedSubscript(..)
        | SemanticAdjustment::AugmentedInPlace(..)
        | SemanticAdjustment::IndexNormalization { .. }
        | SemanticAdjustment::ParameterizedMethodCall { .. }
        | SemanticAdjustment::FieldInvocation { .. }
        | SemanticAdjustment::ElementInvocation(..)
        | SemanticAdjustment::InstantiatedCallableContract { .. }
        | SemanticAdjustment::ImplicitConversion(..)
        | SemanticAdjustment::ConversionResultType(..)
        | SemanticAdjustment::ConversionRaises(..)
        | SemanticAdjustment::BorrowShared
        | SemanticAdjustment::BorrowMutable
        | SemanticAdjustment::BorrowRefArguments { .. }
        | SemanticAdjustment::MaterializeBorrowSource { .. }
        | SemanticAdjustment::BorrowConversionSource { .. }
        | SemanticAdjustment::BorrowViewResult { .. }
        | SemanticAdjustment::CopyPlaceValue
        | SemanticAdjustment::ReferenceResult { .. }
        | SemanticAdjustment::TupleUnpack { .. }
        | SemanticAdjustment::RetainCallPlace
        | SemanticAdjustment::BorrowReadArgument
        | SemanticAdjustment::ReadTemporaryArgument
        | SemanticAdjustment::CallableCaptureAccesses(..)
        | SemanticAdjustment::EraseCompileTimeArgument
        | SemanticAdjustment::ImplicitlyCopyConsumingReceiver
        | SemanticAdjustment::NegatedEquality
        | SemanticAdjustment::ReflectedOperator
        | SemanticAdjustment::InvertedWrite
        | SemanticAdjustment::InvertedReprWrite
        | SemanticAdjustment::ReceiverFromFirstArgument { .. }
        | SemanticAdjustment::Truthiness
        | SemanticAdjustment::Move
        | SemanticAdjustment::ExplicitDestroy
        | SemanticAdjustment::Iterate(..)
        | SemanticAdjustment::ConstructSimd { .. }
        | SemanticAdjustment::SizeOf { .. }
        | SemanticAdjustment::TypeName { .. }
        | SemanticAdjustment::SimdCast { .. }
        | SemanticAdjustment::SimdToBits { .. }
        | SemanticAdjustment::SimdLength { .. }
        | SemanticAdjustment::DtypeConstant { .. }
        | SemanticAdjustment::DtypeFloatQuery { .. }
        | SemanticAdjustment::SimdShuffle { .. }
        | SemanticAdjustment::ConstructVariant { .. }
        | SemanticAdjustment::ConstructVariantInitWith { .. }
        | SemanticAdjustment::ConstructCollection { .. }
        | SemanticAdjustment::ConstructArrayLiteral { .. }
        | SemanticAdjustment::VariantIs { .. }
        | SemanticAdjustment::VariantProject { .. }
        | SemanticAdjustment::VariantSet { .. }
        | SemanticAdjustment::VariantTake { .. }
        | SemanticAdjustment::VariantReplace { .. }
        | SemanticAdjustment::PointerToPlace { .. }
        | SemanticAdjustment::PointerStorageTake { .. }
        | SemanticAdjustment::PointerStorageDestroy { .. }
        | SemanticAdjustment::PointerOffset
        | SemanticAdjustment::PointerOriginCast { .. }
        | SemanticAdjustment::UninitStorageMake { .. }
        | SemanticAdjustment::UninitStorageWrite { .. }
        | SemanticAdjustment::VariantSetInitWith { .. }
        | SemanticAdjustment::VariantDeinitWith { .. }
        | SemanticAdjustment::UninitStorageTake { .. }
        | SemanticAdjustment::UninitStorageDestroy { .. }
        | SemanticAdjustment::PointerWrite { .. }
        | SemanticAdjustment::SliceDescriptors { .. }
        | SemanticAdjustment::InteriorReference { .. }
        | SemanticAdjustment::InvalidateInteriors { .. } => None,
    }
}

/// Why a body's facts cannot be derived, so its instances are checked as
/// clones. A reason is never a verdict on the program: a failed constraint is
/// an error, not a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncompleteReason {
    /// The declaration is outside every class a derivation is enabled for.
    OutsideEnabledClass(&'static str),
    /// The body recorded facts in a table with no derivation recipe yet.
    UnsupportedTable(FactTable),
    /// The body recorded a fact keyed by something other than one of its own
    /// syntax occurrences.
    FactOutsideBody(FactTable),
    /// The body grew a fact store that is not keyed by occurrence, named
    /// here, for which no derivation recipe exists yet.
    UnkeyedFact(&'static str),
    /// A retained fact names a binding that is neither one of the
    /// declaration's parameters nor a local the body declares.
    ExternalBinding,
    /// Two occurrences of the body share one pre-rekey identity, so a fact
    /// cannot be attributed to either.
    AmbiguousOccurrence,
    /// A retained fact's type still mentions a parameter, in a class whose
    /// certificate requires closed facts.
    SymbolicFact,
    /// The source validation run that checked this body ended without a
    /// verdict, so nothing it recorded is certified.
    ValidationAborted,
}

impl IncompleteReason {
    /// A stable counter name under which timing reports this reason.
    pub const fn counter(&self) -> &'static str {
        match self {
            Self::OutsideEnabledClass(_) => "template_capture_incomplete.outside_enabled_class",
            Self::UnsupportedTable(_) => "template_capture_incomplete.unsupported_table",
            Self::FactOutsideBody(_) => "template_capture_incomplete.fact_outside_body",
            Self::UnkeyedFact(_) => "template_capture_incomplete.unkeyed_fact",
            Self::ExternalBinding => "template_capture_incomplete.external_binding",
            Self::AmbiguousOccurrence => "template_capture_incomplete.ambiguous_occurrence",
            Self::SymbolicFact => "template_capture_incomplete.symbolic_fact",
            Self::ValidationAborted => "template_capture_incomplete.validation_aborted",
        }
    }
}

impl std::fmt::Display for IncompleteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideEnabledClass(what) => write!(f, "outside the enabled class: {what}"),
            Self::UnsupportedTable(table) => write!(f, "no derivation recipe for {table:?}"),
            Self::FactOutsideBody(table) => {
                write!(f, "a {table:?} fact is keyed outside the body")
            }
            Self::UnkeyedFact(store) => write!(f, "no derivation recipe for {store}"),
            Self::ExternalBinding => f.write_str("a fact names a binding outside the body"),
            Self::AmbiguousOccurrence => f.write_str("two occurrences share one identity"),
            Self::SymbolicFact => f.write_str("a fact's type mentions a parameter"),
            Self::ValidationAborted => f.write_str("source validation ended without a verdict"),
        }
    }
}

/// The derivation class a certified template belongs to. Each class names the
/// constructs its bodies may hold; the soundness argument for each is in the
/// design record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateClass {
    /// A module-level trait-bound function whose body uses none of its type
    /// parameters: closed scalar expressions and `return`.
    ClosedScalarBody,
    /// [`Self::ClosedScalarBody`] plus direct calls of module-level
    /// functions that are not overloaded, passing literals, parameters, and
    /// further such calls to read parameters, and returning a scalar.
    FixedCalls,
    /// [`Self::FixedCalls`] plus the built-in `len` over a parameter whose
    /// bound promises a length, realized per instance against the concrete
    /// type's `__len__`.
    BoundedOperations,
    /// A module-level function source validation checks and the elaborator
    /// then stubs: keyed on scalar `Bool`/`Int` value parameters or plain
    /// type parameters through `comptime if`, or holding a `rebind`. Its body
    /// is `comptime if` over arms of the statements above. Every arm is
    /// checked once; an instance inherits the facts of the arms the
    /// elaborator selected and owes the `rebind` equalities they hold.
    ScalarBranches,
}

/// Whether a template's facts may stand in for a clone's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateCoverage {
    Certified(TemplateClass),
    Incomplete(IncompleteReason),
}

/// One syntax occurrence of a body, in template-local terms.
///
/// `syntax` is the identity the occurrence had before the final re-key, which
/// a clone occurrence shares with the template occurrence it was copied from.
/// `copy` tells apart the copies an unrolled `comptime for` makes of one
/// template occurrence, in pre-order; a template's own occurrences are all
/// copy zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OccurrenceId {
    pub syntax: SyntaxId,
    pub copy: u32,
}

/// One [`crate::checked::InteriorInvalidation`] with its bindings in
/// template-local terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateInvalidation {
    pub root: TemplateOwner,
    pub path: Vec<mojito_types::origin::OriginSeg>,
    pub except: Option<TemplateOwner>,
    pub include_base_generation: bool,
}

/// A binding a retained fact names, in template-local terms: checker owner
/// identities are per-run counters and mean nothing in another run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TemplateOwner {
    /// The declaration's runtime parameter at this index.
    Param(usize),
    /// The n-th local the body declares, in checking order.
    Local(u32),
    /// A module-scope binding, by name. An instance's call may name a clone
    /// where the template names the generic, so a derivation re-reads the
    /// name from the instance's own syntax.
    Global(String),
    /// One of the declaration's value parameters, which is a binding only
    /// while the body is checked symbolically. The elaborator folds every use
    /// into a literal, so no instance occurrence may carry this.
    CompileTimeParam(String),
}

/// One erased `rebind[Dest](value)`: the static assertion that the operand's
/// type is `Dest` once instantiated.
///
/// Source validation takes `dest` on faith while `operand` is symbolic, so a
/// template retains both and every instance owes their equality after
/// substitution. `by_value` is the overload upstream selects once, on the
/// declaration: a trivially register-passable operand is rebound by value and
/// is not a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebindAssertion {
    pub operand: Ty,
    pub dest: Ty,
    pub by_value: bool,
}

/// One runtime parameter of the callable a direct call selected, as the
/// callee declares it. The type is in the callee's binder scope, so an
/// instance's arguments are never substituted into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallParameterFact {
    pub name: String,
    pub convention: Option<mojito_ast::ast::ArgConvention>,
    pub ty: Ty,
}

/// A per-instance check a derivation still owes after substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateObligation {
    /// The declaration's own bounds and `where` clauses. The checker
    /// discharges them at the requesting call, and the elaborator proves a
    /// fully bound clone's `where` predicates before it generates the clone,
    /// so an instance that exists has already met them.
    DeclarationConstraints,
    /// Every retained [`RebindAssertion`] of the selected occurrences must
    /// hold after substitution. One that does not refuses the derivation, and
    /// the clone check reports the mismatch in its own words.
    RebindEqualities,
    /// Every place copied at a consuming position must be implicitly copyable
    /// at the instance's type, as `check_consuming` demands of a clone.
    ImplicitCopies,
}

/// The facts one body check recorded, keyed by the body's own syntax
/// occurrences rather than by a checker run's spans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedBodyFacts {
    /// Every statement and expression occurrence of the declaration, in
    /// pre-order. An instance must account for each.
    pub occurrences: Vec<OccurrenceId>,
    pub expression_types: Vec<(OccurrenceId, Ty)>,
    pub expression_place_types: Vec<(OccurrenceId, Ty)>,
    pub binding_types: Vec<(OccurrenceId, Ty)>,
    pub expression_bindings: Vec<(OccurrenceId, TemplateOwner)>,
    pub statement_bindings: Vec<(OccurrenceId, TemplateOwner)>,
    pub expression_effects: Vec<(OccurrenceId, EffectFacts)>,
    pub operation_adjustments: Vec<(OccurrenceId, SemanticAdjustment)>,
    /// The application each call of a generic function resolved to, its
    /// arguments in the caller's binder scope.
    pub generic_instantiations: Vec<(OccurrenceId, GenericInstantiation)>,
    /// The lowered callee of each call that names an existing clone or an
    /// overload.
    pub overload_targets: Vec<(OccurrenceId, String)>,
    pub call_parameters: Vec<(OccurrenceId, Vec<CallParameterFact>)>,
    pub borrowed_read_call_places: Vec<OccurrenceId>,
    pub read_temporary_arguments: Vec<OccurrenceId>,
    /// The callables whose transfer and call-through summaries the body
    /// read, every one of them empty: a derivation holds only while they
    /// still are.
    pub effect_free_callees: Vec<String>,
    /// Calls of the built-in `len`, which an instance realizes against its
    /// concrete argument type.
    pub builtin_len_calls: Vec<OccurrenceId>,
    pub rebind_assertions: Vec<(OccurrenceId, RebindAssertion)>,
    /// Places copied at a consuming position. An instance owes the copy: its
    /// concrete type must be implicitly copyable.
    pub copy_place_value_uses: Vec<OccurrenceId>,
    /// Interior generations each mutation invalidates, below a body-local
    /// binding.
    pub interior_invalidations: Vec<(OccurrenceId, Vec<TemplateInvalidation>)>,
    pub unconsumed_temporaries: Vec<OccurrenceId>,
    pub deletable_bindings: Vec<OccurrenceId>,
    /// How many locals the body declares.
    pub locals: u32,
}

impl CheckedBodyFacts {
    /// A template's facts laid out over an instance's occurrences, in the
    /// instance's pre-order.
    ///
    /// Each instance occurrence takes the facts of the template occurrence
    /// whose identity it kept. An occurrence the elaborator dropped — an
    /// untaken arm, a loop that ran zero times — takes its facts, requests,
    /// and effect reads with it; one it copied per loop iteration carries
    /// them once per copy.
    #[must_use]
    pub fn selected(&self, occurrences: &[OccurrenceId]) -> Self {
        fn at<V: Clone>(
            table: &[(OccurrenceId, V)],
            occurrences: &[OccurrenceId],
        ) -> Vec<(OccurrenceId, V)> {
            occurrences
                .iter()
                .filter_map(|occurrence| {
                    table
                        .iter()
                        .find(|(id, _)| id.syntax == occurrence.syntax)
                        .map(|(_, fact)| (*occurrence, fact.clone()))
                })
                .collect()
        }
        let flagged = |table: &[OccurrenceId]| -> Vec<OccurrenceId> {
            occurrences
                .iter()
                .copied()
                .filter(|occurrence| table.iter().any(|id| id.syntax == occurrence.syntax))
                .collect()
        };
        Self {
            occurrences: occurrences.to_vec(),
            expression_types: at(&self.expression_types, occurrences),
            expression_place_types: at(&self.expression_place_types, occurrences),
            binding_types: at(&self.binding_types, occurrences),
            expression_bindings: at(&self.expression_bindings, occurrences),
            statement_bindings: at(&self.statement_bindings, occurrences),
            expression_effects: at(&self.expression_effects, occurrences),
            operation_adjustments: at(&self.operation_adjustments, occurrences),
            generic_instantiations: at(&self.generic_instantiations, occurrences),
            overload_targets: at(&self.overload_targets, occurrences),
            call_parameters: at(&self.call_parameters, occurrences),
            borrowed_read_call_places: flagged(&self.borrowed_read_call_places),
            read_temporary_arguments: flagged(&self.read_temporary_arguments),
            // Realization recomputes these from the calls that remain.
            effect_free_callees: Vec::new(),
            builtin_len_calls: flagged(&self.builtin_len_calls),
            rebind_assertions: at(&self.rebind_assertions, occurrences),
            copy_place_value_uses: flagged(&self.copy_place_value_uses),
            interior_invalidations: at(&self.interior_invalidations, occurrences),
            unconsumed_temporaries: flagged(&self.unconsumed_temporaries),
            deletable_bindings: flagged(&self.deletable_bindings),
            locals: self.locals,
        }
    }

    /// A structural size estimate, in retained entries, for the
    /// `template_fact_entries` counter. It is not a byte count.
    pub const fn entries(&self) -> usize {
        self.occurrences.len()
            + self.expression_types.len()
            + self.expression_place_types.len()
            + self.binding_types.len()
            + self.expression_bindings.len()
            + self.statement_bindings.len()
            + self.expression_effects.len()
            + self.operation_adjustments.len()
            + self.generic_instantiations.len()
            + self.overload_targets.len()
            + self.call_parameters.len()
            + self.borrowed_read_call_places.len()
            + self.read_temporary_arguments.len()
            + self.effect_free_callees.len()
            + self.builtin_len_calls.len()
            + self.rebind_assertions.len()
            + self.copy_place_value_uses.len()
            + self.interior_invalidations.len()
            + self.unconsumed_temporaries.len()
            + self.deletable_bindings.len()
    }
}

/// Which check produced a template's facts.
///
/// There is one producer per body: source validation for a body it checks and
/// the elaborator then stubs, the executable check for a trait-bound body
/// that survives elaboration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateProducer {
    SourceValidation,
    ExecutableCheck,
}

/// One generic declaration's checked signature binders, body facts, and the
/// certificate saying whether instances may inherit them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedTemplate {
    pub id: TemplateId,
    pub producer: TemplateProducer,
    pub param_decls: Vec<ParamDecl>,
    pub facts: CheckedBodyFacts,
    pub coverage: TemplateCoverage,
    pub obligations: Vec<TemplateObligation>,
}

/// How an elaborated clone came from a prepared template.
///
/// This is the expansion trace's declaration-level half. The occurrence-level
/// half is the syntax identity each clone node kept from the template node it
/// was copied from.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceTrace {
    pub template: TemplateId,
    /// The type parameters the clone no longer declares, each with the source
    /// type the elaborator wrote in its place.
    pub type_bindings: Vec<(String, mojito_ast::ast::Type)>,
    /// The value parameters the clone no longer declares.
    pub value_bindings: Vec<(String, mojito_types::ct::CtValue)>,
    /// The parameters the clone still declares.
    pub residual: Vec<String>,
}

/// The per-compilation store of checked templates and clone traces.
///
/// It lives for one `Compiler::compile_linked`. Entries are bucketed by
/// [`TemplateId`] and compared exactly: `Ty` and `CtValue` equality is
/// structural and not uniformly hashable, and a mangled symbol erases origins,
/// so neither is a cache key.
#[derive(Debug, Default)]
pub struct TemplateCatalog {
    templates: Vec<CheckedTemplate>,
    traces: Vec<(InstanceName, InstanceTrace)>,
    /// Set when a source validation run ended without a verdict: nothing
    /// that run recorded may be certified.
    validation_aborted: bool,
    /// Compare each derived bundle with the clone check's own facts.
    verify: bool,
    stats: TemplateStats,
}

/// What the catalog did over one compilation, by declaration name. A name
/// appears once per check pass that visited it, so a test can tell "never
/// inferred" from "inferred once".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateStats {
    /// Generic bodies inferred and retained with a certificate.
    pub certified: Vec<String>,
    /// Clones served from a template without being inferred.
    pub derived: Vec<String>,
    /// Certified template bodies served from their own retained facts in a
    /// later pass.
    pub reused: Vec<String>,
    /// Traced clones that were inferred: outside every enabled class, or
    /// inferred for verification.
    pub inferred_clones: Vec<String>,
    /// Derived bundles that matched the clone check in verification mode.
    pub verified: Vec<String>,
}

/// An elaborated clone's declaration identity: the module tag the elaborator
/// stamped it with, and its output name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceName {
    pub module: Option<String>,
    pub name: String,
}

impl TemplateCatalog {
    pub fn new(verify: bool) -> Self {
        Self {
            verify,
            ..Self::default()
        }
    }

    pub const fn verify(&self) -> bool {
        self.verify
    }

    pub const fn stats(&self) -> &TemplateStats {
        &self.stats
    }

    pub const fn stats_mut(&mut self) -> &mut TemplateStats {
        &mut self.stats
    }

    pub const fn validation_aborted(&self) -> bool {
        self.validation_aborted
    }

    /// Record that a validation run ended without a verdict, and withdraw
    /// every certificate that run produced: a partial traversal proves
    /// nothing about the bodies it did reach.
    pub fn abort_validation(&mut self) {
        self.validation_aborted = true;
        for template in &mut self.templates {
            if template.producer == TemplateProducer::SourceValidation {
                template.coverage =
                    TemplateCoverage::Incomplete(IncompleteReason::ValidationAborted);
            }
        }
    }

    /// Whether source validation produced the record of `id`: the
    /// declaration of that identity in an elaborated program is then a
    /// trapping stub, whose body is not the template.
    pub fn validated(&self, id: &TemplateId) -> bool {
        self.template(id)
            .is_some_and(|template| template.producer == TemplateProducer::SourceValidation)
    }

    /// Record a template, replacing an earlier record of the same
    /// declaration.
    pub fn record(&mut self, template: CheckedTemplate) {
        match self
            .templates
            .iter_mut()
            .find(|existing| existing.id == template.id)
        {
            Some(existing) => *existing = template,
            None => self.templates.push(template),
        }
    }

    pub fn template(&self, id: &TemplateId) -> Option<&CheckedTemplate> {
        self.templates.iter().find(|template| template.id == *id)
    }

    pub fn templates(&self) -> &[CheckedTemplate] {
        &self.templates
    }

    /// Replace the clone traces with those of the elaboration about to be
    /// checked. Traces never outlive their elaboration: the next round names
    /// its own clones.
    pub fn set_traces(&mut self, traces: Vec<(InstanceName, InstanceTrace)>) {
        self.traces = traces;
    }

    pub fn trace(&self, instance: &InstanceName) -> Option<&InstanceTrace> {
        self.traces
            .iter()
            .find(|(name, _)| name == instance)
            .map(|(_, trace)| trace)
    }
}
