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

use crate::checked::{
    CallThroughEffect, CheckedCallArgumentSource, CheckedCallBoundary, CheckedCallContract,
    CheckedCallValueAdjustment, EffectFacts, GenericInstantiation, MethodInstantiation,
    SemanticAdjustment,
};
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
        // Element arithmetic names no type, and a take, a destroy, or a
        // moving write names only the pointee, which substitutes. Each is
        // selected by the method's name on a receiver that is a pointer under
        // every instance. A copying write also marks a reference-result read,
        // a table that takes type-dependent entries too, so it has no recipe.
        SemanticAdjustment::PointerOffset => Some(SemanticAdjustment::PointerOffset),
        SemanticAdjustment::PointerStorageTake { element } => {
            Some(SemanticAdjustment::PointerStorageTake {
                element: substitute(element),
            })
        }
        SemanticAdjustment::PointerStorageDestroy { element } => {
            Some(SemanticAdjustment::PointerStorageDestroy {
                element: substitute(element),
            })
        }
        SemanticAdjustment::PointerWrite {
            element,
            copy: false,
        } => Some(SemanticAdjustment::PointerWrite {
            element: substitute(element),
            copy: false,
        }),
        // An inverted write names no type. A closed receiver's stands as
        // recorded; one on a parameter-typed receiver is re-selected on the
        // instance's type after substitution (`realize_inverted_writes`),
        // where a nominal struct takes its own `write_to` instead.
        SemanticAdjustment::InvertedWrite => Some(SemanticAdjustment::InvertedWrite),
        // Which constructor arguments lend their place is the selected
        // constructor's declaration, and each loan's mutability the place's
        // own. A materialized temporary names a binding of one run.
        SemanticAdjustment::BorrowRefArguments {
            arguments,
            materialized: None,
        } => Some(SemanticAdjustment::BorrowRefArguments {
            arguments: arguments.clone(),
            materialized: None,
        }),
        // A view result's loans are the selected callee's result contract
        // over the call's own receiver and arguments, which no instance
        // changes. A materialized temporary names a binding of one run.
        SemanticAdjustment::BorrowViewResult { materialized: None } => {
            Some(SemanticAdjustment::BorrowViewResult { materialized: None })
        }
        SemanticAdjustment::InvertedReprWrite => Some(SemanticAdjustment::InvertedReprWrite),
        // A collection display or comprehension builds its target through
        // the target struct's own insert method, named by the struct, which
        // substitution keeps; only its arguments substitute.
        SemanticAdjustment::ConstructCollection { target, insert } => {
            let realized = substitute(target);
            matches!((target, &realized), (Ty::Struct(template, _), Ty::Struct(instance, _))
                if template == instance)
            .then(|| SemanticAdjustment::ConstructCollection {
                target: realized,
                insert: insert.clone(),
            })
        }
        // A type name is one type's spelling: the instance re-renders it from
        // the substituted type, as the resolution rendered the template's. A
        // type that is still symbolic has no spelling an instance could use.
        SemanticAdjustment::TypeName { ty, .. } => {
            let ty = substitute(ty);
            (!mojito_types::types::is_symbolic(&ty)).then(|| SemanticAdjustment::TypeName {
                text: mojito_symbol::symbol::unqualified_instance_name(&ty),
                ty,
            })
        }
        // An augmented subscript embeds its calls' contracts, whose
        // boundaries name one run's spans and bindings; an element store is
        // kept apart (`augmented_subscripts`) and rebuilt from the realized
        // call at its site and the contracts kept beside it.
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
        | SemanticAdjustment::BorrowRefArguments {
            materialized: Some(_),
            ..
        }
        | SemanticAdjustment::MaterializeBorrowSource { .. }
        | SemanticAdjustment::BorrowConversionSource { .. }
        | SemanticAdjustment::BorrowViewResult {
            materialized: Some(_),
        }
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
        | SemanticAdjustment::ReceiverFromFirstArgument { .. }
        | SemanticAdjustment::Truthiness
        | SemanticAdjustment::Move
        | SemanticAdjustment::ExplicitDestroy
        | SemanticAdjustment::Iterate(..)
        | SemanticAdjustment::ConstructSimd { .. }
        | SemanticAdjustment::SizeOf { .. }
        | SemanticAdjustment::SimdCast { .. }
        | SemanticAdjustment::SimdToBits { .. }
        | SemanticAdjustment::SimdLength { .. }
        | SemanticAdjustment::DtypeConstant { .. }
        | SemanticAdjustment::DtypeFloatQuery { .. }
        | SemanticAdjustment::SimdShuffle { .. }
        | SemanticAdjustment::ConstructVariant { .. }
        | SemanticAdjustment::ConstructVariantInitWith { .. }
        | SemanticAdjustment::ConstructArrayLiteral { .. }
        | SemanticAdjustment::VariantIs { .. }
        | SemanticAdjustment::VariantProject { .. }
        | SemanticAdjustment::VariantSet { .. }
        | SemanticAdjustment::VariantTake { .. }
        | SemanticAdjustment::VariantReplace { .. }
        | SemanticAdjustment::PointerToPlace { .. }
        | SemanticAdjustment::PointerOriginCast { .. }
        | SemanticAdjustment::UninitStorageMake { .. }
        | SemanticAdjustment::UninitStorageWrite { .. }
        | SemanticAdjustment::VariantSetInitWith { .. }
        | SemanticAdjustment::VariantDeinitWith { .. }
        | SemanticAdjustment::UninitStorageTake { .. }
        | SemanticAdjustment::UninitStorageDestroy { .. }
        | SemanticAdjustment::PointerWrite { copy: true, .. }
        | SemanticAdjustment::SliceDescriptors { .. }
        | SemanticAdjustment::InteriorReference { .. }
        | SemanticAdjustment::InvalidateInteriors { .. } => None,
    }
}

/// One selected method call in template-local terms.
///
/// A [`CheckedCallContract`]'s boundary names each argument by the span of
/// its source occurrence and each invalidated place by a binding identity,
/// and both belong to one checker run. Here the contract's own boundary is
/// left empty, and what it held is kept by occurrence and template owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCallContract {
    pub contract: CheckedCallContract,
    /// The contract's reference result, which names the receiver's binding.
    /// The contract's own is left empty, and its result type is the referent.
    pub reference_result: Option<TemplateReference>,
    /// The origin arguments of a view result's struct type, which name the
    /// receiver's or an argument's binding, in the order a pre-order walk of
    /// the type meets them. The contract's result type keeps those slots
    /// unbound (`TypedOrigins` keeps a retained type's the same way).
    pub result_origins: Vec<TemplateOrigin>,
    pub arguments: Vec<TemplateArgumentBoundary>,
    /// Receiver and call-site generation changes.
    pub invalidations: Vec<TemplateInvalidation>,
}

/// One [`crate::checked::CheckedCallArgumentBoundary`] in template-local
/// terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateArgumentBoundary {
    pub source: CheckedCallArgumentSource,
    pub value: OccurrenceId,
    pub adjustments: Vec<CheckedCallValueAdjustment>,
    pub invalidations: Vec<TemplateInvalidation>,
}

/// Whether a method call's contract holds nothing an instance could change
/// but its target.
///
/// That is a call with no arguments, on a plain read receiver, that neither
/// raises, adapts its result, returns a reference, captures, nor carries
/// compile-time parameters, and whose result is a closed type.
pub fn trivial_method_contract(call: &TemplateCallContract) -> bool {
    closed_method_contract(call)
        && !mojito_types::types::is_symbolic(&call.contract.result_ty)
        && !call.contract.receiver_requires_place
        && call.contract.receiver_convention.is_none()
        && call.contract.arguments.is_empty()
        && call.invalidations.is_empty()
}

/// Whether a method call's contract changes per instance only in its target
/// and, by substitution, its result type.
///
/// The receiver is read or mutated in place, never consumed or bound by a
/// `ref` whose mutability origin solving decides. Every argument is supplied
/// (no default is evaluated in the callee's scope) and either binds a closed
/// scalar parameter by value, adapted at most by materializing a literal to a
/// closed type, or is the caller's place kept for a `mut` or `ref` parameter
/// ([`kept_place_argument`]), which is never adapted. The call neither
/// raises, adapts its result, returns a reference, captures, nor carries
/// compile-time parameters. Whether the receiver needs a place is the
/// callee's declaration, which an instance's clone keeps. Every field is
/// named, so a new one must be given a rule here before this crate builds.
pub fn closed_method_contract(call: &TemplateCallContract) -> bool {
    call.reference_result.is_none() && closed_contract(call, None, false)
}

/// Whether a method call's contract is a [`closed_method_contract`] but for
/// the types its by-value parameters have.
///
/// An argument bound by value to a parameter of any type is still supplied,
/// still bound without a place, and invalidates nothing at its boundary. Its
/// type is either the parameter's, which substitution preserves, or one an
/// `@implicit` constructor converts to it, which an instance selects again
/// from its own source and target types. Either way an instance changes the
/// parameter's type and, for a conversion, the constructor the boundary
/// names. What the argument's own expression owes (a copy, a move, a
/// temporary) is recorded at that expression and is for the body's grammar
/// to admit.
pub fn value_method_contract(call: &TemplateCallContract) -> bool {
    call.reference_result.is_none() && closed_contract(call, None, true)
}

/// Whether a method call's contract is a [`closed_method_contract`] but for
/// the reference it returns.
///
/// The reference's origin is the callee's declared origin against the
/// receiver's place, and its mutability is the receiver binding's: neither
/// reads a struct parameter, so an instance changes only the referent, by
/// substitution, and gets its own receiver binding back in the origin. The
/// receiver needs a place, which a field of `self` is. A `ref self` accessor
/// takes that place as `imm` from a receiver the body may only read and as
/// `ref` from one it may write, which the body's own receiver decides.
pub fn closed_reference_contract(call: &TemplateCallContract) -> bool {
    call.reference_result.is_some()
        && call.contract.receiver_requires_place
        && closed_contract(call, Some(mojito_ast::ast::ArgConvention::Ref), false)
}

/// Whether a method call's contract is a [`closed_method_contract`] but for
/// a receiver it consumes.
///
/// The receiver is the `^` transfer the call spells, which records the move
/// at the receiver's own expression, not in the contract, so an instance's
/// consuming witness changes only the target, as a read one does. The
/// callee takes it as `var self` or `deinit self`.
pub fn consuming_method_contract(call: &TemplateCallContract) -> bool {
    use mojito_ast::ast::ArgConvention;
    let convention = call.contract.receiver_convention;
    call.reference_result.is_none()
        && matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
        && !call.contract.receiver_requires_place
        && closed_contract(call, convention, false)
}

/// Whether a method call's contract is a [`value_method_contract`] but for
/// a nominal receiver it consumes.
///
/// The receiver is the `^` transfer the call spells, as for
/// [`consuming_method_contract`]; the callee takes it as `var self` or as a
/// named `deinit self` destructor, a convention of the struct's own
/// declaration, which substitution does not change.
pub fn consuming_nominal_contract(call: &TemplateCallContract) -> bool {
    use mojito_ast::ast::ArgConvention;
    let convention = call.contract.receiver_convention;
    call.reference_result.is_none()
        && matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
        && !call.contract.receiver_requires_place
        && call.invalidations.is_empty()
        && closed_contract(call, convention, true)
}

/// Whether a call keeps the caller's place for this argument: what a `mut`
/// parameter, or a `ref` one that the call reads, records.
///
/// The callee's declared convention decides it, and the generations a `mut`
/// argument invalidates lie below the argument's own binding, which a
/// template keeps by owner. The parameter's type is for the body's grammar
/// to judge: a kept place is neither copied, moved, nor converted.
pub const fn kept_place_argument(argument: &crate::checked::CheckedCallArgument) -> bool {
    use mojito_ast::ast::ArgConvention;
    argument.requires_place
        && matches!(
            argument.convention,
            Some(ArgConvention::Mut | ArgConvention::Imm | ArgConvention::Ref)
        )
}

/// What [`closed_method_contract`], [`value_method_contract`],
/// [`closed_reference_contract`], and [`consuming_method_contract`] share.
/// `receiver` is the one convention beyond a read or `mut` receiver the call
/// may take, and `values` admits a by-value parameter of any type, which then
/// takes no adjustment at all.
fn closed_contract(
    call: &TemplateCallContract,
    receiver: Option<mojito_ast::ast::ArgConvention>,
    values: bool,
) -> bool {
    use mojito_ast::ast::ArgConvention;
    let TemplateCallContract {
        contract:
            CheckedCallContract {
                target: _,
                raises,
                result_ty: _,
                result_adapter,
                receiver_requires_place: _,
                receiver_elided,
                receiver_convention,
                arguments,
                captures,
                reference_result,
                parameter_arguments,
                param_decls,
                // Kept template-locally, in `call.arguments` and
                // `call.invalidations`.
                boundary:
                    CheckedCallBoundary {
                        arguments: _,
                        invalidations: _,
                    },
            },
        reference_result: _,
        result_origins: _,
        arguments: boundary_arguments,
        invalidations: _,
    } = call;
    let closed_scalar = |ty: &Ty| matches!(ty, Ty::Int | Ty::UInt | Ty::Bool | Ty::Float64);
    raises.is_none()
        && result_adapter.is_none()
        && !receiver_elided
        && (matches!(
            receiver_convention,
            None | Some(ArgConvention::Imm | ArgConvention::Mut)
        ) || (receiver.is_some() && *receiver_convention == receiver))
        && arguments.iter().all(|argument| {
            let by_value = (values || closed_scalar(&argument.parameter_ty))
                && !argument.requires_place
                && matches!(
                    argument.convention,
                    None | Some(ArgConvention::Imm | ArgConvention::Var)
                );
            argument.source != CheckedCallArgumentSource::Default
                && (by_value || kept_place_argument(argument))
        })
        && captures.is_empty()
        && reference_result.is_none()
        && parameter_arguments.is_empty()
        && param_decls.is_empty()
        && boundary_arguments.iter().all(|argument| {
            let kept = arguments
                .iter()
                .any(|bound| bound.source == argument.source && kept_place_argument(bound));
            let opaque = arguments.iter().any(|bound| {
                bound.source == argument.source && !closed_scalar(&bound.parameter_ty)
            });
            if kept {
                return argument.adjustments.is_empty();
            }
            argument.invalidations.is_empty()
                && argument
                    .adjustments
                    .iter()
                    .all(|adjustment| match adjustment {
                        // A closed type a literal materializes to is the one it
                        // has under every instance.
                        CheckedCallValueAdjustment::MaterializeLiteral { target } => {
                            !opaque && closed_scalar(target)
                        }
                        // The constructor an `@implicit` conversion names is
                        // re-selected per instance from the substituted source
                        // and target types, and written back here.
                        CheckedCallValueAdjustment::ImplicitConversion { .. } => values && opaque,
                        CheckedCallValueAdjustment::ResolveCallable { .. }
                        | CheckedCallValueAdjustment::IndexNormalization { .. } => false,
                    })
        })
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
    /// A construction bound an origin binder immutably (`ImmOrigin(o)`), a
    /// record the bundle does not carry.
    ImmutableBinder,
    /// A loop's iterator protocol is not one an instance selects again from
    /// its iterable's type alone.
    IterationRecipe,
    /// A tuple unpacking's plan is not one an instance derives again from
    /// the unpacked value's type and place alone.
    TupleUnpackRecipe,
    /// A comprehension binder is not one an instance declares again from
    /// its clause's iterator protocol alone.
    ComprehensionRecipe,
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
            Self::ImmutableBinder => "template_capture_incomplete.immutable_binder",
            Self::IterationRecipe => "template_capture_incomplete.iteration_recipe",
            Self::TupleUnpackRecipe => "template_capture_incomplete.tuple_unpack_recipe",
            Self::ComprehensionRecipe => "template_capture_incomplete.comprehension_recipe",
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
            Self::ImmutableBinder => f.write_str("a construction binds an origin immutably"),
            Self::IterationRecipe => {
                f.write_str("a loop's iterator protocol has no re-selection recipe")
            }
            Self::TupleUnpackRecipe => {
                f.write_str("a tuple unpacking's element reads have no derivation recipe")
            }
            Self::ComprehensionRecipe => {
                f.write_str("a comprehension binder has no derivation recipe")
            }
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
    /// A module-level function source validation checks and the elaborator
    /// then stubs, keyed on a type pack: its body is `comptime for` over the
    /// pack's indices, reading each element as `pack[i]` into `print`,
    /// beside the statements [`Self::ScalarBranches`] admits. The template
    /// checked the element once at the dependent type `Ts[i]`; an instance
    /// takes each unrolled copy at the element the fold fixed, and owes that
    /// element `Writable`.
    PackElements,
    /// A method of a generic struct with a plain read `self` and no binders
    /// of its own, returning a scalar over closed scalars, runtime
    /// parameters, and reads of `self`'s scalar fields.
    MethodScalarBody,
    /// A method of a generic struct beyond [`Self::MethodScalarBody`]: what
    /// its body holds is named by its [`MethodFeatures`], each of which the
    /// certificate argues for separately.
    MethodBody(MethodFeatures),
}

/// The constructs a [`TemplateClass::MethodBody`] holds beyond scalar `return`s.
///
/// They are independent of one another: a body may call a sibling without
/// moving a value of a parameter type, and the reverse.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MethodFeatures(u32);

impl MethodFeatures {
    /// A receiver other than a plain read `self`, or statements beyond a
    /// scalar `return`: scalar locals, scalar field writes, `if`, `while`.
    pub const STATEMENTS: Self = Self(1);
    /// A value whose type mentions a struct parameter, moved or copied whole
    /// between a parameter, a local, a field of `self`, and the result.
    pub const OPAQUE_MOVES: Self = Self(1 << 1);
    /// Element arithmetic, takes, destroys, and writes through a pointer
    /// field of `self`.
    pub const POINTER_SLOTS: Self = Self(1 << 2);
    /// A method call on `self` or one of its fields whose contract is closed
    /// but carries arguments or a `mut` receiver.
    pub const SIBLING_CALLS: Self = Self(1 << 3);
    /// A reference result: every `return` hands out a place of `self`, a
    /// field or a pointer slot, as a handle rather than a value.
    pub const REFERENCE_RESULT: Self = Self(1 << 4);
    /// A reference-returning method call on a field of `self`, passing
    /// scalars, forwarded as the method's own reference result or read by
    /// value.
    pub const REFERENCE_CALLS: Self = Self(1 << 5);
    /// A `ref` declaration binding a place of `self`, a parameter, a local,
    /// or a reference call's result, and the reads and scalar stores made
    /// through that binding.
    pub const REFERENCE_LOCALS: Self = Self(1 << 6);
    /// A field read or a closed method call through a reference: a reference
    /// call's result or a `ref` local whose referent is a struct.
    pub const REFERENCE_RECEIVERS: Self = Self(1 << 7);
    /// A store through a subscript of a field of `self`: a scalar field of
    /// the element a reference getter yields, an element a declared setter
    /// takes (a scalar, or a whole value of the setter's own parameter type),
    /// or a scalar element stored whole or augmented through the mutable
    /// reference its getter yields.
    pub const SUBSCRIPT_STORES: Self = Self(1 << 8);
    /// A place handed to a `mut` or bare `ref` parameter of a method call: a
    /// local, a parameter, or a field of `self`.
    pub const PLACE_ARGUMENTS: Self = Self(1 << 9);
    /// A `ref` parameter with an origin clause, and the origin binders of the
    /// method that names it.
    pub const ORIGIN_PARAMETERS: Self = Self(1 << 10);
    /// A whole value of any type handed to a by-value parameter of a method
    /// call: a moved place, a temporary, a copied place, or a place the call
    /// reads where it lies.
    pub const VALUE_ARGUMENTS: Self = Self(1 << 11);
    /// A call whose callee stores an argument outward, so that the template,
    /// whose parameter may stand for a loan-carrying type, replays a transfer
    /// summary there and an instance replays it again.
    pub const REPLAYED_TRANSFERS: Self = Self(1 << 12);
    /// An operator over two places of one type that mentions a struct
    /// parameter — a comparison, or an arithmetic, bitwise, or shift operator
    /// the bound proves — which an instance dispatches on its own type.
    pub const OPERATOR_DISPATCH: Self = Self(1 << 13);
    /// A method call on a place of a bare parameter type, which the template
    /// proves through the bound and an instance re-selects on its own type:
    /// `copy()`, `__hash__(hasher)`, `write_to(writer)`, or any other
    /// requirement whose arguments are scalars or bounded places.
    pub const BOUND_DISPATCH: Self = Self(1 << 14);
    /// `hasher.update(value)` and `writer.write(values…)` on a bounded
    /// parameter: checker builtins that select no callee and record nothing
    /// about an argument that its syntax does not decide.
    pub const BOUND_BUILTINS: Self = Self(1 << 15);
    /// The method's own trait-bounded type binders (`[H: Hasher]`), which a
    /// clone keeps and binds symbolically as the template does, so a
    /// construction of one (`H()`) is the same in every clone.
    pub const BOUND_BINDERS: Self = Self(1 << 16);
    /// A construction of a declared struct whose compile-time arguments are
    /// types, passing closed scalars, whole values, `copy:` of a named place,
    /// or a place lent to a `ref` parameter; an instance re-selects the
    /// constructor's clone on its own arguments.
    pub const CONSTRUCTIONS: Self = Self(1 << 17);
    /// A call through a runtime parameter declared with a `def(...)` type,
    /// passing closed scalars or whole values, and such a parameter forwarded
    /// to a sibling call: the residue either records on the body's frame is
    /// republished for the instance ([`TemplateObligation::CallThroughResidue`]).
    pub const CALLABLE_PARAMETERS: Self = Self(1 << 18);
    /// `repr(value)` and `_unqualified_type_name[T]()`: checker builtins that
    /// make a string from a value or from a type and select no callee. Each
    /// records by its argument's type alone, which an instance judges again
    /// ([`TemplateObligation::ImplicitConversions`]).
    pub const STRING_BUILTINS: Self = Self(1 << 19);
    /// A reference handed on as a call argument: a `ref` local, a field
    /// reached through a reference, or a reference call's result, read where
    /// it lies by a read parameter, copied into a `var` one, or kept by a
    /// `mut` or `ref` one.
    pub const REFERENCE_ARGUMENTS: Self = Self(1 << 20);
    /// A `raises` declaration, and a `raise` of a construction or of
    /// `Error("…")`: whether the operand matches the declared error type,
    /// and whether it is a string, hold alike under every instance.
    pub const RAISES: Self = Self(1 << 21);
    /// A method call on the `^` transfer of a named place whose callee
    /// consumes its receiver (`var self`, or a named `deinit self`
    /// destructor), on a nominal struct: the move is recorded at the
    /// transfer, and which methods a struct declares as destructors does not
    /// change with its arguments.
    pub const CONSUMING_CALLS: Self = Self(1 << 22);
    /// A method call whose callee consumes its receiver, on a named place
    /// the call copies first rather than on a `^` transfer
    /// (`slice.start.or_else(0)`): the copy is decided by the receiver's
    /// syntax and the callee's convention, and an instance owes it at its
    /// own type.
    pub const COPIED_RECEIVERS: Self = Self(1 << 23);
    /// A direct call of a module-scope function that is not generic and
    /// takes only closed scalars by value: it selects the same declaration
    /// under every instance.
    pub const DIRECT_CALLS: Self = Self(1 << 24);
    /// A runtime `for` over a place, the `^` transfer of an owned place, or a
    /// sibling call's result: the iterator protocol is selected from the
    /// iterable's type, which an instance substitutes and selects from again,
    /// and the loop variable is a local of the body.
    pub const ITERATION: Self = Self(1 << 25);
    /// A construction of a closed `SIMD`, `Scalar`, or scalar-alias value
    /// from closed scalars (`UInt8(1)`), handed to a checker builtin or to a
    /// by-value parameter: its dtype and width are the same under every
    /// instance, and it selects no callee.
    pub const SIMD_CONSTRUCTIONS: Self = Self(1 << 26);
    /// An `if` or `while` condition that is a place read whole — a
    /// parameter, a local, or a field of `self` — of a type tested through
    /// `__bool__` rather than read as a `Bool`: whether it converts is
    /// decided by its type, which an instance judges again.
    pub const TRUTHINESS: Self = Self(1 << 27);
    /// A tuple unpacked into `var` locals, from a place of the body or a
    /// sibling call's result: the element reads are synthesized from the
    /// tuple's type, which an instance substitutes and derives them from
    /// again.
    pub const TUPLE_UNPACKS: Self = Self(1 << 28);
    /// A method called with explicit compile-time arguments
    /// (`self.field.method[3](x)`) on a nominal receiver: the callee and the
    /// parameters it declares are selected from the receiver's own type,
    /// which no instance changes, and a per-call clone request it records
    /// must name no struct parameter.
    pub const PARAMETERIZED_CALLS: Self = Self(1 << 29);
    /// A list, set, or dict comprehension whose clauses iterate places the
    /// body borrows or sibling calls' results: each clause's protocol is an
    /// `ITERATION` loop's, and each binder is a local of the body declared
    /// from that protocol's binding plan, which an instance selects again.
    pub const COMPREHENSIONS: Self = Self(1 << 30);
    /// A `with` statement: the checker desugars it into ordinary statements
    /// whose shape its manager struct's declarations decide
    /// ([`WithForm`]), and an instance builds the same desugar again from
    /// its own syntax and that form.
    pub const WITH_STATEMENTS: Self = Self(1 << 31);

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
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

/// A compile-time value an instance holds as a literal where its template
/// read a value parameter or a `comptime for` variable by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedLiteral {
    pub occurrence: OccurrenceId,
    /// The literal's own type.
    pub ty: Ty,
    /// The type the literal materializes to where it stands for a runtime
    /// value: the template's recorded type for the name. A pack element's
    /// index is consumed by the fold and materializes to nothing.
    pub materialized: Option<Ty>,
    /// Whether the literal is a temporary lent to a read parameter, where
    /// the template lent the name's place.
    pub read_temporary: bool,
    /// Whether the literal is a temporary no one consumes: a read argument,
    /// or a `print` argument.
    pub unconsumed_temporary: bool,
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

/// A place below one of the declaration's own bindings, in template-local
/// terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePlace {
    pub root: TemplateOwner,
    pub path: Vec<mojito_types::origin::OriginSeg>,
}

/// An origin with each place it is rooted at in template-local terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateOrigin {
    Place(TemplatePlace),
    Union(Vec<Self>),
    /// An origin that names no binding identity, kept as written.
    Unrooted(mojito_types::origin::Origin),
}

/// The origin arguments a retained struct type carries in its argument
/// tails, in template-local terms, in the order a pre-order walk of the type
/// meets them (`TypedOrigins`).
///
/// A constructed iterator's type (`_ListIter[T, origin_of(self)]`) names the
/// receiver inside a type argument. The type is kept with every struct
/// origin slot unbound, and an instance writes its own bindings back into
/// those slots in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedOrigins {
    pub table: TypedTable,
    pub occurrence: OccurrenceId,
    pub origins: Vec<TemplateOrigin>,
}

/// One origin slot a view-returning call's result binds at the call, in
/// template-local terms ([`CheckedBodyFacts::call_result_origins`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCallResultOrigin {
    pub slot: mojito_types::origin::OriginParamId,
    pub origin: TemplateOrigin,
    /// The callee's capability over the place, when its contract fixes one.
    pub mutability: Option<mojito_types::origin::Mutability>,
}

/// One source of a replayed transfer: an origin in template-local terms, and
/// the template's type of the binding it is rooted at.
///
/// A binding whose type may carry loans has its own place as its origin, so
/// the replay records the place; a plain-data binding has none, so the
/// replay records nothing for it. An instance substitutes the type and keeps
/// or drops the source by that judgment. An unrooted origin has no binding
/// and is kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateTransferSource {
    pub origin: TemplateOrigin,
    pub root_ty: Option<Ty>,
}

/// One [`crate::checked::CheckedCallTransfer`] in template-local terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCallTransfer {
    pub dest: TemplateTransferDest,
    pub dest_path: Vec<mojito_types::origin::OriginSeg>,
    pub sources: Vec<TemplateTransferSource>,
    pub mutable: bool,
}

/// The actual a replayed transfer stores into. A concrete captured owner
/// (`CheckedTransferDest::Owner`) is a residue no template retains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateTransferDest {
    Receiver,
    Argument(usize),
}

/// One effect a replay derived on the body's own frame.
///
/// The effect names signature origins, so only the type of the parameter or
/// receiver its source names is instance-dependent; the template's type of
/// that binding is kept beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateTransferEffect {
    pub effect: mojito_types::types::TransferEffect,
    pub src_ty: Ty,
}

/// Which retained type table a [`TypedOrigins`] entry completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedTable {
    Expression,
    Place,
    Binding,
}

/// One [`mojito_types::origin::RefTy`] a call yielded, in template-local
/// terms.
///
/// An instance substitutes the referent and gets its own bindings back in the
/// origin. The mutability is read off the receiver's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateReference {
    pub referent: Ty,
    pub origin: TemplateOrigin,
    pub mutability: mojito_types::origin::Mutability,
}

/// The iterator protocol of one runtime `for`, in template-local terms
/// ([`CheckedBodyFacts::iterations`]).
///
/// The protocol's symbols, projection, and types follow from the iterable's
/// type, which an instance substitutes and selects the protocol from again.
/// The place the loop borrows (`None` for a temporary source) and whether
/// its binding is mutable are the loop's syntax and the declaration's, so an
/// instance takes them as they stand, rooted at its own binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateIteration {
    pub mode: crate::checked::IterationMode,
    pub binding: mojito_ast::ast::LoopBindingMode,
    pub iterable: Ty,
    pub source: Option<TemplatePlace>,
    pub source_mutable: bool,
}

/// One generator binder of a comprehension, in template-local terms
/// ([`CheckedBodyFacts::comprehension_bindings`]).
///
/// The binder's plan and type are its clause's iterator protocol's binding,
/// which an instance selects again at the clause's iterable, and whether its
/// storage is droppable follows from that type. The binder itself is a local
/// of the body, numbered as the instance's own check would mint it. `ty` is
/// the binding's type, a reference's referent, with struct origin slots
/// unbound: what the method grammar judges the local's kind by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateComprehensionBinding {
    pub name: String,
    pub owner: TemplateOwner,
    pub iterable: OccurrenceId,
    pub reference: bool,
    pub ty: Ty,
}

/// The shape of one `with` statement's desugar
/// ([`CheckedBodyFacts::with_forms`]).
///
/// The manager struct's declared `__enter__` and `__exit__` members and
/// whether the context may raise decide it, never a substituted type, so an
/// instance builds its desugar from its own syntax and the template's form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithForm {
    /// A consuming `__enter__` and no `__exit__`: the enter result stands in
    /// for the manager to the end of the block. `returns_none` is whether
    /// `__enter__` returns nothing, so an unnamed result is not bound.
    ConsumingEnter { returns_none: bool },
    /// No `__exit__`: the manager itself lives to the end of the block.
    KeptManager,
    /// `try: body finally: manager.__exit__()`.
    PlainExit,
    /// In a raising context, beside the plain `__exit__`, an
    /// `__exit__(self, err: Error) -> Bool` takes the body's error and
    /// decides whether it is raised again.
    ErrorExit,
}

/// The element reads of one tuple unpacking, in template-local terms
/// ([`CheckedBodyFacts::tuple_unpacks`]).
///
/// Each element's type and generated accessor follow from the unpacked
/// value's type, which an instance substitutes and derives them from again.
/// The reference a place yields (`None` for a temporary) roots the place
/// accessors' results, so an instance takes it rooted at its own binding.
/// The named targets are the statement's syntax: where it declares them,
/// they are locals of the body, which an instance numbers as its own check
/// would (`renumber_locals`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateTupleUnpack {
    pub value: Ty,
    pub source: Option<TemplateReference>,
    pub targets: Vec<OccurrenceId>,
    pub declares: bool,
}

/// An element store through a subscript, in template-local terms: through
/// the reference a getter yields, or read through a value getter and
/// written back through a setter.
///
/// The selected call at the site is the getter for a store through a
/// reference and the setter for a store through a setter, so only the
/// contracts it does not hold are kept beside the element's types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateAugmentedSubscript {
    pub operand_ty: Ty,
    pub result_ty: Ty,
    /// The value getter that reads the element a setter writes back. Absent
    /// for a store through a reference.
    pub getter: Option<TemplateCallContract>,
    /// The in-place dunder a struct element's augmented store selects.
    pub inplace: Option<TemplateCallContract>,
}

impl TemplateAugmentedSubscript {
    /// The contracts the store embeds beside the site's selected call.
    pub fn contracts_mut(&mut self) -> impl Iterator<Item = &mut TemplateCallContract> {
        self.getter.iter_mut().chain(&mut self.inplace)
    }
}

/// A binding a retained fact names, in template-local terms: checker owner
/// identities are per-run counters and mean nothing in another run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TemplateOwner {
    /// The declaration's runtime parameter at this index.
    Param(usize),
    /// A method's `self`.
    Receiver,
    /// The n-th local the body declares, in checking order. An instance
    /// counts each unrolled copy of a declaration as a local of its own.
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
    /// Every argument of a [`TemplateClass::MethodBody`] instance is plain
    /// data: it carries no loan, holds no reference, and mentions no callable.
    /// A clone check decides outward-store transfer effects, view-result
    /// borrows, and closure escapes on exactly those properties, and a
    /// template, whose parameter is symbolic, records none of them.
    PlainDataArguments,
    /// Every `^` transfer of a value whose type mentioned a parameter must be
    /// of a `Movable` type for the instance. A parameter is always movable
    /// while it is symbolic, and the demand only ever produces an error, so
    /// no fact a template retains carries it.
    Movable,
    /// Each binding of a bare parameter type is deletable, linear, or neither
    /// according to the instance's own type, as the declaration's check
    /// decides it; a linear temporary stays one only while its type is still
    /// a parameter.
    Deletability,
    /// A reference result is marked a copyable read where its referent is
    /// implicitly copyable at the instance's type, as inference marks a
    /// clone's. One the template marked must stay marked: a by-value read
    /// rests on it.
    ReferenceResultReads,
    /// Each transfer the template replayed is replayed again at the
    /// instance. The realized callee's summary is the one the template read,
    /// or empty; a source rooted at a binding whose substituted type is plain
    /// data vanishes, since such a binding has no origin, and a transfer, a
    /// merged origin, or an effect whose every source vanished is not
    /// recorded, as the instance's own check records none. The template's
    /// own reuse keeps every source. The escape verdict is monotone in the
    /// sources, so a template that passed it cannot fail at an instance.
    ReplayedTransfers,
    /// The constructor the template selected still binds every argument
    /// exactly at the instance, and the instance's `__init__` clone, where
    /// one exists, is that member's. A construction records no contract, so
    /// the instance repeats the selection from the constructed type, the
    /// arguments' recorded types, and the constructor's declaration.
    ConstructorSelection,
    /// A call-through residue names the callable parameter by slot and each
    /// argument by signature origin, so the instance republishes the
    /// template's verbatim. That holds only while no argument carries an
    /// origin (a loan-carrying binding would carry one in the template and
    /// not in a plain-data instance), which capture refuses, and while every
    /// retained type is loan-free, as a vanishing transfer demands. A residue
    /// the body read from a callee is owed again: the instance's realized
    /// callee must publish exactly the residue the template read.
    CallThroughResidue,
    /// Every implicit conversion the template selected is selected again from
    /// the instance's own types, and records what the template recorded. An
    /// `@implicit` constructor is chosen from the source and target types
    /// alone, so the instance repeats the choice and may name a different
    /// constructor; one that names none, or one that consumes its source,
    /// refuses. A nominal-string wrap names the literal constructor, which no
    /// instance changes, and `repr`'s argument must still be `Writable`.
    ImplicitConversions,
    /// Every argument of a `print` the grammar admitted is `Writable` at the
    /// instance's type. The builtin selects no callee and proved a pack
    /// element through the pack's bound; the element the unrolling fixed is
    /// proved again at its own type.
    PrintableArguments,
}

/// One implicit conversion in template-local terms: what the four conversion
/// tables recorded at one occurrence.
///
/// An instance re-decides the conversion from its own types rather than
/// inheriting this, so the retained entry says which decision to repeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateConversion {
    /// The lowered constructor the template selected.
    pub target: String,
    /// The converted-to type. `None` for a nominal-string wrap, which records
    /// the target alone.
    pub result: Option<Ty>,
    /// The error type of a raising constructor.
    pub raises: Option<Ty>,
    /// The loan mutability of a view constructor's `ref [origin]` parameter.
    pub source_borrow: Option<bool>,
}

/// One subscript's index shape, a slice kind per sliced index, and whether
/// its setter takes the assigned value by keyword.
pub type SubscriptDescriptors = (Vec<Option<mojito_types::types::SliceKind>>, bool);

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
    /// Receivers reached through a reference, which a method call borrows
    /// rather than reads: decided by what the receiver is, never by its type.
    pub borrowed_reference_receivers: Vec<OccurrenceId>,
    pub read_temporary_arguments: Vec<OccurrenceId>,
    /// The callables whose transfer and call-through summaries the body
    /// read, every one of them empty: a derivation holds only while they
    /// still are.
    pub effect_free_callees: Vec<String>,
    /// Those of them read where a callable's name stands as a value (a
    /// parameter forwarded, a declaration handed on) rather than at a call:
    /// the instance reads them under the same name.
    pub value_callees: Vec<String>,
    /// Calls of the built-in `len`, which an instance realizes against its
    /// concrete argument type.
    pub builtin_len_calls: Vec<OccurrenceId>,
    /// The contract of each method call. Only a [`closed_method_contract`]
    /// derives: an instance then realizes its target and substitutes its
    /// result type.
    pub selected_calls: Vec<(OccurrenceId, TemplateCallContract)>,
    /// Every generic-struct application the body reached as a constructor
    /// target or a method-call receiver, in checking order and before any
    /// filter. An instance records the substituted applications itself, which
    /// is what mints the instances only its body reaches. They are not keyed
    /// by occurrence, so a class that drops occurrences must hold none.
    pub struct_applications: Vec<(String, Vec<mojito_types::types::TyArg>)>,
    pub rebind_assertions: Vec<(OccurrenceId, RebindAssertion)>,
    /// Places copied at a consuming position. An instance owes the copy: its
    /// concrete type must be implicitly copyable.
    pub copy_place_value_uses: Vec<OccurrenceId>,
    /// Interior generations each mutation invalidates, below a body-local
    /// binding.
    pub interior_invalidations: Vec<(OccurrenceId, Vec<TemplateInvalidation>)>,
    pub unconsumed_temporaries: Vec<OccurrenceId>,
    /// Values an expression statement or a `_ =` assignment discards. The
    /// statement's syntax alone decides it, so an instance inherits the set.
    pub discarded_reference_results: Vec<OccurrenceId>,
    /// Calls of a struct's named `deinit self` method. A nominal receiver's
    /// struct declares the method whatever its arguments, so an instance
    /// inherits the set; a call through a bound asks the instance's struct
    /// again once it is realized.
    pub explicit_destroy_calls: Vec<OccurrenceId>,
    /// Calls whose callee consumes a place receiver the call copies first.
    /// Whether a call copies its receiver is decided by the receiver's
    /// syntax and the callee's convention, so an instance inherits the set
    /// and owes the copy: the receiver's type must be implicitly copyable.
    pub implicitly_copied_consuming_receivers: Vec<OccurrenceId>,
    /// Conditions tested through `Bool(x)` rather than read as a `Bool`.
    /// Whether one is marked is decided by its type, so an instance judges
    /// each marked condition again at its own type.
    pub truthiness_conditions: Vec<OccurrenceId>,
    /// Expressions kept as a reference handle rather than read through, and
    /// whether the handle is writable. Only the value of a `return` in a
    /// method that returns a reference derives: the declaration and the
    /// statement's syntax decide that entry, and it is never writable.
    pub reference_value_uses: Vec<(OccurrenceId, bool)>,
    pub deletable_bindings: Vec<OccurrenceId>,
    /// Bindings of a bare parameter type whose bounds do not prove
    /// `Deinitable`. An instance judges each binding again at its own type
    /// ([`TemplateObligation::Deletability`]).
    pub linear_bindings: Vec<OccurrenceId>,
    /// Call results of a bare parameter type the body owns and cannot
    /// destroy. An instance's type is never a parameter, so it keeps none.
    pub linear_temporaries: Vec<OccurrenceId>,
    /// The reference each reference-returning call yields: the
    /// [`SemanticAdjustment::ReferenceResult`] entries of the adjustment
    /// table, kept apart because their origins name bindings.
    pub reference_results: Vec<(OccurrenceId, TemplateReference)>,
    /// The element stores made through a subscript's mutable reference, or
    /// augmented through a value getter and a setter.
    ///
    /// `self.counts[i] += 1`, `self.grid[i] = v` on a struct with no setter,
    /// or `self.table[i] += 1` through a setter: the
    /// [`SemanticAdjustment::AugmentedSubscript`] entries of the adjustment
    /// table whose getter, or setter, is the call recorded at the same site,
    /// kept apart because the embedded contracts' boundaries name spans and
    /// bindings of one run. An instance takes the call it realized for the
    /// site, rebuilds the value getter and in-place dunder kept beside it,
    /// and substitutes the two types.
    pub augmented_subscripts: Vec<(OccurrenceId, TemplateAugmentedSubscript)>,
    /// The interior generation a subscript's reference belongs to.
    pub interior_references: Vec<(OccurrenceId, TemplatePlace)>,
    /// The type of each `ref` binding, keyed by its declaration: a reference
    /// whose origin names a binding, kept out of `binding_types` for that
    /// reason.
    pub reference_binding_types: Vec<(OccurrenceId, TemplateReference)>,
    /// The place type of each use of a `ref` binding, kept out of
    /// `expression_place_types` likewise.
    pub reference_place_types: Vec<(OccurrenceId, TemplateReference)>,
    /// Reference results whose referent is implicitly copyable. An instance
    /// judges each reference result again at its own referent
    /// ([`TemplateObligation::ReferenceResultReads`]).
    pub copyable_reference_result_reads: Vec<OccurrenceId>,
    /// The index shape of each subscript a store goes through, and whether
    /// its setter takes the value by keyword. The subscript's syntax and the
    /// setter's declaration decide both, so an instance inherits the entry.
    pub subscript_descriptors: Vec<(OccurrenceId, SubscriptDescriptors)>,
    /// The dtype and width of each `SIMD`, `Scalar`, or scalar-alias
    /// construction. Only a closed construction records one, and its
    /// dimensions are the same in every instance.
    pub simd_constructions: Vec<(OccurrenceId, (mojito_ast::ast::Dtype, i64))>,
    /// The compile-time parameters the selected method declares, at each
    /// call spelled `receiver.method[…](…)`. They are the callee's
    /// declaration, and a nominal receiver's method is selected alike under
    /// every instance, so an instance inherits the entry.
    pub parameterized_method_calls: Vec<(OccurrenceId, Vec<mojito_types::types::ParamDecl>)>,
    /// The owned-interior tags a view-returning method call's result is
    /// projected through (`s.strip()` carries `s.<bytes>`). They are the
    /// selected callee's declared return origin, and a nominal receiver's
    /// method is selected alike under every instance, so an instance
    /// inherits the entry.
    pub view_result_interiors: Vec<(OccurrenceId, Vec<String>)>,
    /// The iterator protocol of each runtime `for`, keyed by its iterable,
    /// which an instance selects again from the substituted iterable type.
    pub iterations: Vec<(OccurrenceId, TemplateIteration)>,
    /// The generator binders of each comprehension, keyed by the
    /// comprehension, in clause order.
    pub comprehension_bindings: Vec<(OccurrenceId, Vec<TemplateComprehensionBinding>)>,
    /// The desugar form of each `with` statement, keyed by the statement,
    /// from which an instance builds its own desugar.
    pub with_forms: Vec<(OccurrenceId, WithForm)>,
    /// The element reads of each tuple unpacking, keyed by the unpacked
    /// value, which an instance derives again from the substituted value
    /// type.
    pub tuple_unpacks: Vec<(OccurrenceId, TemplateTupleUnpack)>,
    /// Arguments a call keeps as the caller's place, for a `mut` or `ref`
    /// parameter. The callee's declared convention decides it, so an instance
    /// inherits the set.
    pub call_place_uses: Vec<OccurrenceId>,
    /// Every `^` transfer, from the syntax alone. An instance owes `Movable`
    /// at each one whose type mentioned a parameter
    /// ([`TemplateObligation::Movable`]).
    pub transfers: Vec<OccurrenceId>,
    /// The transfers each call replayed from its callee's summary, in
    /// template-local terms: which actual receives loans rooted at which of
    /// the body's own places. An instance replays each again
    /// ([`TemplateObligation::ReplayedTransfers`]).
    pub call_transfers: Vec<(OccurrenceId, Vec<TemplateCallTransfer>)>,
    /// The origins those replays merged into each destination binding's
    /// bookkeeping, by template owner.
    pub transferred_origins: Vec<(TemplateOwner, Vec<TemplateTransferSource>)>,
    /// The effects the replays derived on the body's own frame, which the
    /// frame publishes under the body's key.
    pub transfer_effects: Vec<TemplateTransferEffect>,
    /// Each callee whose transfer summary the body replayed, with what it
    /// read. An instance rekeys each to its realized callee, whose summary
    /// must be the same or empty.
    pub transfer_reads: Vec<(String, Vec<mojito_types::types::TransferEffect>)>,
    /// Operators over two places of one parameter-typed type, at which the
    /// template recorded nothing: a comparison, or an arithmetic, bitwise, or
    /// shift operator its bound proves. An instance dispatches each on its own
    /// type and records the dunder it selects, with the copy, conversion, and
    /// adjustment that dispatch carries.
    pub operators: Vec<OccurrenceId>,
    /// Calls of a checker builtin on a bounded parameter (`hasher.update(x)`,
    /// `writer.write(x)`), which select no callee. The template proved each
    /// argument through the bound; an instance owes the same proof at its own
    /// type, which for a hashed value also records the hash leaf.
    pub bound_builtins: Vec<(OccurrenceId, BoundBuiltin)>,
    /// Per-call clone requests. A template never records one that survives
    /// realization (`realize_method_call` refuses a callee with binders of
    /// its own); a bound dispatch whose witness declares binders adds one,
    /// with the binders bound to the caller's own symbolic types, which
    /// discovery leaves alone.
    pub method_instantiations: Vec<(OccurrenceId, MethodInstantiation)>,
    /// Struct constructions the grammar admitted, each with an empty
    /// immutable-binder record. A construction records no contract; an
    /// instance re-selects each constructor's clone from the constructed type
    /// ([`TemplateObligation::ConstructorSelection`]).
    pub constructions: Vec<OccurrenceId>,
    /// Calls through a runtime parameter of `def(...)` type, which the grammar
    /// admitted. Such a call records the parameter's own contract symbol and
    /// parameters, in the caller's binder scope; an instance takes both from
    /// its own parameter binding.
    pub callable_calls: Vec<OccurrenceId>,
    /// The call-through residue the body published on its own frame, by
    /// calling its callable parameter or forwarding it. It names slots and
    /// signature origins only, so an instance republishes it verbatim
    /// ([`TemplateObligation::CallThroughResidue`]).
    pub call_throughs: Vec<CallThroughEffect>,
    /// Each callee whose call-through summary the body read, with what it
    /// read. An instance rekeys each to its realized callee, which must
    /// publish the same residue.
    pub call_through_reads: Vec<(String, Vec<CallThroughEffect>)>,
    /// Calls of the checker builtin `repr`, which the grammar admitted. The
    /// call selects no callee and wraps its compile-time string result as the
    /// nominal `String`; an instance owes only that its argument is still
    /// `Writable` ([`TemplateObligation::ImplicitConversions`]).
    pub repr_calls: Vec<OccurrenceId>,
    /// Calls of the built-in `print`, which the grammar admitted. The call
    /// selects no callee; an instance owes that each argument is still
    /// `Writable` ([`TemplateObligation::PrintableArguments`]).
    pub print_calls: Vec<OccurrenceId>,
    /// The implicit conversion selected at each occurrence that records one.
    /// An instance selects it again from its own types, so a clone whose
    /// source type changed names a different constructor.
    pub conversions: Vec<(OccurrenceId, TemplateConversion)>,
    /// The origins of every retained struct type that names a binding in an
    /// origin argument, kept by template owner while the type itself keeps
    /// those slots unbound.
    pub typed_origins: Vec<TypedOrigins>,
    /// The origin slots each view-returning call's contract resolved at the
    /// call (`self.items()` over `-> View[origin_of(self)]` binds the view's
    /// slot to the receiver), kept by template owner. The contract and the
    /// places it names are the callee's declaration and the call's own
    /// receiver and arguments, which no instance changes.
    pub call_result_origins: Vec<(OccurrenceId, Vec<TemplateCallResultOrigin>)>,
    /// How many locals the body declares.
    pub locals: u32,
}

/// Which checker builtin a [`CheckedBodyFacts::bound_builtins`] entry calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundBuiltin {
    /// `hasher.update(value)`: the value must be `Hashable`.
    Update,
    /// `hasher._update_with_simd(value)`: the value must be a vector.
    UpdateSimd,
    /// `writer.write(values…)`: every value must be writable.
    Write,
    /// `hasher^.finish()` on a `^` transfer of a place of the method's own
    /// `Hasher` binder: it takes no argument and yields a `UInt64` under
    /// every instance.
    Finish,
}

/// One field of a derived bundle beside the inferred one, when they differ:
/// what verification mode reports.
/// Compare each named field of a derived and an inferred bundle with
/// [`differing`], under the field's own name.
macro_rules! differing_fields {
    ($out:expr, $derived:expr, $inferred:expr; $($field:ident),+ $(,)?) => {
        $(differing($out, stringify!($field), &$derived.$field, &$inferred.$field);)+
    };
}

fn differing<T: std::fmt::Debug + PartialEq>(
    out: &mut String,
    name: &str,
    derived: &T,
    inferred: &T,
) {
    use std::fmt::Write as _;
    if derived != inferred {
        let _ = writeln!(
            out,
            " {name}:\n  derived:  {derived:?}\n  inferred: {inferred:?}"
        );
    }
}

impl CheckedBodyFacts {
    /// Whether the body replayed a callee's transfer summary: it recorded a
    /// call transfer, merged a transferred origin, published an effect on its
    /// own frame, or read a summary that held one.
    pub const fn replays_transfers(&self) -> bool {
        !self.call_transfers.is_empty()
            || !self.transferred_origins.is_empty()
            || !self.transfer_effects.is_empty()
            || !self.transfer_reads.is_empty()
    }

    /// The fields in which this (derived) bundle differs from an `other`
    /// (inferred) one, each with both values: what verification mode reports.
    pub fn difference(&self, other: &Self) -> String {
        let mut out = String::new();
        differing_fields!(
            &mut out, self, other;
            occurrences,
            expression_types,
            expression_place_types,
            binding_types,
            expression_bindings,
            statement_bindings,
            expression_effects,
            operation_adjustments,
            generic_instantiations,
            overload_targets,
            call_parameters,
            borrowed_reference_receivers,
            borrowed_read_call_places,
            read_temporary_arguments,
            effect_free_callees,
            builtin_len_calls,
            selected_calls,
            struct_applications,
            rebind_assertions,
            copy_place_value_uses,
            interior_invalidations,
            unconsumed_temporaries,
            discarded_reference_results,
            explicit_destroy_calls,
            implicitly_copied_consuming_receivers,
            truthiness_conditions,
            reference_value_uses,
            deletable_bindings,
            linear_bindings,
            linear_temporaries,
            reference_results,
            augmented_subscripts,
            interior_references,
            reference_binding_types,
            reference_place_types,
            copyable_reference_result_reads,
            subscript_descriptors,
            simd_constructions,
            parameterized_method_calls,
            view_result_interiors,
            iterations,
            comprehension_bindings,
            with_forms,
            tuple_unpacks,
            call_place_uses,
            transfers,
            operators,
        );
        self.transfer_differences(other, &mut out);
        differing_fields!(
            &mut out, self, other;
            bound_builtins,
            method_instantiations,
            constructions,
            value_callees,
            callable_calls,
            call_throughs,
            call_through_reads,
            repr_calls,
            print_calls,
            conversions,
            typed_origins,
            call_result_origins,
            locals,
        );
        out
    }

    /// A template's facts laid out over an instance's occurrences, in the
    /// instance's pre-order.
    ///
    /// Each instance occurrence takes the facts of the template occurrence
    /// whose identity it kept. An occurrence the elaborator dropped — an
    /// untaken arm, a loop that ran zero times — takes its facts, requests,
    /// and effect reads with it; one it copied per loop iteration carries
    /// them once per copy. A `folded` occurrence — a compile-time value the
    /// elaborator wrote as the instance's literal — keeps its identity and
    /// none of the value's facts: a literal records its own type and, where
    /// it stands for a runtime value, its materialization.
    #[must_use]
    pub fn selected(&self, occurrences: &[OccurrenceId], literals: &[FoldedLiteral]) -> Self {
        fn at<V: Clone>(
            table: &[(OccurrenceId, V)],
            occurrences: &[OccurrenceId],
            folded: &[OccurrenceId],
        ) -> Vec<(OccurrenceId, V)> {
            occurrences
                .iter()
                .filter(|occurrence| !folded.contains(occurrence))
                .filter_map(|occurrence| {
                    table
                        .iter()
                        .find(|(id, _)| id.syntax == occurrence.syntax)
                        .map(|(_, fact)| (*occurrence, fact.clone()))
                })
                .collect()
        }
        let folded: Vec<OccurrenceId> = literals.iter().map(|literal| literal.occurrence).collect();
        let folded = folded.as_slice();
        let literal = |occurrence: &OccurrenceId| {
            literals
                .iter()
                .find(|literal| literal.occurrence == *occurrence)
        };
        let flagged_except = |table: &[OccurrenceId], folded: &[OccurrenceId]| {
            occurrences
                .iter()
                .copied()
                .filter(|occurrence| {
                    !folded.contains(occurrence)
                        && table.iter().any(|id| id.syntax == occurrence.syntax)
                })
                .collect::<Vec<_>>()
        };
        let flagged = |table: &[OccurrenceId]| flagged_except(table, folded);
        let flagged_or = |table: &[OccurrenceId], held: fn(&FoldedLiteral) -> bool| {
            occurrences
                .iter()
                .copied()
                .filter(|occurrence| match literal(occurrence) {
                    Some(literal) => held(literal),
                    None => table.iter().any(|id| id.syntax == occurrence.syntax),
                })
                .collect::<Vec<_>>()
        };
        Self {
            occurrences: occurrences.to_vec(),
            expression_types: occurrences
                .iter()
                .filter_map(|occurrence| {
                    if let Some(literal) = literal(occurrence) {
                        return Some((*occurrence, literal.ty.clone()));
                    }
                    self.expression_types
                        .iter()
                        .find(|(id, _)| id.syntax == occurrence.syntax)
                        .map(|(_, ty)| (*occurrence, ty.clone()))
                })
                .collect(),
            expression_place_types: at(&self.expression_place_types, occurrences, folded),
            // A local's facts, and a store's invalidations, are keyed at the
            // value stored, which a fold leaves the statement's.
            binding_types: at(&self.binding_types, occurrences, &[]),
            expression_bindings: at(&self.expression_bindings, occurrences, folded),
            statement_bindings: at(&self.statement_bindings, occurrences, folded),
            expression_effects: at(&self.expression_effects, occurrences, folded),
            operation_adjustments: occurrences
                .iter()
                .filter_map(|occurrence| match literal(occurrence) {
                    Some(literal) => literal.materialized.clone().map(|target| {
                        (*occurrence, SemanticAdjustment::MaterializeLiteral(target))
                    }),
                    None => self
                        .operation_adjustments
                        .iter()
                        .find(|(id, _)| id.syntax == occurrence.syntax)
                        .map(|(_, fact)| (*occurrence, fact.clone())),
                })
                .collect(),
            generic_instantiations: at(&self.generic_instantiations, occurrences, folded),
            overload_targets: at(&self.overload_targets, occurrences, folded),
            call_parameters: at(&self.call_parameters, occurrences, folded),
            borrowed_read_call_places: flagged(&self.borrowed_read_call_places),
            borrowed_reference_receivers: flagged(&self.borrowed_reference_receivers),
            read_temporary_arguments: flagged_or(&self.read_temporary_arguments, |literal| {
                literal.read_temporary
            }),
            // Realization recomputes these from the calls that remain and
            // the value reads, which name no occurrence.
            effect_free_callees: Vec::new(),
            value_callees: self.value_callees.clone(),
            builtin_len_calls: flagged(&self.builtin_len_calls),
            // A call and its arguments are copied together.
            selected_calls: at(&self.selected_calls, occurrences, folded)
                .into_iter()
                .map(|(id, mut call)| {
                    for argument in &mut call.arguments {
                        argument.value.copy = id.copy;
                    }
                    (id, call)
                })
                .collect(),
            struct_applications: self.struct_applications.clone(),
            rebind_assertions: at(&self.rebind_assertions, occurrences, folded),
            copy_place_value_uses: flagged(&self.copy_place_value_uses),
            interior_invalidations: at(&self.interior_invalidations, occurrences, &[]),
            unconsumed_temporaries: flagged_or(&self.unconsumed_temporaries, |literal| {
                literal.unconsumed_temporary
            }),
            discarded_reference_results: flagged(&self.discarded_reference_results),
            explicit_destroy_calls: flagged(&self.explicit_destroy_calls),
            implicitly_copied_consuming_receivers: flagged(
                &self.implicitly_copied_consuming_receivers,
            ),
            truthiness_conditions: flagged(&self.truthiness_conditions),
            reference_value_uses: at(&self.reference_value_uses, occurrences, folded),
            deletable_bindings: flagged_except(&self.deletable_bindings, &[]),
            linear_bindings: flagged_except(&self.linear_bindings, &[]),
            linear_temporaries: flagged(&self.linear_temporaries),
            reference_results: at(&self.reference_results, occurrences, folded),
            augmented_subscripts: at(&self.augmented_subscripts, occurrences, folded)
                .into_iter()
                .map(|(id, mut store)| {
                    for call in store.contracts_mut() {
                        for argument in &mut call.arguments {
                            argument.value.copy = id.copy;
                        }
                    }
                    (id, store)
                })
                .collect(),
            interior_references: at(&self.interior_references, occurrences, folded),
            reference_binding_types: at(&self.reference_binding_types, occurrences, folded),
            reference_place_types: at(&self.reference_place_types, occurrences, folded),
            copyable_reference_result_reads: flagged(&self.copyable_reference_result_reads),
            subscript_descriptors: at(&self.subscript_descriptors, occurrences, folded),
            simd_constructions: at(&self.simd_constructions, occurrences, folded),
            parameterized_method_calls: at(&self.parameterized_method_calls, occurrences, folded),
            view_result_interiors: at(&self.view_result_interiors, occurrences, folded),
            iterations: at(&self.iterations, occurrences, folded),
            comprehension_bindings: at(&self.comprehension_bindings, occurrences, folded),
            with_forms: at(&self.with_forms, occurrences, folded),
            tuple_unpacks: at(&self.tuple_unpacks, occurrences, folded),
            call_place_uses: flagged(&self.call_place_uses),
            transfers: flagged(&self.transfers),
            call_transfers: at(&self.call_transfers, occurrences, folded),
            // Merged origins, effects, and reads name bindings and callees,
            // not occurrences.
            transferred_origins: self.transferred_origins.clone(),
            transfer_effects: self.transfer_effects.clone(),
            transfer_reads: self.transfer_reads.clone(),
            operators: flagged(&self.operators),
            bound_builtins: at(&self.bound_builtins, occurrences, folded),
            method_instantiations: at(&self.method_instantiations, occurrences, folded),
            constructions: flagged(&self.constructions),
            callable_calls: flagged(&self.callable_calls),
            // A residue names slots, not occurrences; realization rekeys each
            // read to the instance's callee and refuses one no call names.
            call_throughs: self.call_throughs.clone(),
            call_through_reads: self.call_through_reads.clone(),
            repr_calls: flagged(&self.repr_calls),
            print_calls: flagged(&self.print_calls),
            conversions: at(&self.conversions, occurrences, folded),
            // Table by table, as the capture keeps them.
            typed_origins: [
                TypedTable::Expression,
                TypedTable::Place,
                TypedTable::Binding,
            ]
            .into_iter()
            .flat_map(|table| {
                occurrences
                    .iter()
                    .filter(|occurrence| !folded.contains(occurrence))
                    .flat_map(move |occurrence| {
                        self.typed_origins
                            .iter()
                            .filter(move |typed| {
                                typed.table == table && typed.occurrence.syntax == occurrence.syntax
                            })
                            .map(|typed| TypedOrigins {
                                occurrence: *occurrence,
                                ..typed.clone()
                            })
                    })
            })
            .collect(),
            call_result_origins: at(&self.call_result_origins, occurrences, folded),
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
            + self.borrowed_reference_receivers.len()
            + self.read_temporary_arguments.len()
            + self.effect_free_callees.len()
            + self.builtin_len_calls.len()
            + self.selected_calls.len()
            + self.struct_applications.len()
            + self.rebind_assertions.len()
            + self.copy_place_value_uses.len()
            + self.interior_invalidations.len()
            + self.unconsumed_temporaries.len()
            + self.discarded_reference_results.len()
            + self.explicit_destroy_calls.len()
            + self.implicitly_copied_consuming_receivers.len()
            + self.truthiness_conditions.len()
            + self.reference_value_uses.len()
            + self.deletable_bindings.len()
            + self.linear_bindings.len()
            + self.linear_temporaries.len()
            + self.reference_results.len()
            + self.augmented_subscripts.len()
            + self.interior_references.len()
            + self.reference_binding_types.len()
            + self.reference_place_types.len()
            + self.copyable_reference_result_reads.len()
            + self.subscript_descriptors.len()
            + self.simd_constructions.len()
            + self.parameterized_method_calls.len()
            + self.view_result_interiors.len()
            + self.iterations.len()
            + self.comprehension_bindings.len()
            + self.with_forms.len()
            + self.tuple_unpacks.len()
            + self.call_place_uses.len()
            + self.transfers.len()
            + self.bound_builtins.len()
            + self.method_instantiations.len()
            + self.constructions.len()
            + self.typed_origins.len()
            + self.call_result_origins.len()
            + self.call_transfers.len()
            + self.transferred_origins.len()
            + self.transfer_effects.len()
            + self.transfer_reads.len()
    }

    /// The replayed-transfer fields in which this bundle differs from
    /// `other`, appended to `out` as [`Self::difference`] appends the rest.
    fn transfer_differences(&self, other: &Self, out: &mut String) {
        differing(
            out,
            "call_transfers",
            &self.call_transfers,
            &other.call_transfers,
        );
        differing(
            out,
            "transferred_origins",
            &self.transferred_origins,
            &other.transferred_origins,
        );
        differing(
            out,
            "transfer_effects",
            &self.transfer_effects,
            &other.transfer_effects,
        );
        differing(
            out,
            "transfer_reads",
            &self.transfer_reads,
            &other.transfer_reads,
        );
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
    /// The type packs the clone no longer declares, each with the element
    /// types the elaborator wrote in its signature.
    pub pack_bindings: Vec<(String, Vec<mojito_ast::ast::Type>)>,
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
    templates: std::collections::HashMap<TemplateId, CheckedTemplate>,
    traces: std::collections::HashMap<InstanceName, InstanceTrace>,
    generated: GeneratedNames,
    /// Set when a source validation run ended without a verdict: nothing
    /// that run recorded may be certified.
    validation_aborted: bool,
    /// Compare each derived bundle with the clone check's own facts.
    verify: bool,
    stats: TemplateStats,
    /// The compilation's parameter-expression context. The catalog is what
    /// already travels through source validation and every discovery round,
    /// so the context travels with it: one per compilation, shared by each
    /// checker run. A defaulted catalog holds a detached context.
    param_context: mojito_types::param_expr::ParamContext,
}

/// The declarations the elaboration being checked generated, as the
/// elaborator reported them.
///
/// A generated declaration is never a template. This list is the only test:
/// a `$` in a name proves nothing, since a module-qualified source name
/// (`__module$std$string$String`) carries one too. A per-instantiation method
/// clone is recognized by its explicit receiver type and is not listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedNames {
    pub defs: std::collections::HashSet<String>,
    /// Structs specialized whole, every member of which is generated.
    pub structs: std::collections::HashSet<String>,
    /// Per-call method clones, as (owner, clone name).
    pub methods: std::collections::HashSet<(String, String)>,
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
    /// Traced clones a derivation refused, each with the reason.
    pub refused: Vec<(String, String)>,
    /// Pack-keyed bodies source validation left to the per-instantiation
    /// check, each with the use of the unbound pack it has no rule for.
    pub no_verdict: Vec<(String, String)>,
}

/// An elaborated clone's declaration identity.
///
/// A `def` clone is the module tag the elaborator stamped it with and its
/// output name. A method clone also names its struct, and the byte range of
/// its body's first statement: same-name overloads clone under one name and
/// one tag, and `Method` carries no range of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceName {
    pub module: Option<String>,
    pub owner: Option<String>,
    pub name: String,
    pub body: Option<Span>,
}

impl TemplateCatalog {
    pub fn new(verify: bool) -> Self {
        Self {
            verify,
            param_context: mojito_types::param_expr::ParamContext::new(),
            ..Self::default()
        }
    }

    pub const fn param_context(&self) -> &mojito_types::param_expr::ParamContext {
        &self.param_context
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
        for template in self.templates.values_mut() {
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
        self.templates.insert(template.id.clone(), template);
    }

    pub fn template(&self, id: &TemplateId) -> Option<&CheckedTemplate> {
        self.templates.get(id)
    }

    pub fn templates(&self) -> impl Iterator<Item = &CheckedTemplate> {
        self.templates.values()
    }

    /// Replace the clone traces with those of the elaboration about to be
    /// checked. Traces never outlive their elaboration: the next round names
    /// its own clones.
    pub fn set_traces(&mut self, traces: Vec<(InstanceName, InstanceTrace)>) {
        self.traces = traces.into_iter().collect();
    }

    /// Replace the generated-declaration list with that of the elaboration
    /// about to be checked.
    pub fn set_generated(&mut self, generated: GeneratedNames) {
        self.generated = generated;
    }

    pub fn generated_def(&self, name: &str) -> bool {
        self.generated.defs.contains(name)
    }

    /// Whether `owner.method` is generated: a member of a struct specialized
    /// whole, or a per-call clone.
    pub fn generated_method(&self, owner: &str, method: &str) -> bool {
        self.generated.structs.contains(owner)
            || self
                .generated
                .methods
                .contains(&(owner.to_string(), method.to_string()))
    }

    pub fn trace(&self, instance: &InstanceName) -> Option<&InstanceTrace> {
        self.traces.get(instance)
    }
}
