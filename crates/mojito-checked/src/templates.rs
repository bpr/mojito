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
    CheckedCallArgumentSource, CheckedCallBoundary, CheckedCallContract,
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
        SemanticAdjustment::InvertedReprWrite => Some(SemanticAdjustment::InvertedReprWrite),
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
    call.reference_result.is_none() && closed_contract(call, false, false)
}

/// Whether a method call's contract is a [`closed_method_contract`] but for
/// the types its by-value parameters have.
///
/// An argument bound by value to a parameter of any type is still supplied,
/// still bound without a place, and still unadapted: no conversion, no
/// materialization, and no invalidation sits at its boundary. So its type
/// equals the parameter's, which substitution preserves, and an instance
/// changes the parameter's type alone. What the argument's own expression
/// owes (a copy, a move, a temporary) is recorded at that expression and is
/// for the body's grammar to admit.
pub fn value_method_contract(call: &TemplateCallContract) -> bool {
    call.reference_result.is_none() && closed_contract(call, false, true)
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
        && closed_contract(call, true, false)
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

/// What [`closed_method_contract`], [`value_method_contract`], and
/// [`closed_reference_contract`] share. `values` admits a by-value parameter
/// of any type, which then takes no adjustment at all.
fn closed_contract(call: &TemplateCallContract, reference: bool, values: bool) -> bool {
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
        ) || (reference && *receiver_convention == Some(ArgConvention::Ref)))
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
            if opaque && !argument.adjustments.is_empty() {
                return false;
            }
            argument.invalidations.is_empty()
                && argument.adjustments.iter().all(|adjustment| {
                    matches!(adjustment, CheckedCallValueAdjustment::MaterializeLiteral { target }
                        if closed_scalar(target))
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
    /// the element a reference getter yields, or a scalar element a closed
    /// setter takes.
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
    /// summary there.
    pub const VANISHING_TRANSFERS: Self = Self(1 << 12);
    /// A comparison of two places of one type that mentions a struct
    /// parameter, which an instance dispatches on its own type.
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
    /// clone keeps and binds symbolically as the template does.
    pub const BOUND_BINDERS: Self = Self(1 << 16);
    /// A construction of a declared struct whose compile-time arguments are
    /// types, passing closed scalars, whole values, or `copy:` of a named
    /// place; an instance re-selects the constructor's clone on its own
    /// arguments.
    pub const CONSTRUCTIONS: Self = Self(1 << 17);

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

/// A binding a retained fact names, in template-local terms: checker owner
/// identities are per-run counters and mean nothing in another run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TemplateOwner {
    /// The declaration's runtime parameter at this index.
    Param(usize),
    /// A method's `self`.
    Receiver,
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
    /// Every value in a body that replayed a transfer summary is plain data at
    /// the instance's types. A transfer moves the loans its source carries,
    /// and a plain-data value carries none, so the instance's check records
    /// no transfer, merges no origin, and publishes no effect of its own. A
    /// template's own body, whose parameter is still symbolic, never meets
    /// this and is inferred again.
    PlainDataTransfers,
    /// The constructor the template selected still binds every argument
    /// exactly at the instance, and the instance's `__init__` clone, where
    /// one exists, is that member's. A construction records no contract, so
    /// the instance repeats the selection from the constructed type, the
    /// arguments' recorded types, and the constructor's declaration.
    ConstructorSelection,
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
    /// Arguments a call keeps as the caller's place, for a `mut` or `ref`
    /// parameter. The callee's declared convention decides it, so an instance
    /// inherits the set.
    pub call_place_uses: Vec<OccurrenceId>,
    /// Every `^` transfer, from the syntax alone. An instance owes `Movable`
    /// at each one whose type mentioned a parameter
    /// ([`TemplateObligation::Movable`]).
    pub transfers: Vec<OccurrenceId>,
    /// Whether the body replayed a callee's transfer summary: it recorded a
    /// call transfer, merged a transferred origin, or published an effect on
    /// its own frame. None of that is retained, because none of it exists for
    /// an instance ([`TemplateObligation::PlainDataTransfers`]).
    pub vanishing_transfers: bool,
    /// Comparisons over two places of one parameter-typed type, at which the
    /// template recorded nothing. An instance dispatches each on its own
    /// type and records the dunder it selects.
    pub comparisons: Vec<OccurrenceId>,
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
    /// The origins of every retained struct type that names a binding in an
    /// origin argument, kept by template owner while the type itself keeps
    /// those slots unbound.
    pub typed_origins: Vec<TypedOrigins>,
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
}

impl CheckedBodyFacts {
    /// The fields in which this (derived) bundle differs from an `other`
    /// (inferred) one, each with both values: what verification mode reports.
    pub fn difference(&self, other: &Self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        if self.occurrences != other.occurrences {
            let _ = writeln!(
                out,
                " occurrences:\n  derived:  {:?}\n  inferred: {:?}",
                self.occurrences, other.occurrences
            );
        }
        if self.expression_types != other.expression_types {
            let _ = writeln!(
                out,
                " expression_types:\n  derived:  {:?}\n  inferred: {:?}",
                self.expression_types, other.expression_types
            );
        }
        if self.expression_place_types != other.expression_place_types {
            let _ = writeln!(
                out,
                " expression_place_types:\n  derived:  {:?}\n  inferred: {:?}",
                self.expression_place_types, other.expression_place_types
            );
        }
        if self.binding_types != other.binding_types {
            let _ = writeln!(
                out,
                " binding_types:\n  derived:  {:?}\n  inferred: {:?}",
                self.binding_types, other.binding_types
            );
        }
        if self.expression_bindings != other.expression_bindings {
            let _ = writeln!(
                out,
                " expression_bindings:\n  derived:  {:?}\n  inferred: {:?}",
                self.expression_bindings, other.expression_bindings
            );
        }
        if self.statement_bindings != other.statement_bindings {
            let _ = writeln!(
                out,
                " statement_bindings:\n  derived:  {:?}\n  inferred: {:?}",
                self.statement_bindings, other.statement_bindings
            );
        }
        if self.expression_effects != other.expression_effects {
            let _ = writeln!(
                out,
                " expression_effects:\n  derived:  {:?}\n  inferred: {:?}",
                self.expression_effects, other.expression_effects
            );
        }
        if self.operation_adjustments != other.operation_adjustments {
            let _ = writeln!(
                out,
                " operation_adjustments:\n  derived:  {:?}\n  inferred: {:?}",
                self.operation_adjustments, other.operation_adjustments
            );
        }
        if self.generic_instantiations != other.generic_instantiations {
            let _ = writeln!(
                out,
                " generic_instantiations:\n  derived:  {:?}\n  inferred: {:?}",
                self.generic_instantiations, other.generic_instantiations
            );
        }
        if self.overload_targets != other.overload_targets {
            let _ = writeln!(
                out,
                " overload_targets:\n  derived:  {:?}\n  inferred: {:?}",
                self.overload_targets, other.overload_targets
            );
        }
        if self.call_parameters != other.call_parameters {
            let _ = writeln!(
                out,
                " call_parameters:\n  derived:  {:?}\n  inferred: {:?}",
                self.call_parameters, other.call_parameters
            );
        }
        if self.borrowed_reference_receivers != other.borrowed_reference_receivers {
            let _ = writeln!(
                out,
                " borrowed_reference_receivers:\n  derived:  {:?}\n  inferred: {:?}",
                self.borrowed_reference_receivers, other.borrowed_reference_receivers
            );
        }
        if self.borrowed_read_call_places != other.borrowed_read_call_places {
            let _ = writeln!(
                out,
                " borrowed_read_call_places:\n  derived:  {:?}\n  inferred: {:?}",
                self.borrowed_read_call_places, other.borrowed_read_call_places
            );
        }
        if self.read_temporary_arguments != other.read_temporary_arguments {
            let _ = writeln!(
                out,
                " read_temporary_arguments:\n  derived:  {:?}\n  inferred: {:?}",
                self.read_temporary_arguments, other.read_temporary_arguments
            );
        }
        if self.effect_free_callees != other.effect_free_callees {
            let _ = writeln!(
                out,
                " effect_free_callees:\n  derived:  {:?}\n  inferred: {:?}",
                self.effect_free_callees, other.effect_free_callees
            );
        }
        if self.builtin_len_calls != other.builtin_len_calls {
            let _ = writeln!(
                out,
                " builtin_len_calls:\n  derived:  {:?}\n  inferred: {:?}",
                self.builtin_len_calls, other.builtin_len_calls
            );
        }
        if self.selected_calls != other.selected_calls {
            let _ = writeln!(
                out,
                " selected_calls:\n  derived:  {:?}\n  inferred: {:?}",
                self.selected_calls, other.selected_calls
            );
        }
        if self.struct_applications != other.struct_applications {
            let _ = writeln!(
                out,
                " struct_applications:\n  derived:  {:?}\n  inferred: {:?}",
                self.struct_applications, other.struct_applications
            );
        }
        if self.rebind_assertions != other.rebind_assertions {
            let _ = writeln!(
                out,
                " rebind_assertions:\n  derived:  {:?}\n  inferred: {:?}",
                self.rebind_assertions, other.rebind_assertions
            );
        }
        if self.copy_place_value_uses != other.copy_place_value_uses {
            let _ = writeln!(
                out,
                " copy_place_value_uses:\n  derived:  {:?}\n  inferred: {:?}",
                self.copy_place_value_uses, other.copy_place_value_uses
            );
        }
        if self.interior_invalidations != other.interior_invalidations {
            let _ = writeln!(
                out,
                " interior_invalidations:\n  derived:  {:?}\n  inferred: {:?}",
                self.interior_invalidations, other.interior_invalidations
            );
        }
        if self.unconsumed_temporaries != other.unconsumed_temporaries {
            let _ = writeln!(
                out,
                " unconsumed_temporaries:\n  derived:  {:?}\n  inferred: {:?}",
                self.unconsumed_temporaries, other.unconsumed_temporaries
            );
        }
        if self.discarded_reference_results != other.discarded_reference_results {
            let _ = writeln!(
                out,
                " discarded_reference_results:\n  derived:  {:?}\n  inferred: {:?}",
                self.discarded_reference_results, other.discarded_reference_results
            );
        }
        if self.reference_value_uses != other.reference_value_uses {
            let _ = writeln!(
                out,
                " reference_value_uses:\n  derived:  {:?}\n  inferred: {:?}",
                self.reference_value_uses, other.reference_value_uses
            );
        }
        if self.deletable_bindings != other.deletable_bindings {
            let _ = writeln!(
                out,
                " deletable_bindings:\n  derived:  {:?}\n  inferred: {:?}",
                self.deletable_bindings, other.deletable_bindings
            );
        }
        if self.linear_bindings != other.linear_bindings {
            let _ = writeln!(
                out,
                " linear_bindings:\n  derived:  {:?}\n  inferred: {:?}",
                self.linear_bindings, other.linear_bindings
            );
        }
        if self.linear_temporaries != other.linear_temporaries {
            let _ = writeln!(
                out,
                " linear_temporaries:\n  derived:  {:?}\n  inferred: {:?}",
                self.linear_temporaries, other.linear_temporaries
            );
        }
        if self.reference_results != other.reference_results {
            let _ = writeln!(
                out,
                " reference_results:\n  derived:  {:?}\n  inferred: {:?}",
                self.reference_results, other.reference_results
            );
        }
        if self.interior_references != other.interior_references {
            let _ = writeln!(
                out,
                " interior_references:\n  derived:  {:?}\n  inferred: {:?}",
                self.interior_references, other.interior_references
            );
        }
        if self.reference_binding_types != other.reference_binding_types {
            let _ = writeln!(
                out,
                " reference_binding_types:\n  derived:  {:?}\n  inferred: {:?}",
                self.reference_binding_types, other.reference_binding_types
            );
        }
        if self.reference_place_types != other.reference_place_types {
            let _ = writeln!(
                out,
                " reference_place_types:\n  derived:  {:?}\n  inferred: {:?}",
                self.reference_place_types, other.reference_place_types
            );
        }
        if self.copyable_reference_result_reads != other.copyable_reference_result_reads {
            let _ = writeln!(
                out,
                " copyable_reference_result_reads:\n  derived:  {:?}\n  inferred: {:?}",
                self.copyable_reference_result_reads, other.copyable_reference_result_reads
            );
        }
        if self.subscript_descriptors != other.subscript_descriptors {
            let _ = writeln!(
                out,
                " subscript_descriptors:\n  derived:  {:?}\n  inferred: {:?}",
                self.subscript_descriptors, other.subscript_descriptors
            );
        }
        if self.call_place_uses != other.call_place_uses {
            let _ = writeln!(
                out,
                " call_place_uses:\n  derived:  {:?}\n  inferred: {:?}",
                self.call_place_uses, other.call_place_uses
            );
        }
        if self.transfers != other.transfers {
            let _ = writeln!(
                out,
                " transfers:\n  derived:  {:?}\n  inferred: {:?}",
                self.transfers, other.transfers
            );
        }
        if self.comparisons != other.comparisons {
            let _ = writeln!(
                out,
                " comparisons:\n  derived:  {:?}\n  inferred: {:?}",
                self.comparisons, other.comparisons
            );
        }
        if self.vanishing_transfers != other.vanishing_transfers {
            let _ = writeln!(
                out,
                " vanishing_transfers:\n  derived:  {:?}\n  inferred: {:?}",
                self.vanishing_transfers, other.vanishing_transfers
            );
        }
        if self.bound_builtins != other.bound_builtins {
            let _ = writeln!(
                out,
                " bound_builtins:\n  derived:  {:?}\n  inferred: {:?}",
                self.bound_builtins, other.bound_builtins
            );
        }
        if self.method_instantiations != other.method_instantiations {
            let _ = writeln!(
                out,
                " method_instantiations:\n  derived:  {:?}\n  inferred: {:?}",
                self.method_instantiations, other.method_instantiations
            );
        }
        if self.constructions != other.constructions {
            let _ = writeln!(
                out,
                " constructions:\n  derived:  {:?}\n  inferred: {:?}",
                self.constructions, other.constructions
            );
        }
        if self.typed_origins != other.typed_origins {
            let _ = writeln!(
                out,
                " typed_origins:\n  derived:  {:?}\n  inferred: {:?}",
                self.typed_origins, other.typed_origins
            );
        }
        if self.locals != other.locals {
            let _ = writeln!(
                out,
                " locals:\n  derived:  {:?}\n  inferred: {:?}",
                self.locals, other.locals
            );
        }
        out
    }

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
            borrowed_reference_receivers: flagged(&self.borrowed_reference_receivers),
            read_temporary_arguments: flagged(&self.read_temporary_arguments),
            // Realization recomputes these from the calls that remain.
            effect_free_callees: Vec::new(),
            builtin_len_calls: flagged(&self.builtin_len_calls),
            // A call and its arguments are copied together.
            selected_calls: at(&self.selected_calls, occurrences)
                .into_iter()
                .map(|(id, mut call)| {
                    for argument in &mut call.arguments {
                        argument.value.copy = id.copy;
                    }
                    (id, call)
                })
                .collect(),
            struct_applications: self.struct_applications.clone(),
            rebind_assertions: at(&self.rebind_assertions, occurrences),
            copy_place_value_uses: flagged(&self.copy_place_value_uses),
            interior_invalidations: at(&self.interior_invalidations, occurrences),
            unconsumed_temporaries: flagged(&self.unconsumed_temporaries),
            discarded_reference_results: flagged(&self.discarded_reference_results),
            reference_value_uses: at(&self.reference_value_uses, occurrences),
            deletable_bindings: flagged(&self.deletable_bindings),
            linear_bindings: flagged(&self.linear_bindings),
            linear_temporaries: flagged(&self.linear_temporaries),
            reference_results: at(&self.reference_results, occurrences),
            interior_references: at(&self.interior_references, occurrences),
            reference_binding_types: at(&self.reference_binding_types, occurrences),
            reference_place_types: at(&self.reference_place_types, occurrences),
            copyable_reference_result_reads: flagged(&self.copyable_reference_result_reads),
            subscript_descriptors: at(&self.subscript_descriptors, occurrences),
            call_place_uses: flagged(&self.call_place_uses),
            transfers: flagged(&self.transfers),
            vanishing_transfers: self.vanishing_transfers,
            comparisons: flagged(&self.comparisons),
            bound_builtins: at(&self.bound_builtins, occurrences),
            method_instantiations: at(&self.method_instantiations, occurrences),
            constructions: flagged(&self.constructions),
            typed_origins: occurrences
                .iter()
                .flat_map(|occurrence| {
                    self.typed_origins
                        .iter()
                        .filter(|typed| typed.occurrence.syntax == occurrence.syntax)
                        .map(|typed| TypedOrigins {
                            occurrence: *occurrence,
                            ..typed.clone()
                        })
                })
                .collect(),
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
            + self.reference_value_uses.len()
            + self.deletable_bindings.len()
            + self.linear_bindings.len()
            + self.linear_temporaries.len()
            + self.reference_results.len()
            + self.interior_references.len()
            + self.reference_binding_types.len()
            + self.reference_place_types.len()
            + self.copyable_reference_result_reads.len()
            + self.subscript_descriptors.len()
            + self.call_place_uses.len()
            + self.transfers.len()
            + self.bound_builtins.len()
            + self.method_instantiations.len()
            + self.constructions.len()
            + self.typed_origins.len()
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
