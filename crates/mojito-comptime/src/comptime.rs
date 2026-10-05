//! Stage 2: compile-time elaboration.
//!
//! A pass between parsing and type-checking that **resolves compile-time
//! constructs before runtime lowering**, per `docs/notes/comptime.md`:
//! `comptime` is a *phase distinction*, so the elaborator rewrites the AST so the
//! checker/MIR/VM only ever see ordinary code.
//!
//! - **`comptime NAME = expr`** — evaluated at compile time (a compile-time value is
//!   required; the elaborator is the validator). Recorded in a compile-time
//!   environment; the statement is kept as an ordinary binding.
//! - **`comptime if`** — keeps only the taken branch. Every branch was already
//!   checked by source validation (`mojito_checker::checker::validate_comptime_templates`,
//!   run by [`elaborate`] and the compiler driver on the [`prepare`]d program),
//!   so dropping the others hides no type error.
//! - **`comptime for`** — unrolls over a compile-time `range(...)` or a compile-time
//!   tuple/list, substituting the loop variable with its literal in each body copy;
//!   a **fuel quota** bounds the work.
//! - **CTFE** — a `comptime` context may call a **pure top-level function**. The
//!   elaborator verifies a restricted helper call graph, folds compile-time-only
//!   facts such as `T.size` and `is_same_type[T, U]()` into literals, and executes
//!   the resulting helper through HIR/MIR on the register VM with a shared fuel
//!   budget. This keeps function-body execution on the same path as runtime code.
//! - **Materialization** — module-level `comptime` constants are inlined as literals
//!   into runtime code, so a top-level comptime value is usable inside functions.
//! - **Delayed generic elaboration (roadmap milestone 6)** — a generic `def` whose (value)
//!   parameters feed a `comptime if`/`comptime for` cannot be elaborated early (the
//!   parameter value is only known per call). Such a def is kept as a *template*;
//!   a monomorphization pass then specializes it per distinct value argument,
//!   resolving the comptime construct so only the *selected* branch reaches the
//!   executable check (`f[0]` and `f[1]` take different branches; the dropped
//!   branch was validated symbolically first).
//!
//! Compile-time values are the shared [`CtValue`](mojito_types::ct::CtValue) universe:
//! runtime-materializable `Int`/`Bool`/`String`/`Tuple`/`List`, plus
//! compile-time-only `Type` and symbolic `Param` facts.

use mojito_ast::ast::{
    ArgConvention, Expr, ExprKind, FnParam, InfixOp, ParamArg, ParamKind, PrefixOp, Stmt, StmtKind,
    StructComptime, TStringPart, Type, TypeParam, WithItem,
};
pub use mojito_symbol::symbol::{
    mangle, tstring_specialization_symbol, tuple_specialization_symbol, tuple_specialization_values,
};

use mojito_ast::call::{CallVariadics, effective_keyword_only_index, match_call_slots};
use mojito_checked::census::CloneClass;
use mojito_common::token::{SourceSpan, Span};
use mojito_types::ct::{CtMarker, CtValue};
use mojito_types::param_expr::{ParamContext, ParamError, ParamExpr};
use mojito_types::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
use mojito_vm::backend::VmBackend;
use mojito_vm::runtime::Value;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

/// One checker-discovered instantiation of the public variadic `Tuple` struct.
///
/// Compile-time elaboration cannot soundly infer the types of arbitrary runtime
/// expressions.  The checker therefore supplies the exact element types and may
/// identify one bare `Tuple(...)` occurrence whose callee should be rewritten to
/// the resulting concrete specialization.  A request without an occurrence only
/// materializes the declaration (for example, for a contextual type use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleSpecializationRequest {
    elements: Vec<Ty>,
    bare_call: Option<SourceSpan>,
    transform: Option<TupleTransformRequest>,
}

/// One value-producing Tuple method selected during checked discovery.
///
/// These requests are receiver-specific: emitting every transform whose result
/// type happens to exist would manufacture reciprocal declaration dependencies
/// (for example `[Int, String].reverse()` and the uncalled reverse direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TupleTransformRequest {
    Reverse,
    Concat(Vec<Ty>),
}

impl TupleSpecializationRequest {
    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub const fn declaration(elements: Vec<Ty>) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: None,
        }
    }

    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub const fn bare_call(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            bare_call: Some(occurrence),
            transform: None,
        }
    }

    pub const fn transform(elements: Vec<Ty>, transform: TupleTransformRequest) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: Some(transform),
        }
    }

    pub fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub const fn occurrence(&self) -> Option<&SourceSpan> {
        self.bare_call.as_ref()
    }

    pub const fn requested_transform(&self) -> Option<&TupleTransformRequest> {
        self.transform.as_ref()
    }
}

/// One checker-discovered lazy template-string occurrence.
///
/// The interleaved element types of a `t"…"` expression (literal segments as
/// `String`, interpolation snapshots at their checked types) and the source
/// occurrence whose AST node monomorphization rewrites into a construction of
/// the concrete `TString` specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TStringSpecializationRequest {
    elements: Vec<Ty>,
    occurrence: SourceSpan,
}

impl TStringSpecializationRequest {
    pub const fn new(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            occurrence: occurrence.without_syntax(),
        }
    }

    pub fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub const fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }
}

/// One checker-discovered inferred application of a bound-generic `def`
/// template.
///
/// The pre-check elaborator cannot infer types, so the compiler's discovery
/// loop replays the checker's resolved instantiation at the exact call
/// occurrence. A request can only upgrade a call from the abstract
/// erased-dispatch path to a concrete clone; any mismatch, misalignment, or
/// collision is skipped and the call stays abstract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefSpecializationRequest {
    /// The call occurrence, stored without its phase-local syntax id.
    occurrence: SourceSpan,
    callee: String,
    /// The selected overload's runtime parameter names, in declaration order:
    /// which declaration of an overloaded template name the request
    /// is for. A uniquely named callee ignores it.
    parameter_names: Vec<String>,
    /// The same parameters' declared types, mangled as `TypeKey`, breaking a
    /// tie between overloads that share a parameter-name list.
    parameter_types: Vec<String>,
    /// The selected overload's `*args` collector, spelled by neither list:
    /// what tells two type-pack overloads apart when every regular parameter
    /// agrees.
    variadic: Option<mojito_symbol::symbol::VariadicKey>,
    /// The checker's declaration-order argument list from `resolve_use_params`.
    arguments: Vec<TyArg>,
}

impl DefSpecializationRequest {
    pub const fn new(
        occurrence: SourceSpan,
        callee: String,
        parameter_names: Vec<String>,
        parameter_types: Vec<String>,
        arguments: Vec<TyArg>,
    ) -> Self {
        Self {
            occurrence: occurrence.without_syntax(),
            callee,
            parameter_names,
            parameter_types,
            variadic: None,
            arguments,
        }
    }

    /// Name the selected overload's `*args` collector as well.
    #[must_use]
    pub fn with_variadic(mut self, variadic: Option<mojito_symbol::symbol::VariadicKey>) -> Self {
        self.variadic = variadic;
        self
    }

    pub const fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }

    pub fn callee(&self) -> &str {
        &self.callee
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
    }

    pub fn parameter_types(&self) -> &[String] {
        &self.parameter_types
    }

    pub const fn variadic(&self) -> Option<&mojito_symbol::symbol::VariadicKey> {
        self.variadic.as_ref()
    }

    pub fn arguments(&self) -> &[TyArg] {
        &self.arguments
    }
}

/// One checker-discovered application of a generic *method* of a specialized
/// variadic struct (`bag.find[Int]()`, `v.set(3)` inferring `T`).
///
/// The specializer mints one clone per distinct instantiation (`find$y3:Int`)
/// inside the owner, and the checker retargets the call to it by exact name on
/// the next discovery round. A request that names no method, or whose
/// arguments do not align with the method's declaration, is skipped: the call
/// keeps the template's erased path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSpecializationRequest {
    /// The call occurrence, stored without its phase-local syntax id.
    occurrence: SourceSpan,
    /// The specialized struct's name as the checker saw it (`Bag$t2[...]`).
    owner: String,
    method: String,
    /// The selected overload's runtime parameter names, in declaration
    /// order: same-named overloads (`set[T](value)` and `set(*, init_with)`)
    /// mint separate clones.
    parameter_names: Vec<String>,
    /// The selected overload's signature qualifier (`$ov$T$Int`), telling
    /// apart same-arity overloads that share their parameter names.
    overload: Option<String>,
    /// The checker's declaration-order argument list from `resolve_use_params`.
    arguments: Vec<TyArg>,
}

impl MethodSpecializationRequest {
    pub const fn new(
        occurrence: SourceSpan,
        owner: String,
        method: String,
        parameter_names: Vec<String>,
        arguments: Vec<TyArg>,
    ) -> Self {
        Self {
            occurrence: occurrence.without_syntax(),
            owner,
            method,
            parameter_names,
            overload: None,
            arguments,
        }
    }

    /// Name the selected overload's signature qualifier as well.
    #[must_use]
    pub fn with_overload(mut self, overload: Option<String>) -> Self {
        self.overload = overload;
        self
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
    }

    /// Whether this request selects `method`, declared on the struct
    /// `owner` names: the same source name and regular parameter names, and,
    /// when the request names one of the method's overloads by its signature
    /// qualifier, that overload.
    pub fn selects(
        &self,
        method: &mojito_ast::ast::Method,
        owner: &str,
        owners: &mojito_symbol::symbol::MethodBinderOwners,
    ) -> bool {
        let regular: Vec<&str> = method
            .params
            .iter()
            .filter(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Regular)
            .map(|parameter| parameter.name.as_str())
            .collect();
        let same_overload = self
            .overload
            .as_deref()
            .is_none_or(|selected| owners.call_qualifier(owner, method) == Some(selected));
        self.method == method.name
            && self.parameter_names.iter().map(String::as_str).eq(regular)
            && same_overload
    }

    pub const fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn arguments(&self) -> &[TyArg] {
        &self.arguments
    }
}

/// One checker-discovered closed application of an ordinary generic struct
/// (`Optional[Int]`): the template name and its declaration-order arguments.
///
/// The specializer appends one clone per available method to the live template
/// with the struct's parameters baked (`get$y3:Int`), and the checker
/// retargets calls on that instance to the clones by exact name on the next
/// discovery round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructInstanceRequest {
    template: String,
    arguments: Vec<TyArg>,
}

impl StructInstanceRequest {
    pub const fn new(template: String, arguments: Vec<TyArg>) -> Self {
        Self {
            template,
            arguments,
        }
    }

    pub fn template(&self) -> &str {
        &self.template
    }

    pub fn arguments(&self) -> &[TyArg] {
        &self.arguments
    }
}

/// Exact callable types which a generated public-Tuple declaration references
/// through opaque compiler-only AST ids.
///
/// Source `def(...)` annotations cannot encode all of this metadata, so the
/// compiler passes this map directly to the second checker pass instead of
/// round-tripping through syntax.
pub fn tuple_materialized_callables(
    requests: &[TupleSpecializationRequest],
) -> HashMap<String, Ty> {
    fn collect(ty: &Ty, output: &mut Vec<Ty>) {
        if matches!(ty, Ty::Func { .. } | Ty::GenericFunc { .. }) {
            if !output.contains(ty) {
                output.push(ty.clone());
            }
            return;
        }
        match ty {
            Ty::Struct(_, arguments) => {
                for argument in arguments {
                    if let TyArg::Ty(ty) = argument {
                        collect(ty, output);
                    }
                }
            }
            Ty::ComptimeList(element)
            | Ty::VariadicPack(element)
            | Ty::Pointer { element, .. }
            | Ty::Assoc { base: element, .. } => collect(element, output),
            Ty::Tuple(elements)
            | Ty::RuntimePack(elements)
            | Ty::Variant(elements)
            | Ty::Overload(elements) => {
                for element in elements {
                    collect(element, output);
                }
            }
            Ty::Ref(reference) => collect(&reference.referent, output),
            Ty::Dependent(dependent) => {
                for element in dependent
                    .selection()
                    .map_or(&[][..], |(elements, _)| elements)
                {
                    collect(element, output);
                }
            }
            _ => {}
        }
    }

    let mut callables = Vec::new();
    for request in requests {
        for element in request.elements() {
            collect(element, &mut callables);
        }
    }
    callables
        .into_iter()
        .enumerate()
        .map(|(index, callable)| (format!("$mojito$callable_type${index}"), callable))
        .collect()
}

/// Comptime-specific accessors on the shared [`CtValue`], reporting a
/// [`ComptimeError`] when a value is not of the required kind.
///
/// An extension trait: `CtValue` lives in the types layer below this phase, so
/// an inherent impl cannot.
pub trait CtValueExt {
    fn as_bool(&self, ctx: &str) -> Result<bool, ComptimeError>;
    fn as_int(&self, ctx: &str) -> Result<i64, ComptimeError>;
    fn as_sequence(&self, ctx: &str) -> Result<Vec<CtValue>, ComptimeError>;
    fn typelist_elements(&self) -> Option<&[CtValue]>;
}

impl CtValueExt for CtValue {
    fn as_bool(&self, ctx: &str) -> Result<bool, ComptimeError> {
        match self {
            Self::Bool(b) => Ok(*b),
            _ => Err(ComptimeError::NotBool(ctx.to_string())),
        }
    }
    fn as_int(&self, ctx: &str) -> Result<i64, ComptimeError> {
        match self {
            Self::Int(n) => Ok(*n),
            Self::IntLiteral(n) => n.wrapping_signed(64).ok_or_else(|| {
                ComptimeError::BadArithmetic(format!(
                    "integer literal cannot materialize as Int in {ctx}"
                ))
            }),
            _ => Err(ComptimeError::NotInt(ctx.to_string())),
        }
    }
    /// The elements of a compile-time collection (`Tuple`/`List`), for
    /// iteration and indexing. A `TypeList` value (`Sized` and iterable
    /// upstream) yields its element types.
    fn as_sequence(&self, ctx: &str) -> Result<Vec<CtValue>, ComptimeError> {
        match self {
            Self::Tuple(v) | Self::List(v) | Self::Set { elements: v, .. } => Ok(v.clone()),
            // A dictionary iterates (and counts) its keys, as at runtime.
            Self::Dict { entries, .. } => Ok(entries.iter().map(|(key, _)| key.clone()).collect()),
            _ => self
                .typelist_elements()
                .map(<[Self]>::to_vec)
                .ok_or_else(|| ComptimeError::BadRange(ctx.to_string())),
        }
    }

    /// The element types carried by a compile-time `TypeList` value, or
    /// `None` for any other value.
    fn typelist_elements(&self) -> Option<&[CtValue]> {
        match self {
            Self::Struct { name, fields } if name == "TypeList" => match fields.as_slice() {
                [(field, Self::Tuple(values))] if field == "values" => Some(values),
                _ => None,
            },
            // A bound type pack (`*Ts` specialized to concrete types) is
            // upstream's `TypeList` in every compile-time position, so
            // `Ts.length`, `Ts[i]`, `Ts.all_conforms_to[..]()`, and
            // `Ts.contains[T]()` read it directly.
            Self::Tuple(values) if values.iter().all(|value| matches!(value, Self::Type(_))) => {
                Some(values)
            }
            _ => None,
        }
    }
}

/// An error from compile-time elaboration.
#[derive(Debug)]
pub enum ComptimeError {
    /// An expression is not compile-time evaluable (or names an unknown comptime).
    NotComptime(String),
    /// A compile-time value used at runtime without an explicit crossing;
    /// the message is upstream's diagnostic verbatim.
    Crossing(String),
    /// A condition did not evaluate to `Bool`.
    NotBool(String),
    /// A context required a compile-time `Int`.
    NotInt(String),
    /// Integer `//`/`%` by zero, or a negative `**` exponent, at compile time.
    BadArithmetic(String),
    /// A `comptime for` iterable was not a `range(...)` / tuple / list.
    BadRange(String),
    /// A `comptime for` iterable whose type has no `__iter__` (a compile-time
    /// Tuple); the payload is the type's spelling.
    NotIterable(String),
    /// A CTFE call had the wrong number of arguments.
    Arity(String),
    /// An inferred type-pack element failed one of the pack's trait bounds at
    /// the call that requested specialization.
    PackBound(Box<PackBoundError>),
    /// An explicit type argument failed its type parameter's trait bound at
    /// the call that requested specialization.
    GenericBound(Box<GenericBoundError>),
    /// A fully specialized declaration's trailing `where` predicate was false.
    Constraint(String),
    /// Source validation rejected a compile-time control-flow construct
    /// before any arm was selected: a checker diagnostic, reported verbatim.
    Type(mojito_common::error::TypeError),
    /// A variadic struct member spelled the struct's own pack bare, where
    /// upstream requires `Self.Ts`; the message is upstream's diagnostic, the
    /// same text the checker reports for a non-pack parameter.
    UnqualifiedStructParam(String),
    /// The compile-time step/iteration quota was exceeded (a likely infinite loop).
    QuotaExceeded,
}

impl From<mojito_symbol::symbol::NonConstantSpecialization> for ComptimeError {
    fn from(error: mojito_symbol::symbol::NonConstantSpecialization) -> Self {
        Self::NotComptime(error.to_string())
    }
}

impl From<ParamError> for ComptimeError {
    fn from(error: ParamError) -> Self {
        match error {
            ParamError::Arithmetic(message) => Self::BadArithmetic(message),
            other => Self::NotComptime(other.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct PackBoundError {
    function: String,
    pack: String,
    index: usize,
    ty: String,
    trait_name: String,
    site: String,
    reason: Option<String>,
}

#[derive(Debug)]
pub struct GenericBoundError {
    function: String,
    param: String,
    ty: String,
    trait_name: String,
    site: String,
    reason: Option<String>,
}

impl std::fmt::Display for ComptimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotComptime(s) => write!(f, "not a compile-time value: {s}"),
            Self::Crossing(s) => write!(f, "{s}"),
            Self::NotBool(s) => write!(f, "expected a compile-time Bool ({s})"),
            Self::NotInt(s) => write!(f, "expected a compile-time Int ({s})"),
            Self::BadArithmetic(s) => write!(f, "compile-time arithmetic error: {s}"),
            Self::BadRange(s) => {
                write!(f, "'comptime for' needs a range(...)/tuple/list: {s}")
            }
            Self::NotIterable(ty) => write!(f, "'{ty}' does not implement the '__iter__' method"),
            Self::Arity(s) => write!(f, "compile-time call arity: {s}"),
            Self::UnqualifiedStructParam(name) => write!(
                f,
                "unqualified access to struct parameter '{name}'; use 'Self.{name}' instead"
            ),
            Self::PackBound(error) => {
                let PackBoundError {
                    function,
                    pack,
                    index,
                    ty,
                    trait_name,
                    site,
                    reason,
                } = error.as_ref();
                write!(
                    f,
                    "type-pack bound failed at '{function}' instantiation {site}: element {} of type pack '{pack}' has type '{ty}', which does not conform to trait '{trait_name}'",
                    index + 1
                )?;
                if let Some(reason) = reason {
                    write!(f, " ({reason})")?;
                }
                Ok(())
            }
            Self::GenericBound(error) => {
                let GenericBoundError {
                    function,
                    param,
                    ty,
                    trait_name,
                    site,
                    reason,
                } = error.as_ref();
                write!(
                    f,
                    "generic bound failed at '{function}' instantiation {site}: type parameter '{param}' received type '{ty}', which does not conform to trait '{trait_name}'"
                )?;
                if let Some(reason) = reason {
                    write!(f, " ({reason})")?;
                }
                Ok(())
            }
            Self::Constraint(message) => {
                write!(f, "compile-time constraint failed: {message}")
            }
            Self::Type(error) => write!(f, "{error}"),
            Self::QuotaExceeded => {
                write!(f, "compile-time execution exceeded the step quota ({FUEL})")
            }
        }
    }
}

/// Elaborate all compile-time constructs in a program, returning an ordinary AST.
///
/// The composed-stage seam: prepares the program, validates every
/// compile-time control-flow construct with the declarations' parameters
/// symbolic (a rejection is [`ComptimeError::Type`]), and only then selects
/// arms and unrolls loops — the same contract the compiler driver enforces.
pub fn elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    let prepared = prepare(program)?;
    mojito_checker::checker::validate_comptime_templates(&prepared).map_err(ComptimeError::Type)?;
    elaborate_prepared(&prepared, ElaborationInputs::default()).map(|elaborated| elaborated.program)
}

/// Prepare a linked program for source validation and elaboration.
///
/// Qualify struct packs, synthesize the derived `copy`/`__hash__` methods,
/// give each conformer the trait defaults it inherits, desugar SIMD-keyed
/// methods, and fold SIMD alias bounds.
///
/// These rewrites normalize declarations without selecting a `comptime if`
/// arm, unrolling a loop, stubbing a template body, or minting a clone, so
/// the result still carries every source body the validator must see. The
/// driver prepares once and re-elaborates the prepared program each
/// discovery round.
pub fn prepare(mut program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    pack_qualification::qualify_struct_packs(&mut program)?;
    synthesize_copyable_copy(&mut program);
    synthesize_hashable_hash(&mut program);
    let mut program =
        mojito_checker::checker::expand_trait_defaults(&program).map_err(ComptimeError::Type)?;
    desugar_simd_keyed_methods(&mut program);
    fold_simd_alias_bounds(&mut program);
    Ok(program)
}

/// An elaborated program plus the generic-struct instances the specializer
/// minted method clones for along the way.
///
/// The instances are the closed applications reached from user code and from
/// other clones, so the driver's discovery loop does not treat the checker's
/// recordings of those instances as new work. `unserved_template_uses` are
/// the references this elaboration left on an abstract path that can reach a
/// compile-time-keyed template's stub; the driver rejects any that survive
/// its discovery fixpoint. `stub_reaching_structs` are the generic structs
/// with such a method: an instance of one that the fixpoint discovers too
/// late to mint clones for would run that method erased, so the driver
/// reports divergence rather than converging on the erased path.
/// `stub_reaching_methods` are those methods, as (struct, method): one with
/// compile-time parameters of its own keeps its per-call clones, which the
/// driver keys for the next round, since its template cannot serve a call.
pub struct Elaborated {
    pub program: Vec<Stmt>,
    pub instances: Vec<StructInstanceRequest>,
    pub stub_reaching_structs: HashSet<String>,
    pub stub_reaching_methods: Vec<(String, String)>,
    pub unserved_template_uses: Vec<UnservedTemplateUse>,
    /// How each generated `def` clone came from its template.
    pub def_traces: Vec<DefInstanceTrace>,
    /// How each per-instantiation or per-call method clone, and each member
    /// of a struct specialized whole, came from its template.
    pub method_traces: Vec<MethodInstanceTrace>,
    /// Every declaration this elaboration generated rather than kept: a
    /// consumer asks this list, never a `$` in a name, since a
    /// module-qualified source name carries one too.
    pub generated: GeneratedDeclarations,
    /// What the checks of this elaboration's VM CTFE subprograms derived
    /// and inferred, for the compilation's own template statistics.
    pub ctfe_template_stats: mojito_checked::templates::TemplateStats,
    /// The bodies this elaboration minted, by class.
    pub clones: mojito_checked::census::CloneCensus,
}

/// What the driver's discovery loop hands one elaboration.
///
/// The requests are the ones the previous round's check discovered; the
/// templates are the compilation's checked templates, which VM CTFE's
/// subprogram checks derive their traced clones from. The default is a
/// first elaboration outside the driver.
#[derive(Clone, Copy, Default)]
pub struct ElaborationInputs<'a> {
    pub tuple_requests: &'a [TupleSpecializationRequest],
    pub tstring_requests: &'a [TStringSpecializationRequest],
    pub def_requests: &'a [DefSpecializationRequest],
    pub method_requests: &'a [MethodSpecializationRequest],
    pub struct_requests: &'a [StructInstanceRequest],
    /// Template methods of ordinary generic structs, as (struct, method),
    /// whose checked bodies hold a type only an instance can lower: each
    /// keeps its per-instantiation clone.
    pub keyed_methods: &'a [(String, String)],
    /// Hashed vector types beyond the eager width-1 set.
    pub hash_leaf_types: &'a [Ty],
    pub templates: Option<&'a mojito_checked::templates::TemplateCatalog>,
}

/// The declarations an elaboration generated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedDeclarations {
    /// `def` clones, by output name.
    pub defs: Vec<String>,
    /// Structs specialized whole (`Tuple$…`), by output name. Every member of
    /// one is generated.
    pub structs: Vec<String>,
    /// Per-call method clones, as (owner, clone name). A per-instantiation
    /// clone is recognized by its explicit receiver type instead.
    pub methods: Vec<(String, String)>,
}

/// The declaration-level expansion trace of one per-instantiation or
/// per-call method clone.
///
/// The clone is appended to its template struct's own method list
/// (`get$y3:Int` on `Box`, `kind$y3:Int$y4:Bool` for a call of `kind[Bool]`
/// on `Box[Int]`), so it is identified by that struct, its name, the source
/// tag stamped on its body, and the byte range of its own body's first
/// statement — same-name overloads clone under one name and one tag, and a
/// `Method` has no range of its own. A member of a struct specialized whole
/// (`Tuple$t2[…]`, `AHasher$vuint64:4;[…]`) is traced the same way, its
/// `owner` the specialized struct and its `template_owner` the template.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodInstanceTrace {
    /// The struct the clone is a method of.
    pub owner: String,
    /// The struct whose method the clone instantiates: `owner` itself for a
    /// per-instantiation or per-call clone.
    pub template_owner: String,
    /// The template struct's module.
    pub owner_module: Option<String>,
    pub clone_name: String,
    /// The source tag stamped on every node of the clone's body.
    pub clone_module: String,
    pub template_name: String,
    /// The first statement of the template method's body.
    pub body: mojito_common::token::Span,
    /// The first statement of the clone's own body: the template's where the
    /// body is copied whole, or the first statement the elaborator kept of a
    /// body that opens with compile-time control flow.
    pub clone_body: mojito_common::token::Span,
    /// The struct's type parameters, then a per-call clone's own, with the
    /// source type written for each.
    pub type_bindings: Vec<(String, Type)>,
    /// A per-call clone's own value parameters, or a value-keyed struct's,
    /// folded into its body.
    pub value_bindings: Vec<(String, CtValue)>,
    /// A per-call clone's own type packs, or a variadic struct's, each with
    /// the source element types written in its signature.
    pub pack_bindings: Vec<(String, Vec<Type>)>,
    /// Whether the clone copies a body the elaborator shaped itself — the
    /// trap stub of an unavailable or a SIMD-keyed template method, or a `Tuple`
    /// specialization's synthesized default constructor: `body` is then that
    /// body's own first statement, and the clone binds nothing.
    pub first_copy_template: bool,
}

/// The declaration-level expansion trace of one generated `def` clone.
///
/// It says which prepared declaration the clone instantiates, and what each
/// of that declaration's compile-time parameters became. A consumer
/// identifies the clone by `clone_module` and `clone_name` exactly as written
/// here, never by demangling a symbol. The occurrence-level trace is the
/// syntax identity each clone node keeps from the template node it was copied
/// from.
#[derive(Debug, Clone, PartialEq)]
pub struct DefInstanceTrace {
    /// The source tag stamped on every node of the clone.
    pub clone_module: String,
    pub clone_name: String,
    pub template_module: Option<String>,
    pub template_name: String,
    pub template_span: mojito_common::token::Span,
    /// Type parameters baked into the clone, with the source type written in
    /// their place, in name order.
    pub type_bindings: Vec<(String, Type)>,
    /// Value parameters folded into the clone as literals.
    pub value_bindings: Vec<(String, CtValue)>,
    /// Type packs expanded into the clone's signature, each with the source
    /// element types written there, in name order.
    pub pack_bindings: Vec<(String, Vec<Type>)>,
    /// Parameters the clone still declares.
    pub residual: Vec<String>,
}

/// The elaborator's declaration-level clone traces in the checker's terms.
///
/// The elaborator records what it generated, and the checker decides what
/// that lets it derive. The driver hands them to the program's check, and
/// VM CTFE to its subprogram's.
pub fn instance_traces(
    defs: Vec<DefInstanceTrace>,
    methods: Vec<MethodInstanceTrace>,
) -> Vec<(
    mojito_checked::templates::InstanceName,
    mojito_checked::templates::InstanceTrace,
)> {
    use mojito_checked::templates::{InstanceName, InstanceTrace, TemplateId};
    let defs = defs.into_iter().map(|trace| {
        (
            InstanceName {
                module: Some(trace.clone_module),
                owner: None,
                name: trace.clone_name,
                body: None,
            },
            InstanceTrace {
                template: TemplateId {
                    module: trace.template_module,
                    owner: None,
                    name: trace.template_name,
                    declaration: trace.template_span,
                },
                type_bindings: trace.type_bindings,
                value_bindings: trace.value_bindings,
                pack_bindings: trace.pack_bindings,
                residual: trace.residual,
                first_copy_template: false,
            },
        )
    });
    let methods = methods.into_iter().map(|trace| {
        (
            InstanceName {
                module: Some(trace.clone_module),
                owner: Some(trace.owner.clone()),
                name: trace.clone_name,
                body: Some(trace.clone_body),
            },
            InstanceTrace {
                template: TemplateId {
                    module: trace.owner_module,
                    owner: Some(trace.template_owner),
                    name: trace.template_name,
                    declaration: trace.body,
                },
                type_bindings: trace.type_bindings,
                value_bindings: trace.value_bindings,
                pack_bindings: trace.pack_bindings,
                residual: Vec::new(),
                first_copy_template: trace.first_copy_template,
            },
        )
    });
    defs.chain(methods).collect()
}

/// The elaborator's generated-declaration list in the checker's terms.
pub fn generated_names(
    generated: GeneratedDeclarations,
) -> mojito_checked::templates::GeneratedNames {
    mojito_checked::templates::GeneratedNames {
        defs: generated.defs.into_iter().collect(),
        structs: generated.structs.into_iter().collect(),
        methods: generated.methods.into_iter().collect(),
    }
}

/// A reference to a template that stays on its abstract path and can run a
/// compile-time-keyed stub, which has no executable body.
///
/// The callee is a compile-time-keyed template, or a bound-generic `def`
/// whose abstract body reaches one. A reference made inside such a
/// bound-generic body is not listed: that body runs only through a listed
/// reference.
///
/// A call is abstract only while the checker still records its instantiation
/// against the template: the checker retargets an inferred call to a clone
/// that already exists without a request. A function-value use has no
/// instantiation and always stays abstract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnservedTemplateUse {
    pub callee: String,
    pub site: SourceSpan,
    pub function_value: bool,
}

/// The top-level variadic struct template names (`struct S[*Ts: Bound]`) of a
/// linked program.
///
/// A specialized instance is named `<template>$t<n>[...]`; the compiler's
/// discovery loop filters checker-recorded method instantiations to receivers
/// of that shape.
pub fn variadic_struct_template_names(program: &[Stmt]) -> HashSet<String> {
    program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct {
                name, type_params, ..
            } if type_params
                .iter()
                .any(|parameter| parameter.name.starts_with('*')) =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect()
}

/// The struct templates the AST cloner specializes whole (a variadic struct
/// such as `Tuple`, a struct keyed by a `DType` or a struct value).
///
/// A method body that applies one over its own struct's parameters has no
/// template the MIR elaborator could instantiate.
pub fn specialized_struct_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_specializable(program, &HashSet::new())
        .into_iter()
        .filter(|(_, statement)| matches!(statement.kind, StmtKind::Struct { .. }))
        .map(|(name, _)| name)
        .collect()
}

/// The top-level bound-generic template names of a linked program, as the
/// elaborator will classify them. The compiler's discovery loop filters
/// checker-recorded instantiations to these callees.
pub fn bound_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_bound_generic_templates(program)
}

/// The top-level type-pack template names (`def show[*Ts: Writable](*args:
/// *Ts)`) of a linked program.
///
/// A call whose element types are not statically evident before checking (a
/// local, a generic construction, an origin-bearing temporary) is minted from
/// the checker-recorded instantiation on the next discovery round, as inferred
/// bound-generic calls are.
pub fn pack_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_pack_generic_templates(program)
}

/// The infix marking a name the lexical nested pass qualified: the enclosing
/// specialization, this marker, and the nested `def`'s source name.
///
/// A declaration and a call spelled this way belong to a nested template the
/// discovery check can still see, so the driver harvests its instantiation
/// like a top-level one. `$` cannot occur in a parsed identifier.
pub const NESTED_MARKER_INFIX: &str = "$nested$";

/// The top-level compile-time-keyed template names (`def show[T: Copyable](x:
/// T)` whose body holds a `comptime if`/`comptime for`) of a linked program.
///
/// A call that omits a parameter is minted from the checker-recorded
/// instantiation on the next discovery round; until then the template stands
/// in as a signature-only stub. A call from an abstract generic body over its
/// own parameters stays on that stub; any other reference that can reach it
/// at the fixpoint is rejected ([`Elaborated::unserved_template_uses`],
/// [`unserved_template_parameter`]).
pub fn comptime_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_comptime_generic_templates(program)
}

/// The top-level `DType`-keyed template names (`def only_dt[dt: DType](a:
/// Scalar[dt])`) of a linked program.
///
/// A call that omits the lane leaves it to the checker, which reads it off the
/// argument's own SIMD slot and records the instantiation the next discovery
/// round mints; until then the template stands in as a signature-only stub.
pub fn dtype_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_dtype_generic_templates(program)
}

/// The source name a template is reported under: a nested `def` is spelled by
/// its qualified marker while the discovery check sees it, but a reader knows
/// it by the name it was written with.
pub fn template_display_name(template: &str) -> &str {
    if template.contains(NESTED_MARKER_INFIX) {
        return template.rsplit_once('$').map_or(template, |(_, name)| name);
    }
    template
}

/// The parameter an inferred application of `template` failed to close.
///
/// That is the declaration of the first checker argument that is not closed;
/// `arguments` is the checker's declaration-order list.
pub fn unserved_template_parameter(
    program: &[Stmt],
    template: &str,
    parameter_names: &[String],
    arguments: &[TyArg],
    is_closed: &dyn Fn(&TyArg) -> bool,
) -> String {
    let Some(parameters) = declaration_type_params(program, template, parameter_names) else {
        return String::new();
    };
    let mut cursor = arguments
        .iter()
        .filter(|argument| !matches!(argument, TyArg::Origin(_)));
    let mut first = None;
    for parameter in parameters {
        if matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
            || parameter.is_origin_mutability_binder(parameters)
        {
            continue;
        }
        first.get_or_insert(parameter);
        match cursor.next() {
            Some(argument) if is_closed(argument) => {}
            _ => return parameter.name.trim_start_matches('*').to_string(),
        }
    }
    first.map_or_else(String::new, |parameter| {
        parameter.name.trim_start_matches('*').to_string()
    })
}

/// The type parameters of the `def` named `template`, at any nesting depth.
///
/// A nested `def` is reported by the same diagnostics as a top-level one, and
/// it is spelled by the name the search is given: its qualified marker while
/// the discovery check sees it, its source name otherwise. An overloaded name
/// resolves to the declaration whose runtime parameters the call supplied, so
/// the message names that overload's parameter rather than a sibling's.
fn declaration_type_params<'a>(
    program: &'a [Stmt],
    template: &str,
    parameter_names: &[String],
) -> Option<&'a Vec<TypeParam>> {
    fn in_block<'a>(
        block: &'a [Stmt],
        template: &str,
        accepts: &dyn Fn(&Stmt) -> bool,
    ) -> Option<&'a Vec<TypeParam>> {
        block.iter().find_map(|statement| match &statement.kind {
            StmtKind::Def {
                name, type_params, ..
            } if name == template && accepts(statement) => Some(type_params),
            StmtKind::Def { body, .. } => in_block(body, template, accepts),
            StmtKind::Struct { methods, .. } => methods
                .iter()
                .find_map(|method| in_block(&method.body, template, accepts)),
            _ => None,
        })
    }
    in_block(program, template, &|statement| {
        declaration_takes_names(statement, parameter_names)
    })
    .or_else(|| in_block(program, template, &|_| true))
}

/// Elaborate a [`prepare`]d, validated program while materializing
/// checker-discovered public `Tuple` and `TString` specializations and
/// inferred bound-generic applications.
///
/// This is the already-validated route: ordinary callers use [`elaborate`],
/// and the compiler's discovery loop — which validates the prepared program
/// once — supplies requests here each round.
pub fn elaborate_prepared(
    program: &[Stmt],
    inputs: ElaborationInputs<'_>,
) -> Result<Elaborated, ComptimeError> {
    let ElaborationInputs {
        tuple_requests,
        tstring_requests,
        def_requests,
        method_requests,
        struct_requests,
        keyed_methods,
        hash_leaf_types,
        templates,
    } = inputs;
    let mut method_requests_by_owner: HashMap<String, Vec<MethodSpecializationRequest>> =
        HashMap::new();
    for request in method_requests {
        method_requests_by_owner
            .entry(request.owner().to_string())
            .or_default()
            .push(request.clone());
    }
    let mut instance_requests: HashMap<String, Vec<Vec<TyArg>>> = HashMap::new();
    for request in struct_requests {
        instance_requests
            .entry(request.template().to_string())
            .or_default()
            .push(request.arguments().to_vec());
    }
    // Every `Hasher` conformer's `_update_with_simd` is cloned per hashed
    // vector type: the closed width-1 set eagerly, wider vectors on demand.
    for statement in program {
        for request in hasher_leaf_requests(statement, hash_leaf_types) {
            method_requests_by_owner
                .entry(request.owner().to_string())
                .or_default()
                .push(request);
        }
    }
    let indexes = mojito_common::timing::span("indexes");
    let conformance =
        mojito_checker::checker::ConformanceOracle::from_program(program).map_err(|error| {
            ComptimeError::NotComptime(format!(
                "could not build the specialization conformance oracle: {error}"
            ))
        })?;
    let mut tuple_universe = Vec::new();
    let mut tuple_transforms = Vec::<(Vec<Ty>, Vec<TupleTransformRequest>)>::new();
    for request in tuple_requests {
        if !tuple_universe
            .iter()
            .any(|elements| elements == request.elements())
        {
            tuple_universe.push(request.elements().to_vec());
        }
        if let Some(transform) = request.requested_transform() {
            if let Some((_, transforms)) = tuple_transforms
                .iter_mut()
                .find(|(elements, _)| elements == request.elements())
            {
                if !transforms.contains(transform) {
                    transforms.push(transform.clone());
                }
            } else {
                tuple_transforms.push((request.elements().to_vec(), vec![transform.clone()]));
            }
        }
    }
    let materialized_callables = tuple_materialized_callables(tuple_requests)
        .into_iter()
        .map(|(key, ty)| (ty, key))
        .collect();
    let bound_generics = collect_bound_generic_templates(program);
    let pack_generics = collect_pack_generic_templates(program);
    let elab = Elab {
        program,
        fns: collect_fns(program),
        structs: collect_structs(program),
        struct_names: program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect(),
        specializable: collect_specializable(program, &bound_generics),
        bound_generics,
        pack_generics,
        served_packs: served_pack_defs(program),
        served_lanes: served_lane_defs(program),
        comptime_generics: collect_comptime_generic_templates(program),
        dtype_generics: collect_dtype_generic_templates(program),
        overload_families: collect_overload_families(program),
        method_binder_owners: mojito_symbol::symbol::MethodBinderOwners::scan(
            program,
            &mojito_symbol::symbol::OverloadSets::scan(program),
        ),
        method_requests: method_requests_by_owner,
        instance_requests,
        keyed_methods: keyed_methods.iter().cloned().collect(),
        hash_leaf_types: hash_leaf_types.to_vec(),
        templates,
        ctfe_template_stats: RefCell::new(mojito_checked::templates::TemplateStats::default()),
        pending_struct_instances: RefCell::new(HashMap::new()),
        per_call_clones: RefCell::new(HashSet::new()),
        nested_clones: Cell::new(0),
        ctfe_clones: Cell::new(0),
        def_requests: def_requests
            .iter()
            .map(|request| {
                (
                    request.occurrence().clone().without_syntax(),
                    request.clone(),
                )
            })
            .collect(),
        stub_reaching: RefCell::new(HashSet::new()),
        per_call_stubs: std::cell::OnceCell::new(),
        template_served_defs: RefCell::new(HashMap::new()),
        conformance,
        tuple_universe,
        tuple_transforms,
        materialized_callables,
        fuel: Cell::new(FUEL),
        template_binders: RefCell::new(Vec::new()),
        def_traces: RefCell::new(Vec::new()),
        method_traces: RefCell::new(Vec::new()),
        generated: RefCell::new(GeneratedDeclarations::default()),
        top_consts: RefCell::new(HashMap::new()),
        generic_aliases: RefCell::new(HashMap::new()),
    };
    drop(indexes);
    elab.check_default_effects(program)?;
    let mut env = HashMap::new();
    let mut elaborated = elab.block(program, &mut env, false)?;
    // A module constant declared after its use crosses here.
    let consts = elab.top_consts.borrow().clone();
    elab.fold_runtime_crossings(&mut elaborated, &consts)?;
    // Materialize module-level comptime constants into runtime literals.
    let materialized = materialize_block(
        elaborated,
        &consts,
        &elab.struct_names,
        &elab.applied_constants(),
    );
    // Monomorphize comptime-dependent generic templates against their call sites.
    let Elaborated {
        program: mut result,
        instances,
        stub_reaching_structs,
        stub_reaching_methods,
        unserved_template_uses,
        def_traces: _,
        method_traces: _,
        generated: _,
        ctfe_template_stats: _,
        clones: _,
    } = elab.monomorphize(materialized, tuple_requests, tstring_requests, def_requests)?;
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    // Per-instantiation and per-call method clones reuse their template's
    // spans; each clone's body gets its own source tag after the uniform
    // module stamp above (the discipline struct specializations follow),
    // keeping span-keyed checked facts separate across instantiations.
    let per_call_clones = elab.per_call_clones.take();
    for statement in &mut result {
        let module = statement.module.clone();
        if let StmtKind::Struct { name, methods, .. } = &mut statement.kind {
            for method in methods.iter_mut() {
                if method.self_ty.is_some()
                    || per_call_clones.contains(&(name.clone(), method.name.clone()))
                {
                    let tag = clone_source_tag(module.as_deref(), name, &method.name);
                    mojito_ast::ast::stamp_source(&mut method.body, &tag);
                }
            }
        }
    }
    // Nested templates are specialized only after enclosing top-level
    // specializations and source stamping. At that point every clone carries its
    // concrete outer substitutions, and per-instance source tags will not be
    // overwritten by the uniform module stamp above.
    let mut unserved_template_uses = unserved_template_uses;
    unserved_template_uses.extend(elab.monomorphize_nested_program(&mut result)?);
    let mut generated = elab.generated.take();
    generated.methods.extend(per_call_clones);
    let def_traces = elab.def_traces.take();
    let method_traces = elab.method_traces.take();
    let mut clones = census::clone_census(&census::Minted {
        prepared: program,
        elaborated: &result,
        def_traces: &def_traces,
        method_traces: &method_traces,
        generated: &generated,
    });
    clones.add(CloneClass::NestedDef, elab.nested_clones.get());
    clones.add(CloneClass::Ctfe, elab.ctfe_clones.get());
    Ok(Elaborated {
        program: result,
        instances,
        stub_reaching_structs,
        stub_reaching_methods,
        unserved_template_uses,
        def_traces,
        method_traces,
        generated,
        clones,
        ctfe_template_stats: elab.ctfe_template_stats.take(),
    })
}

mod census;
mod crossing;
mod ctfe_calls;
mod elab;
mod pack_qualification;
mod packs;
mod params;
mod synth;
mod unparse;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use ctfe_calls::*;
use mojito_ast::simd_width::def_uses_layout_dependent_param;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use packs::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use params::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use synth::*;

/// A statement elaboration rebuilt from `source` with new contents. It keeps
/// the source's syntax identity: that is the occurrence-level expansion trace
/// a checked template's instances are matched by.
const fn rebuilt(source: &Stmt, kind: StmtKind) -> Stmt {
    Stmt {
        kind,
        span: source.span,
        module: None,
        syntax_id: source.syntax_id,
    }
}

/// The origin binders one generated clone declares so its baked type
/// arguments spell their origin slots (`Span[Int, __clone_origin0]`): each an
/// infer-only `Origin`, preceded by an infer-only `Bool` mutability binder
/// unless the slot fixes its mutability. A binder's `OriginParamId` counts
/// down from `u32::MAX`, far above any checker slot index, and spells as its
/// own name wherever the bound type is spelled (`source_type_from_ty`).
///
/// A slot already bound to an enclosing declaration's origin parameter
/// rebinds too only for a binder set built by
/// [`CloneOriginBinders::over_enclosing`]: a `def` call supplies such a
/// binder explicitly, spelled as the enclosing parameter's name.
#[derive(Clone, Default)]
pub(super) struct CloneOriginBinders {
    count: u32,
    params: Vec<TypeParam>,
    enclosing: bool,
}

impl CloneOriginBinders {
    /// Binders that also stand for a slot bound to an enclosing
    /// declaration's origin parameter (`Span[Int, o]` inside `def
    /// slots[o: MutOrigin]`).
    pub(super) fn over_enclosing() -> Self {
        Self {
            enclosing: true,
            ..Self::default()
        }
    }

    /// The declared binders, in the order a clone lists them first.
    pub(super) fn params(&self) -> &[TypeParam] {
        &self.params
    }

    /// The name a synthetic binder's id spells as.
    fn name(id: mojito_types::origin::OriginParamId) -> Option<String> {
        let index = u32::MAX - id.0;
        (index < Self::LIMIT).then(|| {
            format!(
                "{}{index}",
                mojito_symbol::symbol::CLONE_ORIGIN_BINDER_PREFIX
            )
        })
    }

    const LIMIT: u32 = 1 << 16;

    fn fresh(&mut self, slot_mutability: Option<&Expr>) -> Option<mojito_types::origin::Origin> {
        let index = self.count;
        if index >= Self::LIMIT {
            return None;
        }
        self.count += 1;
        self.params.extend(Self::declared(index, slot_mutability));
        Some(mojito_types::origin::Origin::Param(
            mojito_types::origin::OriginParamId(u32::MAX - index),
        ))
    }

    /// The binder `index` as declared: its `Bool` mutability binder unless
    /// the slot fixes the mutability, then the `Origin` itself.
    fn declared(index: u32, slot_mutability: Option<&Expr>) -> Vec<TypeParam> {
        let prefix = mojito_symbol::symbol::CLONE_ORIGIN_BINDER_PREFIX;
        let binder = |name: String, bound: &str, origin_mutability: Option<Expr>| TypeParam {
            name,
            bounds: vec![bound.to_string()],
            value_type: None,
            callable_bound: None,
            origin_mutability,
            infer_only: true,
            default: None,
            constraints: Vec::new(),
        };
        let mut declared = Vec::new();
        let mutability = if let Some(ExprKind::Bool(fixed)) =
            slot_mutability.map(|expression| &expression.kind)
        {
            Expr::new(ExprKind::Bool(*fixed), mojito_common::token::DUMMY_SPAN)
        } else {
            let name = format!("{prefix}_mut{index}");
            declared.push(binder(name.clone(), "Bool", None));
            Expr::new(ExprKind::Identifier(name), mojito_common::token::DUMMY_SPAN)
        };
        declared.push(binder(
            format!("{prefix}{index}"),
            "Origin",
            Some(mutability),
        ));
        declared
    }
}

fn mk(kind: StmtKind, span: Span) -> Stmt {
    Stmt {
        kind,
        span,
        module: None,
        syntax_id: mojito_common::token::SyntaxId::fresh(),
    }
}

/// The pack an expression names, bare (`Ts`, a `def`'s own pack) or through
/// `Self` (`Self.Ts`, a struct's pack inside its members).
fn pack_name(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Member { object, field } if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") => {
            Some(field)
        }
        _ => None,
    }
}

/// Whether an expression names the pack `binding` in either spelling.
fn names_pack(expression: &Expr, binding: &str) -> bool {
    pack_name(expression) == Some(binding)
}

/// The pack a `.values` projection names: `Ts.values` or `Self.Ts.values`.
fn pack_values_projection(expression: &Expr) -> Option<&str> {
    match &expression.kind {
        ExprKind::Member { object, field } if field == "values" => pack_name(object),
        _ => None,
    }
}

/// Whether a block directly contains a `comptime if`/`comptime for` (not descending
/// into nested `def`/`struct`, which have their own compile-time scope).
fn block_has_comptime(stmts: &[Stmt]) -> bool {
    block_has_statement(stmts, &|kind| {
        matches!(
            kind,
            StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. }
        )
    })
}

/// Whether a block directly contains a `comptime for` its template does not
/// serve, under the same scope rule as [`block_has_comptime`]; `packs`
/// names the `def`'s packs as [`comptime_for_is_template_served`] reads them.
fn block_has_unkept_comptime_for(stmts: &[Stmt], packs: &HashSet<String>) -> bool {
    block_has_statement(stmts, &|kind| {
        matches!(kind, StmtKind::ComptimeFor { iter, body, .. }
            if !comptime_for_is_template_served(iter, body, packs))
    })
}

/// Whether a generic `def`'s template serves a `comptime for`: the iterable
/// is a `range` whose bounds are parameter expressions — literals, names,
/// `Self.` members, the length of one of the `def`'s `packs` as the pin
/// spells it at compile time (`args.__len__()`, `Ts.length`, `len(Ts)`;
/// `len(args)` is a runtime value there), and arithmetic over them — and
/// the body declares no `comptime` binding of its own, which the elaborator
/// above MIR would have to evaluate with the index unknown, other than an
/// alias of a pack element ([`pack_element_alias`]). Such a loop is
/// checked once with the index symbolic, carried by MIR as a loop header,
/// and unrolled below MIR; any other — over a compile-time list, a pack, a
/// reflection query — is unrolled in the AST, on a clone per instantiation.
pub(super) fn comptime_for_is_template_served(
    iter: &Expr,
    body: &[Stmt],
    packs: &HashSet<String>,
) -> bool {
    fn pack_length(expression: &Expr, packs: &HashSet<String>) -> bool {
        let names_pack = |expression: &Expr| matches!(&expression.kind, ExprKind::Identifier(name) if packs.contains(name));
        match &expression.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                name == "len"
                    && param_args.is_empty()
                    && kwargs.is_empty()
                    && matches!(args.as_slice(), [pack] if names_pack(pack))
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => method == "__len__" && args.is_empty() && kwargs.is_empty() && names_pack(object),
            ExprKind::Member { object, field } => field == "length" && names_pack(object),
            _ => false,
        }
    }
    fn parameter_shaped(expression: &Expr, packs: &HashSet<String>) -> bool {
        match &expression.kind {
            ExprKind::Int(_) | ExprKind::Identifier(_) => true,
            ExprKind::Member { object, .. } => {
                matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")
                    || pack_length(expression, packs)
            }
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => pack_length(expression, packs),
            ExprKind::Prefix(PrefixOp::Neg, inner) => parameter_shaped(inner, packs),
            ExprKind::Infix(
                InfixOp::Add
                | InfixOp::Sub
                | InfixOp::Mul
                | InfixOp::FloorDiv
                | InfixOp::Mod
                | InfixOp::Pow
                | InfixOp::Shl,
                left,
                right,
            ) => parameter_shaped(left, packs) && parameter_shaped(right, packs),
            _ => false,
        }
    }
    matches!(&iter.kind, ExprKind::Call { name, args, .. }
        if name == "range"
            && !args.is_empty()
            && args.iter().all(|bound| parameter_shaped(bound, packs)))
        && !block_has_statement(body, &|kind| {
            matches!(kind, StmtKind::Comptime { .. })
                && pack_element_alias(kind, &|base| packs.contains(base)).is_none()
        })
}

/// The alias a `comptime NAME = Ts[index]` statement declares of an element
/// of a pack `is_pack` accepts, as the type `Ts[index]` it denotes: such an
/// alias names a parameter expression, so a template carries it as that
/// dependent element rather than evaluating it with the index unknown.
pub(super) fn pack_element_alias(
    kind: &StmtKind,
    is_pack: &dyn Fn(&str) -> bool,
) -> Option<(String, Type)> {
    let StmtKind::Comptime {
        name,
        type_params,
        ty: None,
        where_clauses,
        value,
    } = kind
    else {
        return None;
    };
    let ExprKind::Index { object, index } = &value.kind else {
        return None;
    };
    let ExprKind::Identifier(base) = &object.kind else {
        return None;
    };
    (type_params.is_empty() && where_clauses.is_empty() && is_pack(base)).then(|| {
        (
            name.clone(),
            Type::Named(base.clone(), vec![ParamArg::Value((**index).clone())]),
        )
    })
}

/// Whether a block names `rebind[Dest](value)` anywhere below it, a nested
/// `def` included.
///
/// `rebind` asserts that the operand's parametric type resolves to `Dest`
/// once instantiated. A generator's MIR carries the assertion for the
/// elaborator to judge per instance; a body that still specializes as a
/// clone — a nested `def`, or a compile-time evaluation's unelaborated
/// subprogram — makes it on the clone.
pub(super) fn block_has_rebind(stmts: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder {
        fn visit_expr(&mut self, expr: &Expr) {
            if matches!(&expr.kind, ExprKind::TypeApply { name, .. } | ExprKind::Call { name, .. } if name == "rebind")
            {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { found: false };
    mojito_ast::visit::walk_block(&mut finder, stmts);
    finder.found
}

/// Whether a nested `def`'s or a compile-time evaluation's body can only
/// check once its own parameters are bound: it holds compile-time control
/// flow, or a `rebind` assertion over them. Either way the template is
/// stubbed and every instantiation clones.
fn block_keys_specialization(stmts: &[Stmt]) -> bool {
    block_has_comptime(stmts) || block_has_rebind(stmts)
}

/// Whether a top-level `def`'s body keys a clone per instantiation: it
/// unrolls a `comptime for` over a compile-time list or a pack, or a `def`
/// nested in it asserts a `rebind` — that nested body specializes per call,
/// so the body holding it reaches it through a clone of its own. A `comptime
/// if`, a `comptime for` over a `range` — of a pack's length included — and
/// a `rebind` of its own do not: the template keeps the region or the
/// assertion, the check types it with the binders symbolic, and the
/// elaborator below MIR selects, unrolls, or judges it.
fn def_body_keys_specialization(stmts: &[Stmt], packs: &HashSet<String>) -> bool {
    block_has_unkept_comptime_for(stmts, packs) || nested_def_has_rebind(stmts)
}

/// Whether a `def` (or a lambda) nested anywhere in a block names
/// `rebind[Dest](value)` ([`block_has_rebind`]).
fn nested_def_has_rebind(stmts: &[Stmt]) -> bool {
    struct Finder {
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder {
        fn visit_stmt(&mut self, statement: &Stmt) {
            if let StmtKind::Def { body, .. } = &statement.kind {
                self.found |= block_has_rebind(body);
            }
        }
    }

    let mut finder = Finder { found: false };
    mojito_ast::visit::walk_block(&mut finder, stmts);
    finder.found
}

/// The names a `def`'s type packs go by in its body: each `*Ts` binder,
/// bare, and each collector that spreads one (`*args: *Ts`).
pub(super) fn def_pack_names(type_params: &[TypeParam], params: &[FnParam]) -> HashSet<String> {
    let binders: HashSet<&str> = type_params
        .iter()
        .filter_map(|parameter| parameter.name.strip_prefix('*'))
        .collect();
    let collectors = params.iter().filter_map(|parameter| {
        let Type::Named(spread, _) = &parameter.ty else {
            return None;
        };
        (parameter.kind == ParamKind::Variadic && binders.contains(spread.trim_start_matches('*')))
            .then(|| parameter.name.clone())
    });
    binders
        .iter()
        .map(|binder| (*binder).to_string())
        .chain(collectors)
        .collect()
}

/// Whether a top-level `def` keyed on a type pack is served by its template:
/// its name is among [`served_pack_defs`].
fn pack_def_template_served(statement: &Stmt, served_packs: &HashSet<String>) -> bool {
    matches!(&statement.kind, StmtKind::Def { name, .. } if served_packs.contains(name))
}

/// The names of the pack-keyed top-level `def`s the template serves. Every
/// declaration of the name passes the shape test ([`pack_def_shape_served`]),
/// and every callee a body spreads its pack into is `print` or a served name
/// too, by fixpoint: a served body's spread is a call of the callee's
/// template, which the elaborator expands into the bound pack's elements, so
/// a spread into a cloned callee (a constructor, a method, a `def` the
/// template does not serve) keeps the spreading `def` on the cloner. An
/// overloaded name is served or cloned whole, since a forward (`tally(*rest)`)
/// may bind any of its declarations.
fn served_pack_defs(program: &[Stmt]) -> HashSet<String> {
    let mut spreads: HashMap<String, Vec<String>> = HashMap::new();
    let mut cloned: HashSet<String> = HashSet::new();
    for statement in program {
        let StmtKind::Def { name, .. } = &statement.kind else {
            continue;
        };
        if !pack_keyed_declaration(statement) {
            continue;
        }
        match pack_def_shape_served(statement) {
            Some(callees) => spreads.entry(name.clone()).or_default().extend(callees),
            None => {
                cloned.insert(name.clone());
            }
        }
    }
    let mut served: HashSet<String> = spreads
        .keys()
        .filter(|name| !cloned.contains(*name))
        .cloned()
        .collect();
    loop {
        let before = served.len();
        let kept: HashSet<String> = served
            .iter()
            .filter(|name| {
                spreads[*name]
                    .iter()
                    .all(|callee| callee == "print" || served.contains(callee))
            })
            .cloned()
            .collect();
        served = kept;
        if served.len() == before {
            return served;
        }
    }
}

/// The names of the `DType`- or lane-keyed top-level `def`s the template
/// serves: a uniquely named `def` keyed on a `DType` binder, on a parameter
/// used as a lane width, or on a layout operand, whose shape passes
/// [`lane_def_shape_served`]. Such a body is checked once with its lane
/// slots symbolic, MIR carries the slots in its register types and SIMD
/// instructions, and the elaborator closes them per instance. An overloaded
/// name stays a template family, since overload selection is the checker's.
fn served_lane_defs(program: &[Stmt]) -> HashSet<String> {
    let def_counts = def_name_counts(program);
    let struct_names: HashSet<&str> = program
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    // The structs the cloner still specializes whole (R4): one applied over
    // a lane binder has no instance a served body could name.
    let whole_structs: HashSet<&str> = program
        .iter()
        .filter(|statement| {
            matches!(&statement.kind, StmtKind::Struct { .. })
                && is_specializable_declaration_in(
                    statement,
                    &|bound| struct_names.contains(bound),
                    &HashSet::new(),
                    &HashSet::new(),
                )
        })
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    program
        .iter()
        .filter_map(|statement| {
            let StmtKind::Def { name, .. } = &statement.kind else {
                return None;
            };
            (def_counts[name.as_str()] == 1
                && (dtype_keyed_declaration(statement)
                    || def_uses_layout_dependent_param(statement))
                && lane_def_shape_served(statement, &whole_structs))
            .then(|| name.clone())
        })
        .collect()
}

/// Whether a `DType`- or lane-keyed `def`'s own shape lets its template serve
/// it: every compile-time parameter is a type parameter or an `Int`, `Bool`,
/// or `DType` value the runtime parameters name only as a lane slot
/// ([`template_serves_binders`]), and the body holds no form MIR has no
/// symbolic lane for: a `shuffle`, `slice`, or `join` (a lane gather whose
/// mask is the width's), a `DType` float query over a binder, a `hash` of a
/// lane value (whose hasher leaf is keyed by the closed vector type), a
/// local `comptime` binding (which the cloner's body elaboration evaluates
/// before the check), a nested `def` or lambda, or an application of one of
/// `whole_structs` (a struct the cloner specializes whole) over one of the
/// `def`'s own binders.
fn lane_def_shape_served(statement: &Stmt, whole_structs: &HashSet<&str>) -> bool {
    struct Finder<'a> {
        binders: Vec<&'a str>,
        whole_structs: &'a HashSet<&'a str>,
        found: bool,
    }

    impl Finder<'_> {
        fn whole_application(&self, name: &str, arguments: &[ParamArg]) -> bool {
            self.whole_structs.contains(name)
                && arguments.iter().any(|argument| match argument {
                    ParamArg::Type(ty) => self.binders.iter().any(|binder| type_names(ty, binder)),
                    ParamArg::Value(value) => {
                        self.binders.iter().any(|binder| expr_names(value, binder))
                    }
                    ParamArg::Named { .. } => true,
                })
        }
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.found |= matches!(
                &statement.kind,
                StmtKind::Def { .. } | StmtKind::Comptime { .. }
            );
        }

        fn visit_type(&mut self, ty: &Type) {
            if let Type::Named(name, arguments) = ty {
                self.found |= self.whole_application(name, arguments);
            }
        }

        fn visit_expr(&mut self, expr: &Expr) {
            self.found |= match &expr.kind {
                ExprKind::Lambda { .. } => true,
                ExprKind::Call {
                    name, param_args, ..
                } => name == "hash" || self.whole_application(name, param_args),
                ExprKind::TypeApply { name, args } => self.whole_application(name, args),
                ExprKind::MethodCall { method, .. } => {
                    matches!(method.as_str(), "shuffle" | "slice" | "join")
                }
                ExprKind::Invoke {
                    callee, param_args, ..
                } => match &callee.kind {
                    ExprKind::Member { object, field } => {
                        matches!(field.as_str(), "shuffle" | "slice")
                            || (matches!(&object.kind, ExprKind::Identifier(head) if head == "DType")
                                && param_args.iter().any(|argument| {
                                    !matches!(
                                        argument,
                                        ParamArg::Value(Expr {
                                            kind: ExprKind::Member { object, .. },
                                            ..
                                        }) if matches!(&object.kind, ExprKind::Identifier(head) if head == "DType")
                                    )
                                }))
                    }
                    _ => false,
                },
                _ => false,
            };
        }
    }

    let StmtKind::Def {
        name,
        type_params,
        params,
        ret,
        body,
        ..
    } = &statement.kind
    else {
        return false;
    };
    if !template_serves_binders(type_params, params, name) {
        return false;
    }
    let mut finder = Finder {
        binders: type_params
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect(),
        whole_structs,
        found: false,
    };
    for parameter in params {
        mojito_ast::visit::walk_type(&mut finder, &parameter.ty);
    }
    if let Some(ret) = ret {
        mojito_ast::visit::walk_type(&mut finder, ret);
    }
    mojito_ast::visit::walk_block(&mut finder, body);
    !finder.found
}

/// Whether a pack-keyed `def`'s own shape lets its template serve it, and
/// the callees its body spreads its pack into when so: every compile-time
/// parameter is a type parameter, the pack among them; the collector is read
/// or owned (`var *args`, destroyed last to first after its last element use;
/// an element transferred out by subscript is rejected, as the pin rejects
/// it, since the collector is a `VariadicPack`); every spread of the pack is
/// a call's argument (`show(*args)`, `print(*args)`, `drain(*args^)`); and
/// the body keys no clone. The check types
/// such a body once, with the collector a pack of the symbolic `Ts` and each
/// `args[i]` the dependent `Ts[i]`, and the elaborator below MIR binds the
/// pack from the call.
fn pack_def_shape_served(statement: &Stmt) -> Option<Vec<String>> {
    let StmtKind::Def {
        name,
        type_params,
        params,
        body,
        ..
    } = &statement.kind
    else {
        return None;
    };
    let packs = def_pack_names(type_params, params);
    let shape = type_params.iter().all(|parameter| {
        matches!(
            classify_ct_param(parameter, type_params, name),
            Some(ParamDecl::Type { .. })
        )
    }) && !def_body_keys_specialization(body, &packs);
    shape.then(|| pack_spread_callees(body, &packs)).flatten()
}

/// The callees a block spreads one of `packs` into as a call argument
/// (`other(*args)`, `other(*args^)`), a nested `def` included; `None` when a
/// spread of one of `packs` stands anywhere else (a method call, a
/// parameterized call), which only a clone expands.
fn pack_spread_callees(stmts: &[Stmt], packs: &HashSet<String>) -> Option<Vec<String>> {
    struct Finder<'a> {
        packs: &'a HashSet<String>,
        callees: Vec<String>,
        spreads: usize,
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Spread(inner) => {
                    let spread = match &inner.kind {
                        ExprKind::Identifier(name) => Some(name),
                        ExprKind::Transfer(moved) => match &moved.kind {
                            ExprKind::Identifier(name) => Some(name),
                            _ => None,
                        },
                        _ => None,
                    };
                    self.spreads +=
                        usize::from(spread.is_some_and(|name| self.packs.contains(name)));
                }
                ExprKind::Call { name, args, .. } => {
                    self.callees.extend(
                        args.iter()
                            .filter(|argument| matches!(argument.kind, ExprKind::Spread(_)))
                            .map(|_| name.clone()),
                    );
                }
                _ => {}
            }
        }
    }

    let mut finder = Finder {
        packs,
        callees: Vec::new(),
        spreads: 0,
    };
    mojito_ast::visit::walk_block(&mut finder, stmts);
    (finder.callees.len() == finder.spreads).then_some(finder.callees)
}

fn collect_reference_origin_parameters(
    ty: &Ty,
    origins: &mut HashMap<mojito_types::origin::OriginParamId, mojito_types::origin::Mutability>,
) -> Option<()> {
    match ty {
        Ty::Ref(reference) => {
            let mojito_types::origin::Origin::Param(id) = &reference.origin else {
                if matches!(
                    &reference.origin,
                    mojito_types::origin::Origin::Untracked { .. }
                ) {
                    return collect_reference_origin_parameters(&reference.referent, origins);
                }
                return None;
            };
            match origins.entry(*id) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(reference.mutability);
                }
                std::collections::hash_map::Entry::Occupied(entry)
                    if *entry.get() != reference.mutability =>
                {
                    return None;
                }
                std::collections::hash_map::Entry::Occupied(_) => {}
            }
            collect_reference_origin_parameters(&reference.referent, origins)
        }
        Ty::Struct(_, arguments) => arguments.iter().try_for_each(|argument| match argument {
            TyArg::Ty(ty) => collect_reference_origin_parameters(ty, origins),
            TyArg::Val(_) | TyArg::Origin(_) => Some(()),
        }),
        Ty::Tuple(elements)
        | Ty::RuntimePack(elements)
        | Ty::Variant(elements)
        | Ty::Overload(elements) => elements
            .iter()
            .try_for_each(|element| collect_reference_origin_parameters(element, origins)),
        Ty::ComptimeList(element)
        | Ty::VariadicPack(element)
        | Ty::Pointer { element, origin: _ }
        | Ty::Assoc { base: element, .. } => collect_reference_origin_parameters(element, origins),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().try_for_each(|parameter| {
                collect_reference_origin_parameters(parameter, origins)
            })?;
            collect_reference_origin_parameters(ret, origins)?;
            for optional in [variadic, kw_variadic, error].into_iter().flatten() {
                collect_reference_origin_parameters(optional, origins)?;
            }
            Some(())
        }
        Ty::Dependent(dependent) => dependent
            .selection()
            .map_or(&[][..], |(elements, _)| elements)
            .iter()
            .try_for_each(|element| collect_reference_origin_parameters(element, origins)),
        _ => Some(()),
    }
}

/// Substitute one now-concrete method type binder in source annotations. This
/// is used when variadic Tuple specialization turns its type-filtered generic
/// membership implementation into ordinary overloads.
fn substitute_source_type_binding(ty: &mut Type, binding: &str, replacement: &Type) {
    match ty {
        Type::Named(name, arguments) if name == binding && arguments.is_empty() => {
            *ty = replacement.clone();
        }
        Type::Named(_, arguments) => {
            for argument in arguments {
                substitute_source_param_arg_binding(argument, binding, replacement);
            }
        }
        // `Self.T` — the enclosing struct's own parameter spelled through
        // `Self`, the dominant spelling inside struct bodies.
        Type::SelfParam(name) if name == binding => {
            *ty = replacement.clone();
        }
        Type::Assoc { base, name, args }
            if args.is_empty() && name == binding && matches!(base.as_ref(), Type::SelfType) =>
        {
            *ty = replacement.clone();
        }
        Type::Assoc { base, args, .. } => {
            substitute_source_type_binding(base, binding, replacement);
            for argument in args {
                substitute_source_param_arg_binding(argument, binding, replacement);
            }
        }
        Type::IndexedProjection { base, .. } => {
            substitute_source_type_binding(base, binding, replacement);
        }
        Type::Func {
            type_params,
            params,
            ret,
            raises_type,
            ..
        } => {
            // The contract's own binder of that spelling shadows the binding.
            if type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == binding)
            {
                return;
            }
            for parameter in type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    substitute_source_type_binding(value_type, binding, replacement);
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    substitute_source_type_binding(callable, binding, replacement);
                }
            }
            for parameter in params {
                substitute_source_type_binding(&mut parameter.ty, binding, replacement);
            }
            substitute_source_type_binding(ret, binding, replacement);
            if let Some(error) = raises_type {
                substitute_source_type_binding(error, binding, replacement);
            }
        }
        Type::Ref { referent, .. } => {
            substitute_source_type_binding(referent, binding, replacement);
        }
        Type::Int
        | Type::UInt
        | Type::Bool
        | Type::StringLiteral
        | Type::ClosedStringLiteral
        | Type::Float64
        | Type::None
        | Type::SelfParam(_)
        | Type::SelfType
        | Type::MaterializedCallable(_) => {}
    }
}

fn literal_ct_value(expr: &Expr) -> Option<CtValue> {
    match &expr.kind {
        ExprKind::Int(value) => Some(CtValue::IntLiteral(value.clone())),
        ExprKind::Float(value) => Some(CtValue::FloatLiteral(value.clone())),
        ExprKind::Bool(value) => Some(CtValue::Bool(*value)),
        ExprKind::Str(value) => Some(CtValue::Str(value.clone())),
        ExprKind::TupleLit(values) => values
            .iter()
            .map(literal_ct_value)
            .collect::<Option<Vec<_>>>()
            .map(CtValue::Tuple),
        ExprKind::ListLit(values) => values
            .iter()
            .map(literal_ct_value)
            .collect::<Option<Vec<_>>>()
            .map(CtValue::List),
        _ => None,
    }
}

/// A value parameter's default as declaration metadata: literals, the
/// sibling value parameters declared before it, and integer arithmetic over
/// them, built through the shared typed constructors. The elaborator itself
/// evaluates a default from its source expression, so `None` here only means
/// the metadata carries no symbolic form.
fn ct_expr_from_ast(expr: &Expr, siblings: &[TypeParam], owner: &str) -> Option<ParamExpr> {
    let context = ParamContext::detached();
    match &expr.kind {
        ExprKind::Identifier(name) => {
            let sibling = siblings.iter().find(|sibling| sibling.name == *name)?;
            let ty = match (&sibling.value_type, sibling.bounds.as_slice()) {
                (Some(source), _) => ct_param_source_type(source)?,
                (None, [only]) => ct_value_param_type(only)?,
                _ => return None,
            };
            Some(context.decl_ref(
                elaborated_binder(sibling, siblings, owner),
                name,
                mojito_types::param_expr::MetaTy::value(ty),
            ))
        }
        ExprKind::Prefix(PrefixOp::Neg, value) => {
            context.neg(&ct_expr_from_ast(value, siblings, owner)?).ok()
        }
        ExprKind::Infix(
            op @ (InfixOp::Add
            | InfixOp::Sub
            | InfixOp::Mul
            | InfixOp::FloorDiv
            | InfixOp::Mod
            | InfixOp::Pow),
            left,
            right,
        ) => context
            .infix(
                *op,
                &ct_expr_from_ast(left, siblings, owner)?,
                &ct_expr_from_ast(right, siblings, owner)?,
            )
            .ok(),
        _ => context.constant(literal_ct_value(expr)?).ok(),
    }
}

fn ct_param_source_type(source: &Type) -> Option<Ty> {
    match source {
        Type::Int => Some(Ty::Int),
        Type::UInt => Some(Ty::UInt),
        Type::Bool => Some(Ty::Bool),
        Type::StringLiteral | Type::ClosedStringLiteral => Some(Ty::StringLiteral),
        Type::Float64 => Some(Ty::Float64),
        Type::None => Some(Ty::None),
        Type::Named(name, args) if name == "List" && args.len() == 1 => {
            let ParamArg::Type(element) = &args[0] else {
                return None;
            };
            Some(list_type(ct_param_source_type(element)?))
        }
        Type::Named(name, args) if name == "Tuple" => args
            .iter()
            .map(|argument| match argument {
                ParamArg::Type(ty) => ct_param_source_type(ty),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(tuple_type),
        // A vector-typed value parameter (`[key: SIMD[DType.uint64, 4]]`).
        Type::Named(name, args) if name == "SIMD" => {
            simd_source_dims(args).map(|(dtype, width)| Ty::Simd {
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(width),
            })
        }
        _ => None,
    }
}

fn source_type_from_ty_with_origins(
    ty: &Ty,
    origin_names: &HashMap<mojito_types::origin::OriginParamId, String>,
    materialized_callables: &[(Ty, String)],
) -> Option<Type> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => Type::Int,
        Ty::UInt => Type::UInt,
        Ty::Bool => Type::Bool,
        Ty::StringLiteral => Type::ClosedStringLiteral,
        Ty::Float64 | Ty::FloatLiteral => Type::Float64,
        Ty::None => Type::None,
        Ty::Dtype => Type::Named("DType".to_string(), Vec::new()),
        callable @ (Ty::Func { .. } | Ty::GenericFunc { .. }) => {
            let (_, key) = materialized_callables
                .iter()
                .find(|(candidate, _)| candidate == callable)?;
            Type::MaterializedCallable(key.clone())
        }
        Ty::ComptimeList(element) => Type::Named(
            "List".to_string(),
            vec![ParamArg::Type(source_type_from_ty_with_origins(
                element,
                origin_names,
                materialized_callables,
            )?)],
        ),
        Ty::Tuple(elements) => Type::Named(
            "__RuntimeTuple".to_string(),
            elements
                .iter()
                .map(|element| {
                    source_type_from_ty_with_origins(element, origin_names, materialized_callables)
                })
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .map(ParamArg::Type)
                .collect(),
        ),
        // A generated public-Tuple specialization retains its element types
        // as semantic metadata behind an argument-less symbol. Spell it as the
        // canonical `Tuple[...]` application, which the checker maps back onto
        // the discovered specialization, instead of applying the retained
        // arguments to the erased symbol.
        Ty::Struct(name, _)
            if name != mojito_types::types::TUPLE_TYPE_NAME
                && let Some(elements) = mojito_types::types::tuple_elements(ty) =>
        {
            Type::Named(
                mojito_types::types::TUPLE_TYPE_NAME.to_string(),
                elements
                    .into_iter()
                    .map(|element| {
                        source_type_from_ty_with_origins(
                            element,
                            origin_names,
                            materialized_callables,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into_iter()
                    .map(ParamArg::Type)
                    .collect(),
            )
        }
        Ty::Struct(name, arguments) => Type::Named(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => {
                        source_type_from_ty_with_origins(ty, origin_names, materialized_callables)
                            .map(ParamArg::Type)
                    }
                    TyArg::Val(value) => value.materialize((0, 0)).map(ParamArg::Value),
                    // An origin tail entry spells as the binder it names when
                    // the clone has one in scope, else as upstream's `_`
                    // placeholder: a concrete place has no source spelling,
                    // and the slot infers again at the clone's own use sites.
                    TyArg::Origin(origin) => {
                        // A clone's own origin binder spells as a bare type
                        // name, which an origin slot reads as the binder: it
                        // adds no expression the template did not check.
                        if let mojito_types::origin::Origin::Param(id) = origin
                            && !origin_names.contains_key(id)
                            && let Some(binder) = CloneOriginBinders::name(*id)
                        {
                            return Some(ParamArg::Type(Type::Named(binder, Vec::new())));
                        }
                        let spelling = match origin {
                            mojito_types::origin::Origin::Param(id) => origin_names
                                .get(id)
                                .cloned()
                                .unwrap_or_else(|| "_".to_string()),
                            _ => "_".to_string(),
                        };
                        Some(ParamArg::Value(Expr::new(
                            ExprKind::Identifier(spelling),
                            (0, 0),
                        )))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        // A symbolic slot has no source spelling; the elaborator only spells
        // bound instances.
        Ty::Simd { dtype, width } => Type::Named(
            "SIMD".to_string(),
            vec![
                ParamArg::Value(CtValue::Dtype(dtype.known()?).materialize((0, 0))?),
                ParamArg::Value(CtValue::Int(width.known()?).materialize((0, 0))?),
            ],
        ),
        // A pointer spells only over a clone's own origin binder
        // (`Elab::clone_binding`), re-applying the binder's interior and
        // subtree projection; a place origin has no source spelling.
        Ty::Pointer {
            element,
            origin:
                mojito_types::origin::PointerOrigin::Param {
                    id,
                    interior,
                    subtree,
                    ..
                },
        } if !origin_names.contains_key(id) => {
            let mut origin = Type::Named(CloneOriginBinders::name(*id)?, Vec::new());
            for tag in interior {
                origin = Type::IndexedProjection {
                    base: Box::new(Type::Assoc {
                        base: Box::new(origin),
                        name: "_get_owned_interior".to_string(),
                        args: Vec::new(),
                    }),
                    index: Box::new(Expr::new(ExprKind::Str(tag.clone()), (0, 0))),
                };
            }
            if *subtree {
                origin = Type::Assoc {
                    base: Box::new(origin),
                    name: "_subtree".to_string(),
                    args: Vec::new(),
                };
            }
            Type::Named(
                "Pointer".to_string(),
                vec![
                    ParamArg::Type(source_type_from_ty_with_origins(
                        element,
                        origin_names,
                        materialized_callables,
                    )?),
                    ParamArg::Type(origin),
                ],
            )
        }
        Ty::Ref(reference) => {
            let origin_name = match &reference.origin {
                mojito_types::origin::Origin::Param(id) => origin_names.get(id)?.clone(),
                mojito_types::origin::Origin::Untracked { mutable: false } => {
                    "UntrackedOrigin".to_string()
                }
                _ => return None,
            };
            Type::Ref {
                referent: Box::new(source_type_from_ty_with_origins(
                    &reference.referent,
                    origin_names,
                    materialized_callables,
                )?),
                origin: Some(vec![Expr::new(ExprKind::Identifier(origin_name), (0, 0))]),
            }
        }
        _ => return None,
    })
}

impl Elab<'_> {
    /// Every clone binder `ty`'s struct origin tails name, with the
    /// mutability its slot declares.
    fn collect_clone_binders(&self, ty: &Ty, found: &mut Vec<(u32, Option<Expr>)>) {
        if let Ty::Pointer { element, origin } = ty {
            self.collect_clone_binders(element, found);
            if let mojito_types::origin::PointerOrigin::Param { id, mutability, .. } = origin
                && CloneOriginBinders::name(*id).is_some()
            {
                let fixed = match mutability {
                    mojito_types::origin::Mutability::Mutable => Some(true),
                    mojito_types::origin::Mutability::Immutable => Some(false),
                    mojito_types::origin::Mutability::Param(_) => None,
                };
                found.push((
                    u32::MAX - id.0,
                    fixed.map(|fixed| {
                        Expr::new(ExprKind::Bool(fixed), mojito_common::token::DUMMY_SPAN)
                    }),
                ));
            }
            return;
        }
        let Ty::Struct(name, arguments) = ty else {
            return;
        };
        let mut slots = self.explicit_origin_slots(name).into_iter();
        for argument in arguments {
            match argument {
                TyArg::Ty(inner) => self.collect_clone_binders(inner, found),
                TyArg::Origin(origin) => {
                    let slot = slots.next();
                    if let mojito_types::origin::Origin::Param(id) = origin
                        && CloneOriginBinders::name(*id).is_some()
                    {
                        found.push((
                            u32::MAX - id.0,
                            slot.and_then(|slot| slot.origin_mutability.clone()),
                        ));
                    }
                }
                TyArg::Val(_) => {}
            }
        }
    }

    /// `ty` with every origin slot of an origin-slotted struct rebound to a
    /// fresh clone binder; see [`Elab::clone_binding`].
    fn bind_clone_origins(&self, ty: &Ty, binders: &mut CloneOriginBinders) -> Option<Ty> {
        if let Ty::Pointer { element, origin } = ty {
            let element = Box::new(self.bind_clone_origins(element, binders)?);
            let Some(projection) = origin.clone_bindable_place() else {
                return Some(Ty::Pointer {
                    element,
                    origin: origin.clone(),
                });
            };
            let fixed = Expr::new(
                ExprKind::Bool(projection.mutable),
                mojito_common::token::DUMMY_SPAN,
            );
            let mojito_types::origin::Origin::Param(id) = binders.fresh(Some(&fixed))? else {
                return None;
            };
            return Some(Ty::Pointer {
                element,
                origin: projection.binder_origin(id),
            });
        }
        let Ty::Struct(name, arguments) = ty else {
            return (!self.ty_mentions_origin_slotted_struct(ty)).then(|| ty.clone());
        };
        let slots = self.explicit_origin_slots(name);
        let tail = arguments
            .iter()
            .filter(|argument| matches!(argument, TyArg::Origin(_)))
            .count();
        if tail != slots.len() {
            return None;
        }
        let mut slots = slots.into_iter();
        let arguments = arguments
            .iter()
            .map(|argument| match argument {
                TyArg::Ty(inner) => self.bind_clone_origins(inner, binders).map(TyArg::Ty),
                TyArg::Val(value) => (!matches!(value, CtValue::Tuple(elements)
                    if elements.iter().any(|element| matches!(element,
                        CtValue::Type(inner) if self.ty_mentions_origin_slotted_struct(inner)))))
                .then(|| argument.clone()),
                TyArg::Origin(mojito_types::origin::Origin::Param(_)) if !binders.enclosing => None,
                TyArg::Origin(_) => {
                    let slot = slots.next()?;
                    (slot.bounds == ["Origin"])
                        .then(|| binders.fresh(slot.origin_mutability.as_ref()))
                        .flatten()
                        .map(TyArg::Origin)
                }
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Ty::Struct(name.clone(), arguments.into()))
    }
}

/// A compile-time value as the runtime value the VM takes
/// (`mojito_vm::crossing`), a refusal reported as a compile-time error.
fn ct_to_vm(value: &CtValue) -> Result<Value, ComptimeError> {
    mojito_vm::crossing::ct_to_vm(value).map_err(crossing_error)
}

/// A runtime value the VM produced as a compile-time value
/// (`mojito_vm::crossing`), a refusal reported as a compile-time error.
fn vm_to_ct(value: Value) -> Result<CtValue, ComptimeError> {
    mojito_vm::crossing::vm_to_ct(value).map_err(crossing_error)
}

fn crossing_error(error: mojito_vm::runtime::RuntimeError) -> ComptimeError {
    ComptimeError::NotComptime(match error {
        mojito_vm::runtime::RuntimeError::Unsupported(text) => text,
        other => other.to_string(),
    })
}

/// A CTFE-callable function: a pure top-level `def`, optionally with compile-time
/// parameters specialized at the call site.
struct CtFn<'a> {
    ct_params: Vec<ParamDecl>,
    params: Vec<String>,
    body: &'a [Stmt],
}

/// Compile-time metadata for a top-level struct, enough for generic CTFE to read
/// associated facts such as `T.size`.
struct CtStruct<'a> {
    decls: Vec<ParamDecl>,
    /// The source parameters `decls` classified from — the fallback for
    /// declared defaults classification cannot resolve without evaluation
    /// (`H: Hasher = default_hasher` names a module alias).
    source_params: &'a [TypeParam],
    associated: &'a [StructComptime],
    fields: &'a [mojito_ast::ast::Param],
    /// Whether instances construct fieldwise (`@fieldwise_init`, or a
    /// hand-written `__init__` mirroring the fields in declaration order) —
    /// the precondition for freezing a VM instance into a
    /// [`CtValue::Struct`] and materializing it back.
    fieldwise: bool,
}

/// Whether a declaration must remain a template until a concrete call selects
/// its compile-time arguments. This predicate is intentionally independent of
/// the top-level registry: nested generic pack functions need the same delayed
/// elaboration even though their lexical specialization happens later.
fn is_specializable_declaration(
    statement: &Stmt,
    served_packs: &HashSet<String>,
    served_lanes: &HashSet<String>,
) -> bool {
    is_specializable_declaration_in(statement, &|_| false, served_packs, served_lanes)
}

/// The registry-aware form: `is_value_struct` recognizes a single bound that
/// names a struct (a struct-typed value parameter such as `[e: Extent]`),
/// which — like a `DType` parameter — forces per-application monomorphization.
fn is_specializable_declaration_in(
    statement: &Stmt,
    is_value_struct: &dyn Fn(&str) -> bool,
    served_packs: &HashSet<String>,
    served_lanes: &HashSet<String>,
) -> bool {
    match &statement.kind {
        StmtKind::Def {
            name,
            type_params,
            params,
            body,
            ..
        } => {
            !type_params.is_empty()
                && (def_body_keys_specialization(body, &def_pack_names(type_params, params))
                    // A type pack keys a clone per call unless the template
                    // serves the body (`pack_def_template_served`).
                    || (type_params
                        .iter()
                        .any(|parameter| parameter.name.starts_with('*'))
                        && !pack_def_template_served(statement, served_packs))
                    // A `[dtype: DType]` parameter, or a parameter used as a
                    // lane width or a layout operand, keys a clone per call
                    // unless the template serves the body
                    // (`served_lane_defs`): the body is checked once with the
                    // lane symbolic, and the elaborator closes it per
                    // instance.
                    || ((dtype_keyed_declaration(statement)
                        || def_uses_layout_dependent_param(statement))
                        && !served_lanes.contains(name)))
        }
        StmtKind::Struct { type_params, .. } => {
            type_params
                .iter()
                .any(|parameter| parameter.name.starts_with('*'))
                // Struct- and vector-typed value parameters only execute
                // concretely, so the struct monomorphizes per application. A
                // `DType` or lane-width parameter is a generator's binder the
                // template serves: the elaborator closes it per instance.
                || type_params.iter().any(|parameter| {
                    matches!(parameter.bounds.as_slice(), [only] if is_value_struct(only))
                        || parameter.value_type.as_ref().is_some_and(|source| {
                            matches!(ct_param_source_type(source), Some(Ty::Simd { .. }))
                        })
                })
        }
        _ => false,
    }
}

/// The maximum number of compile-time "steps" (loop iterations, statements
/// executed, function calls) across a whole program — a hard bound so compile-time
/// execution can't hang the compiler (cf. Zig's quota).
const FUEL: usize = mojito_vm::crossing::CTFE_FUEL;

/// The compile-time elaboration engine: the CTFE-callable functions and a shared
/// fuel budget. `top_consts` captures module-level constants for materialization;
/// `specializable` holds the comptime-dependent generic `def` templates
/// (roadmap milestone 6).
struct Elab<'a> {
    program: &'a [Stmt],
    fns: HashMap<String, CtFn<'a>>,
    structs: HashMap<String, CtStruct<'a>>,
    /// Every declared struct name, for materialization's projection rewrite.
    struct_names: HashSet<String>,
    /// Top-level generic `def`s whose value parameters feed a `comptime if`/`for`
    /// (so they must be monomorphized per call), by name → the template `Stmt`.
    specializable: HashMap<String, &'a Stmt>,
    /// The subset of `specializable` that is a plain trait-bound generic `def`
    /// (no comptime constructs). Calls resolve softly: an explicit concrete
    /// application monomorphizes, every other reference stays on the template's
    /// abstract erased-dispatch path and retains the template.
    bound_generics: HashSet<String>,
    /// Top-level type-pack `def`s (a `*Ts` type parameter, unique name). A
    /// call whose pack element types the elaborator cannot read syntactically
    /// consults the checker-recorded instantiation for its occurrence, and a
    /// deferred call keeps the template as a signature-only stub for the
    /// discovery check.
    pack_generics: HashSet<String>,
    /// The pack-keyed `def`s the template serves ([`served_pack_defs`]).
    served_packs: HashSet<String>,
    /// The `DType`- or lane-keyed `def`s the template serves
    /// ([`served_lane_defs`]).
    served_lanes: HashSet<String>,
    /// The subset of `specializable` specialized only for its compile-time
    /// control flow (unique name, no pack, `DType`, or SIMD-width parameter).
    /// A call that omits a parameter consults the checker-recorded
    /// instantiation for its occurrence; a deferred call keeps the template as
    /// a signature-only stub for the discovery check.
    comptime_generics: HashSet<String>,
    /// The subset of `specializable` keyed on a `DType` parameter of its own.
    /// The lane such a call omits is the argument's, which only the checker
    /// reads, so the call consults the checker-recorded instantiation for its
    /// occurrence and a deferred one keeps the template as a signature-only
    /// stub for the discovery check.
    dtype_generics: HashSet<String>,
    /// The declarations of every overloaded template name, in
    /// declaration order (see [`collect_overload_families`]). A call
    /// to such a name is served only from the checker's recorded
    /// instantiation, which names the selected overload.
    overload_families: HashMap<String, Vec<&'a Stmt>>,
    /// The owner of each struct method's own binders, as the checker names
    /// it: an overloaded method's is its lowered symbol.
    method_binder_owners: mojito_symbol::symbol::MethodBinderOwners,
    /// Checker-discovered generic-method instantiations on specialized
    /// variadic structs, by owner name: each becomes a per-call clone.
    method_requests: HashMap<String, Vec<MethodSpecializationRequest>>,
    /// Checker-discovered closed applications of ordinary generic structs, by
    /// template name: each mints per-instantiation method clones on the
    /// template.
    instance_requests: HashMap<String, Vec<Vec<TyArg>>>,
    /// Driver-reported template methods that keep their per-instantiation
    /// clones, as (struct, method).
    keyed_methods: HashSet<(String, String)>,
    /// Checker-owned declaration facts used to validate inferred pack bounds
    /// before specialization consumes the source generic call.
    conformance: mojito_checker::checker::ConformanceOracle,
    /// Closed-world public Tuple element sets discovered by the first checker
    /// pass. Concrete Tuple specializations use this universe to emit ordinary
    /// reverse/concat overloads whose result implementation also exists.
    tuple_universe: Vec<Vec<Ty>>,
    /// Receiver element sets paired with exactly the Tuple transforms observed
    /// by checked discovery. This is deliberately separate from the universe:
    /// mere materialization of a result type must not create an uncalled method.
    tuple_transforms: Vec<(Vec<Ty>, Vec<TupleTransformRequest>)>,
    /// Reverse lookup for the opaque callable ids emitted into generated Tuple
    /// annotations. The compiler independently passes the forward map to the
    /// second checker pass.
    materialized_callables: Vec<(Ty, String)>,
    /// The checker-demanded hashed vector types beyond the eager width-1
    /// set; the VM-CTFE subprogram mints the same hasher clones from them.
    hash_leaf_types: Vec<Ty>,
    /// The compilation's checked templates, which the checks of a VM-CTFE
    /// subprogram derive its traced clones from; absent outside the driver.
    templates: Option<&'a mojito_checked::templates::TemplateCatalog>,
    /// What those checks derived and inferred.
    ctfe_template_stats: RefCell<mojito_checked::templates::TemplateStats>,
    /// Fully concrete applications of vector-keyed value templates named by
    /// compile-time type expressions (`comptime default_hasher =
    /// AHasher[SIMD[DType.uint64, 4](0)]`), by mangled clone name: each is
    /// one specialization identity for every consumer, and monomorphization
    /// mints them alongside the call-site applications.
    pending_struct_instances: RefCell<HashMap<String, (String, Vec<CtValue>)>>,
    /// Per-call method clones minted on a non-generic struct, as (owner,
    /// clone name). They carry no receiver type, so source stamping names
    /// them here rather than by `Method::self_ty`.
    per_call_clones: RefCell<HashSet<(String, String)>>,
    /// Nested `def` clones minted, for the instantiation census.
    nested_clones: Cell<usize>,
    /// Bodies minted for VM CTFE subprograms, for the instantiation census.
    ctfe_clones: Cell<usize>,
    /// The driver's checker-discovered bound-generic applications by call
    /// occurrence. Top-level monomorphization consults its own seeded copy;
    /// the lexical nested pass, which runs after that walk, reads these.
    def_requests: HashMap<SourceSpan, DefSpecializationRequest>,
    /// The bodies top-level monomorphization found could run a
    /// compile-time-keyed stub, including the [`nested_body_owner`] keys of
    /// nested `def`s. The nested pass registers a nested `def` named here,
    /// so that its instances reach the callee's clone.
    stub_reaching: RefCell<HashSet<String>>,
    /// The struct methods whose template is a trap stub that only a per-call
    /// clone serves (a `comptime if` over the method's own binders), as
    /// [`method_owner`] keys: a body calling one over its own binders reaches
    /// a stub.
    per_call_stubs: std::cell::OnceCell<HashSet<String>>,
    /// Whether a bound-generic `def`'s template serves its closed calls, by
    /// name, as first decided ([`Elab::template_serves_def`]).
    template_served_defs: RefCell<HashMap<String, bool>>,
    fuel: Cell<usize>,
    /// The compile-time parameter names of each generic `def` whose body is
    /// being elaborated as a template, innermost last. A `comptime if`
    /// whose condition names one is kept for the check: its arms are the
    /// template's, and the elaborator below MIR selects.
    template_binders: RefCell<Vec<HashSet<String>>>,
    /// The declaration-level trace of every `def` clone generated so far.
    def_traces: RefCell<Vec<DefInstanceTrace>>,
    /// The same for every whole-instance method clone.
    method_traces: RefCell<Vec<MethodInstanceTrace>>,
    /// The `def` clones and whole-struct specializations generated so far.
    generated: RefCell<GeneratedDeclarations>,
    top_consts: RefCell<HashMap<String, CtValue>>,
    /// Module-scope generic `comptime` aliases in declaration order, name →
    /// (parameters, body). The declarations pass through elaboration for the
    /// checker's alias registry, but an application inside a `comptime if`
    /// condition must already evaluate here — the branches are pruned before
    /// checking.
    generic_aliases: RefCell<HashMap<String, (Vec<TypeParam>, Expr)>>,
}

fn classify_ct_params(tps: &[TypeParam], owner: &str) -> Vec<ParamDecl> {
    tps.iter()
        .filter_map(|tp| classify_ct_param(tp, tps, owner))
        .collect()
}

fn materialize_ct_value(value: CtValue, ty: &Ty) -> Option<CtValue> {
    value.materialize_as(ty)
}

fn substitute_source_param_arg_binding(argument: &mut ParamArg, binding: &str, replacement: &Type) {
    match argument {
        ParamArg::Type(ty) => substitute_source_type_binding(ty, binding, replacement),
        ParamArg::Named { value, .. } => {
            substitute_source_param_arg_binding(value, binding, replacement);
        }
        // The parser encodes a bare identifier argument (`Tuple[T, T]`) as a
        // value expression; once the binding is concrete it is a type argument.
        ParamArg::Value(expr) => {
            if matches!(&expr.kind, ExprKind::Identifier(name) if name == binding) {
                *argument = ParamArg::Type(replacement.clone());
            }
        }
    }
}

/// The concrete clone a checker-discovered inferred application selects.
struct DefCallTarget {
    template: String,
    /// Which declaration of an overloaded template name the request
    /// selected, as an index into [`Elab::overload_families`]. `None` for
    /// every uniquely named template.
    decl: Option<usize>,
    vals: Vec<CtValue>,
}

/// The concrete `TString` specialization a checked `t"…"` occurrence
/// constructs, with the interleaved element types directing the argument
/// rewrite.
struct TStringTarget {
    symbol: String,
    elements: Vec<Ty>,
}

/// The source tag a per-instantiation or per-call method clone's body carries:
/// the module, the owning struct, and the clone's own name.
///
/// Clones reuse their template's spans, so this tag is what keeps span-keyed
/// checked facts — recorded instantiations above all — separate across
/// instantiations. A clone is stamped before it is walked, so the walk's own
/// span-keyed lookups (`def_call_targets` and its siblings) find the
/// checker's records for that clone rather than the template's.
fn clone_source_tag(module: Option<&str>, owner: &str, method: &str) -> String {
    match module {
        Some(module) => format!("{module}${owner}${method}"),
        None => format!("{owner}${method}"),
    }
}

/// The key [`Mono::abstract_owner`] gives a struct method's erased body.
/// Neither half can contain a `.`, so [`owner_method`] recovers the method.
fn method_owner(owner: &str, method: &str) -> String {
    format!("{owner}.{method}")
}

/// The method half of a [`method_owner`] key, or `None` for a `def` owner.
fn owner_method(owner: &str) -> Option<&str> {
    if owner.starts_with(NESTED_OWNER_PREFIX) {
        return None;
    }
    owner.split_once('.').map(|(_, method)| method)
}

/// The prefix of a [`nested_body_owner`] key, which a source path's `.` would
/// otherwise make [`owner_method`] read as a method name.
const NESTED_OWNER_PREFIX: &str = "$nested-body$";

/// The key [`Mono::abstract_owner`] gives a generic nested `def`'s body.
///
/// A nested `def` has no unique name — the same spelling may declare
/// unrelated helpers in two enclosing bodies — so the key is its declaration
/// site. The lexical nested pass derives the same key from the template it
/// registers, so the two passes agree on which bodies specialize.
fn nested_body_owner(site: &SourceSpan) -> String {
    format!(
        "{NESTED_OWNER_PREFIX}{}${}${}",
        site.source.as_deref().unwrap_or(""),
        site.span.0,
        site.span.1
    )
}

/// The stub-reaching bodies `body` can run: the callees of the references it
/// left abstract, and every stub-reaching method its by-name method edges
/// can dispatch to.
fn body_callees<'a>(
    body: &str,
    stubbed: &HashSet<&'a str>,
    uses: &'a [AbstractUse],
    edges: &'a [(String, String)],
) -> Vec<&'a str> {
    let called = uses
        .iter()
        .filter(|reference| reference.owner.as_deref() == Some(body))
        .map(|reference| reference.callee.as_str())
        .filter(|callee| stubbed.contains(callee));
    let dispatched = edges
        .iter()
        .filter(|(owner, _)| owner == body)
        .flat_map(|(_, method)| {
            stubbed
                .iter()
                .copied()
                .filter(|reached| owner_method(reached) == Some(method.as_str()))
        });
    called.chain(dispatched).collect()
}

/// What one closed instance of a generic struct mints: its per-instantiation
/// method clones, its storage types with the parameters baked, and the
/// methods it withholds — unavailable through a false `where` clause or a
/// false conditional conformance, so no call reaches their erased bodies.
#[derive(Default)]
struct InstanceClones {
    clones: Vec<mojito_ast::ast::Method>,
    field_types: Vec<Type>,
    withheld: HashSet<String>,
}

/// One reference [`Mono::retain_abstract`] left on a template's abstract
/// path.
struct AbstractUse {
    callee: String,
    site: SourceSpan,
    function_value: bool,
    owner: Option<String>,
}

/// The monomorphization worklist and its results.
#[derive(Default)]
struct Mono {
    queue: VecDeque<Job>,
    /// Mangled names already requested (dedups identical instantiations).
    done: HashSet<String>,
    /// The same dedup for an overloaded template family, where two
    /// declarations specialized at the same values share one mangled name and
    /// are told apart only by the declaration index. `done` cannot serve here:
    /// it is shared with struct instances.
    overload_done: HashSet<(String, usize)>,
    /// Generated specializations, by template name (in generation order).
    generated: HashMap<String, Vec<Stmt>>,
    /// Lexical value bindings visible while call sites are rewritten. `true`
    /// denotes a top-level specialization template; `false` is an ordinary
    /// binding that shadows a same-spelled template.
    value_scopes: Vec<HashMap<String, bool>>,
    /// Scope index of each active function/method body. Walrus bindings have
    /// function scope even when their expression occurs in a nested block.
    function_scopes: Vec<usize>,
    /// Concrete runtime-pack element types visible while scanning a generated
    /// specialization. `None` is an ordinary binding which shadows a pack of
    /// the same name; scopes mirror `value_scopes` exactly.
    runtime_pack_scopes: Vec<HashMap<String, Option<Vec<Type>>>>,
    /// Exact bare public `Tuple(...)` occurrences selected by the checker and
    /// the concrete variadic-struct symbol each one constructs.
    tuple_call_targets: HashMap<SourceSpan, String>,
    /// Exact `t"…"` occurrences selected by the checker: the concrete
    /// `TString` specialization symbol each one constructs plus the
    /// interleaved element types (an element typed `String` where the source
    /// part is an interpolation directs the rewrite to wrap that argument in
    /// a `String(...)` conversion — the snapshot for non-Copyable places).
    tstring_call_targets: HashMap<SourceSpan, TStringTarget>,
    /// Bound-generic templates with at least one reference left on the
    /// abstract path (an unresolvable call or a function-value use), and
    /// variadic struct templates applied over such a body's own symbolic
    /// parameters. The program rebuild keeps these templates alongside
    /// their specializations (a variadic template as a shell).
    retained: HashSet<String>,
    /// The type-parameter names of the declarations enclosing the walk
    /// (outermost first): a variadic template applied over one of them
    /// stays symbolic instead of failing eager specialization.
    symbolic_type_params: Vec<String>,
    /// Checker-discovered inferred bound-generic applications: call occurrence
    /// (without its syntax id) → the concrete clone that call selects.
    def_call_targets: HashMap<SourceSpan, DefCallTarget>,
    /// Checker-selected constructor rewrites: call occurrence → the struct
    /// template plus the values its specialization bakes — a scalar
    /// `range(...)` with the linked range-family template and its dtype, or a
    /// bare variadic-struct construction with the pack the checker inferred.
    /// `mono_expr` rewrites the call into that concrete constructor.
    struct_call_targets: HashMap<SourceSpan, (String, Vec<CtValue>)>,
    /// Closed applications of ordinary generic structs found while walking
    /// (annotations, constructor calls, and generated clones themselves):
    /// template → baked type values, minted as per-instantiation method
    /// clones within this elaboration. `instances_done` dedups by the
    /// mangled instance key.
    instance_jobs: VecDeque<(String, Vec<CtValue>)>,
    instances_done: HashSet<String>,
    /// Instances minted in this elaboration, reported to the driver so the
    /// checker's recordings of them do not count as new discoveries.
    minted_instances: Vec<StructInstanceRequest>,
    /// Whether the walk is inside an unstamped bundled stdlib declaration:
    /// instances reached only from there keep the erased path.
    in_bundled: bool,
    /// The body the walk is inside when that body runs only on an abstract
    /// path: a top-level bound-generic `def` by name, the erased template of
    /// a struct method as `Struct.method`, or a generic nested `def` by its
    /// [`nested_body_owner`] site. A `def`'s body runs only through a
    /// reference that stays abstract; a method's erased body only where
    /// [`Elab::unserved_template_uses`]'s table says so; a nested `def`'s
    /// body only through the instances the lexical nested pass mints.
    abstract_owner: Option<String>,
    /// How many function bodies enclose the walk. A generic `def` declared
    /// directly in one (`def_depth == 1` on entry) is what the lexical nested
    /// pass can specialize, so only that depth owns its abstract references.
    def_depth: usize,
    /// Every reference left on a bound-generic or compile-time-keyed
    /// template's abstract path, with the body it was made from.
    abstract_uses: Vec<AbstractUse>,
    /// Stub-reaching methods (`Struct.method`) an instance minted no clone
    /// for: that instance's calls keep the erased body, whose stub cannot
    /// run, so the references it holds are reported unserved.
    unclonable_methods: Vec<String>,
    /// Method calls made from an abstract body, as (owner, method name). The
    /// receiver's type is the checker's to solve, so the edge is by name: an
    /// owner that can call a stub-reaching method of that name reaches a stub
    /// itself.
    method_edges: Vec<(String, String)>,
}

impl Mono {
    /// Whether a specialization named `output_name` is new and should be
    /// queued. Two declarations of an overloaded template family
    /// specialized at the same values share one mangled name, so they dedup
    /// on the declaration index instead.
    fn queue_specialization(&mut self, output_name: &str, decl: Option<usize>) -> bool {
        match decl {
            Some(index) => self.overload_done.insert((output_name.to_string(), index)),
            None => self.done.insert(output_name.to_string()),
        }
    }

    /// Leave the call or function-value use of `template` at `site` on its
    /// abstract path.
    fn retain_abstract(&mut self, template: &str, site: &SourceSpan, function_value: bool) {
        self.retained.insert(template.to_string());
        self.abstract_uses.push(AbstractUse {
            callee: template.to_string(),
            site: site.clone(),
            function_value,
            owner: self.abstract_owner.clone(),
        });
    }

    /// Record that the body being walked can call `method` on a receiver the
    /// checker types. Only an abstract body needs the edge: a concrete one
    /// reaches its callee's clone.
    fn record_method_edge(&mut self, method: &str) {
        if let Some(owner) = &self.abstract_owner {
            self.method_edges.push((owner.clone(), method.to_string()));
        }
    }

    /// Bring a declaration's type parameters into the symbolic set for the
    /// walk of its signature and body; returns the length to truncate back
    /// to afterwards.
    fn push_symbolic_type_params(&mut self, type_params: &[TypeParam]) -> usize {
        let base = self.symbolic_type_params.len();
        self.symbolic_type_params.extend(
            type_params
                .iter()
                .map(|parameter| parameter.name.trim_start_matches('*').to_string()),
        );
        base
    }

    fn push_value_scope(&mut self) {
        self.value_scopes.push(HashMap::new());
        self.runtime_pack_scopes.push(HashMap::new());
    }

    fn pop_value_scope(&mut self) {
        self.value_scopes.pop();
        self.runtime_pack_scopes.pop();
    }

    fn push_function_scope(&mut self) {
        self.push_value_scope();
        self.function_scopes.push(self.value_scopes.len() - 1);
    }

    fn pop_function_scope(&mut self) {
        self.function_scopes.pop();
        self.pop_value_scope();
    }

    fn bind_value(&mut self, name: &str, template: bool) {
        self.value_scopes
            .last_mut()
            .expect("monomorphization always has a value scope")
            .insert(name.to_string(), template);
        self.runtime_pack_scopes
            .last_mut()
            .expect("runtime-pack scopes mirror value scopes")
            .insert(name.to_string(), None);
    }

    fn bind_parameter(&mut self, parameter: &FnParam) {
        self.bind_value(&parameter.name, false);
        let Type::Named(name, arguments) = &parameter.ty else {
            return;
        };
        if parameter.kind != ParamKind::Variadic || name != "$pack" {
            return;
        }
        let Some(types) = arguments
            .iter()
            .map(|argument| match argument {
                ParamArg::Type(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        self.runtime_pack_scopes
            .last_mut()
            .expect("runtime-pack scopes mirror value scopes")
            .insert(parameter.name.clone(), Some(types));
    }

    fn resolve_runtime_pack(&self, name: &str) -> Option<&[Type]> {
        self.runtime_pack_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .and_then(Option::as_deref)
    }

    fn resolves_top_template(&self, name: &str) -> bool {
        self.value_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .unwrap_or(false)
    }

    fn bind_named_value(&mut self, name: &str) {
        let base = self
            .function_scopes
            .last()
            .copied()
            .unwrap_or_else(|| self.value_scopes.len() - 1);
        if let Some(scope) = self.value_scopes[base..]
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            // Assigning through a walrus to a function template is a type error
            // for the checker to report. It must not remain a template here,
            // otherwise monomorphization can erase that invalid assignment.
            scope.insert(name.to_string(), false);
        } else {
            self.value_scopes[base].insert(name.to_string(), false);
        }
        if let Some(scope) = self.runtime_pack_scopes[base..]
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))
        {
            scope.insert(name.to_string(), None);
        } else {
            self.runtime_pack_scopes[base].insert(name.to_string(), None);
        }
    }
}

fn scalar_type_name(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::Int),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Some(Ty::Dtype),
        // A SIMD width parameter is a compile-time Int value parameter (the
        // removed `SIMDSize` spelling rejects).
        "SIMDLength" => Some(Ty::Int),
        "UInt" => Some(Ty::UInt),
        "Bool" => Some(Ty::Bool),
        "StringLiteral" => Some(Ty::StringLiteral),
        "Float64" => Some(Ty::Float64),
        "None" | "NoneType" => Some(Ty::None),
        // The (qualified) `String` spelling deliberately falls through to
        // ordinary struct resolution: in type-argument and type-value
        // positions it denotes the nominal stdlib struct. Value-parameter
        // classification keeps the literal type via `ct_value_param_type`.
        // The sized scalar aliases (`Int8`, `UInt64`, `Float32`, ...) are
        // width-1 SIMD types, so `comptime c_int = Int32` is a type value.
        _ => mojito_ast::ast::Dtype::from_scalar_alias(name).map(|dtype| Ty::Simd {
            dtype: mojito_types::types::SimdDtype::Known(dtype),
            width: mojito_types::types::SimdWidth::Known(1),
        }),
    }
}

fn collect_fns(program: &[Stmt]) -> HashMap<String, CtFn<'_>> {
    let mut fns = HashMap::new();
    for s in program {
        if let StmtKind::Def {
            name,
            params,
            body,
            type_params,
            ..
        } = &s.kind
        {
            fns.insert(
                name.clone(),
                CtFn {
                    ct_params: classify_ct_params(type_params, name),
                    params: params.iter().map(|p| p.name.clone()).collect(),
                    body,
                },
            );
        }
    }
    fns
}

fn collect_structs(program: &[Stmt]) -> HashMap<String, CtStruct<'_>> {
    let mut structs = HashMap::new();
    for s in program {
        if let StmtKind::Struct {
            name,
            type_params,
            associated,
            fields,
            methods,
            fieldwise_init,
            ..
        } = &s.kind
        {
            let mirrored_init = methods.iter().any(|method| {
                method.name == "__init__"
                    && method.params.len() == fields.len()
                    && method
                        .params
                        .iter()
                        .zip(fields.iter())
                        .all(|(parameter, field)| parameter.name == field.name)
            });
            structs.insert(
                name.clone(),
                CtStruct {
                    decls: classify_ct_params(type_params, name),
                    source_params: type_params,
                    associated,
                    fields,
                    fieldwise: *fieldwise_init || mirrored_init,
                },
            );
        }
    }
    structs
}

/// Collect the top-level generic `def`s that must be monomorphized (roadmap
/// milestones 6/7): a generic `def` (type and/or value parameters) whose body
/// contains a `comptime if`/`comptime for`, plus every heterogeneous type-pack
/// function.
/// Such a construct may depend on the parameters
/// (e.g. `comptime if is_same_type[T, Int]()`), so it can only be resolved per call
/// site — each specialization binds the concrete arguments and resolves the
/// comptime construct, so only the *selected* branch is type-checked. The
/// elaborator does not infer types: an inferred call to a type-pack or
/// compile-time-keyed template is served from the checker's recorded
/// instantiation, and every other such `def` needs explicit `[...]` arguments.
fn collect_specializable<'a>(
    program: &'a [Stmt],
    bound_generics: &HashSet<String>,
) -> HashMap<String, &'a Stmt> {
    let struct_names: HashSet<&str> = program
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let served_packs = served_pack_defs(program);
    let served_lanes = served_lane_defs(program);
    let mut m = HashMap::new();
    for s in program {
        if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &s.kind
            && (is_specializable_declaration_in(
                s,
                &|bound| struct_names.contains(bound),
                &served_packs,
                &served_lanes,
            ) || bound_generics.contains(name))
        {
            // An overloaded name has one entry here, the first declaration:
            // this registry answers the name-level question "is this a
            // template at all?". Which declaration of an overloaded
            // template name a call selects is
            // [`Elab::family_declaration`]'s to answer, from
            // [`collect_overload_families`].
            m.entry(name.clone()).or_insert(s);
        }
    }
    m
}

/// The declarations of every overloaded template name, in declaration order:
/// a name declared more than once with a compile-time-keyed, type-pack, or
/// `DType`-keyed declaration among them.
///
/// Overload selection is the checker's, so the elaborator cannot pick among
/// these itself: a call reaches one of them only through the checker's
/// recorded instantiation, which names the selected overload by its runtime
/// parameter names. A family may mix specialization classes — a keyed
/// declaration beside a type pack, a `DType` parameter, or a layout-dependent
/// one — because the class is a property of a declaration, not of the name:
/// the request's selected declaration index is what tells a call which class
/// serves it.
fn collect_overload_families(program: &[Stmt]) -> HashMap<String, Vec<&Stmt>> {
    let mut families: HashMap<String, Vec<&Stmt>> = HashMap::new();
    for statement in program {
        if let StmtKind::Def { name, .. } = &statement.kind {
            families.entry(name.clone()).or_default().push(statement);
        }
    }
    let served_packs = served_pack_defs(program);
    families.retain(|_, declarations| {
        declarations.len() > 1
            && declarations.iter().any(|s| {
                comptime_keyed_declaration(s)
                    || (pack_keyed_declaration(s) && !pack_def_template_served(s, &served_packs))
                    || dtype_keyed_declaration(s)
            })
    });
    families
}

/// Whether `statement`'s caller-visible runtime parameters are exactly
/// `parameter_names`.
///
/// This must agree with how the checker builds `Ty::GenericFunc::names`
/// (`caller_regular`, `checker/statements.rs`): regular parameters in
/// declaration order, with an `out` named result excluded, since a caller
/// never supplies one.
fn declaration_takes_names(statement: &Stmt, parameter_names: &[String]) -> bool {
    declaration_takes(statement, parameter_names, |parameter| {
        parameter.name.clone()
    })
}

/// Whether `statement`'s caller-visible parameters are declared with exactly
/// `parameter_types`, mangled the way the checker mangles the resolved types it
/// recorded. [`mojito_symbol::symbol::TypeKey`] aligns its declaration and
/// call-resolution sides precisely so these compare.
fn declaration_takes_types(statement: &Stmt, parameter_types: &[String]) -> bool {
    let StmtKind::Def { type_params, .. } = &statement.kind else {
        return false;
    };
    declaration_takes(statement, parameter_types, |parameter| {
        mojito_symbol::symbol::TypeKey::from_ast_in_scope(&parameter.ty, type_params)
            .as_str()
            .to_string()
    })
}

/// Whether `statement`'s `*args` collector is `variadic`: the same position
/// among its caller-visible parameters and the same element key, or no
/// collector on either side.
fn declaration_takes_variadic(
    statement: &Stmt,
    variadic: Option<&mojito_symbol::symbol::VariadicKey>,
) -> bool {
    let StmtKind::Def {
        params,
        type_params,
        ..
    } = &statement.kind
    else {
        return false;
    };
    mojito_symbol::symbol::VariadicKey::from_ast_params(params, type_params).as_ref() == variadic
}

fn declaration_takes(
    statement: &Stmt,
    expected: &[String],
    spell: impl Fn(&mojito_ast::ast::FnParam) -> String,
) -> bool {
    let StmtKind::Def { params, .. } = &statement.kind else {
        return false;
    };
    let mut caller_visible = params
        .iter()
        .filter(|parameter| {
            parameter.kind == mojito_ast::ast::ParamKind::Regular
                && !matches!(parameter.convention, Some(ArgConvention::Out))
        })
        .map(spell);
    expected
        .iter()
        .all(|wanted| caller_visible.next().as_ref() == Some(wanted))
        && caller_visible.next().is_none()
}

/// Whether a top-level `def` keys a lane on a `DType` parameter of its own —
/// the `DType`-keyed class's per-declaration predicate. Such a signature
/// stands in as a checkable stub: a `Scalar[dt]` slot validates symbolically.
fn dtype_keyed_declaration(statement: &Stmt) -> bool {
    let StmtKind::Def { type_params, .. } = &statement.kind else {
        return false;
    };
    type_params
        .iter()
        .any(|parameter| matches!(parameter.bounds.as_slice(), [only] if only == "DType"))
}

/// Whether a top-level `def` is specializable only because its body holds
/// compile-time control flow or a `rebind` over its own parameters — the
/// compile-time-keyed class's per-declaration predicate.
fn comptime_keyed_declaration(statement: &Stmt) -> bool {
    let StmtKind::Def {
        type_params,
        params,
        body,
        ..
    } = &statement.kind
    else {
        return false;
    };
    def_body_keys_specialization(body, &def_pack_names(type_params, params))
        && admits_comptime_keying(statement)
        && type_params
            .iter()
            .any(|parameter| !retained_specialization_param(parameter, type_params))
}

/// The nested form of [`is_specializable_declaration`]: a nested `def`
/// holding a `comptime if` still clones per call, since its body is minted
/// with the enclosing clone (roadmap: nested definitions over compile-time
/// parameters).
pub(super) fn is_specializable_nested_declaration(statement: &Stmt) -> bool {
    is_specializable_declaration(statement, &HashSet::new(), &HashSet::new())
        || matches!(&statement.kind, StmtKind::Def { type_params, body, .. }
            if !type_params.is_empty()
                && (block_has_comptime(body)
                    || type_params.iter().any(|parameter| parameter.name.starts_with('*'))))
}

/// Whether every compile-time parameter of a `def` is one its template
/// serves: a type parameter — a type pack included, which the elaborator
/// binds from the call's recorded elements — or a scalar (`Int`, `Bool`,
/// `DType`) value parameter that a runtime parameter type names only as a
/// vector's lane slot (`a: Scalar[dt]`, `v: SIMD[dt, width]`), if at all, so
/// the elaborator binds it from the call's recorded arguments or from the
/// argument's slot. A value a call must infer from any other argument type
/// keeps the clone until the elaborator binds one from the call.
pub(super) fn template_serves_binders(
    type_params: &[TypeParam],
    params: &[FnParam],
    owner: &str,
) -> bool {
    !type_params.is_empty()
        && type_params.iter().all(|parameter| {
            match classify_ct_param(parameter, type_params, owner) {
                Some(ParamDecl::Type { .. }) => true,
                Some(ParamDecl::Value {
                    ty,
                    variadic: false,
                    ..
                }) => {
                    matches!(ty.as_ref(), Ty::Int | Ty::Bool | Ty::Dtype)
                        && !params
                            .iter()
                            .any(|param| type_names_outside_lanes(&param.ty, &parameter.name))
                }
                _ => false,
            }
        })
}

/// [`type_names`], except inside the arguments of a `SIMD` or `Scalar` type,
/// whose slots the elaborator binds from the argument's own slots.
fn type_names_outside_lanes(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Named(head, _) if head == "SIMD" || head == "Scalar" => false,
        Type::Named(_, arguments) => arguments.iter().any(|argument| match argument {
            ParamArg::Type(inner) => type_names_outside_lanes(inner, name),
            ParamArg::Value(value) => expr_names(value, name),
            ParamArg::Named { value, .. } => match value.as_ref() {
                ParamArg::Type(inner) => type_names_outside_lanes(inner, name),
                ParamArg::Value(inner) => expr_names(inner, name),
                ParamArg::Named { .. } => type_names(ty, name),
            },
        }),
        other => type_names(other, name),
    }
}

/// Whether the expression `expr` reads the identifier `name`.
fn expr_names(expr: &Expr, name: &str) -> bool {
    struct Finder<'a> {
        name: &'a str,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if matches!(&expr.kind, ExprKind::Identifier(found) if found == self.name) {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { name, found: false };
    mojito_ast::visit::walk_expr(&mut finder, expr);
    finder.found
}

/// Whether the annotation `ty` spells `name`, as a type or in a value slot.
fn type_names(ty: &Type, name: &str) -> bool {
    struct Finder<'a> {
        name: &'a str,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if matches!(&expr.kind, ExprKind::Identifier(found) if found == self.name) {
                self.found = true;
            }
        }

        fn visit_type(&mut self, ty: &Type) {
            if matches!(ty, Type::Named(found, _) | Type::SelfParam(found) if found == self.name) {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { name, found: false };
    mojito_ast::visit::walk_type(&mut finder, ty);
    finder.found
}

/// Whether a declaration's own parameters permit the compile-time-keyed class.
/// A pack, a `DType` parameter, or a layout-dependent parameter keeps its own
/// specialization path: such a signature cannot stand in as a checkable stub.
fn admits_comptime_keying(statement: &Stmt) -> bool {
    let StmtKind::Def { type_params, .. } = &statement.kind else {
        return false;
    };
    !type_params.iter().any(|parameter| {
        parameter.name.starts_with('*')
            || matches!(parameter.bounds.as_slice(), [only] if only == "DType")
    }) && !def_uses_layout_dependent_param(statement)
}

/// How many top-level `def`s share each name: the name-keyed template classes
/// admit only a unique name, since overload selection is the checker's.
fn def_name_counts(program: &[Stmt]) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for statement in program {
        if let StmtKind::Def { name, .. } = &statement.kind {
            *counts.entry(name.as_str()).or_default() += 1;
        }
    }
    counts
}

/// Top-level type-pack templates: a `def` with a `*Ts` type parameter (see
/// [`pack_generic_template_names`]) whose template does not serve it
/// ([`pack_def_template_served`]). Value packs stay on the syntactic (hard)
/// specialization path. An overloaded name is a template family
/// ([`collect_overload_families`]), whose request path tells its declarations
/// apart, since overload selection is the checker's.
fn collect_pack_generic_templates(program: &[Stmt]) -> HashSet<String> {
    let served_packs = served_pack_defs(program);
    program
        .iter()
        .filter(|statement| {
            pack_keyed_declaration(statement) && !pack_def_template_served(statement, &served_packs)
        })
        .filter_map(|statement| match &statement.kind {
            StmtKind::Def { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Whether a top-level `def` is a type-pack template: it declares a `*Ts` type
/// parameter — the type-pack class's per-declaration predicate.
fn pack_keyed_declaration(statement: &Stmt) -> bool {
    let StmtKind::Def {
        name, type_params, ..
    } = &statement.kind
    else {
        return false;
    };
    type_params.iter().any(|parameter| {
        matches!(
            classify_ct_param(parameter, type_params, name),
            Some(ParamDecl::Type { variadic: true, .. })
        )
    })
}

/// Top-level compile-time-keyed templates (see
/// [`comptime_generic_template_names`]): a uniquely named `def` specializable
/// only because its body holds compile-time control flow or a `rebind`
/// assertion over its own parameters. Packs, `DType`
/// parameters, and SIMD-width parameters stay on their own paths: their
/// signatures cannot stand in as a checkable stub.
fn collect_dtype_generic_templates(program: &[Stmt]) -> HashSet<String> {
    let families = collect_overload_families(program);
    let def_counts = def_name_counts(program);
    let served_lanes = served_lane_defs(program);
    program
        .iter()
        .filter_map(|statement| {
            let StmtKind::Def { name, .. } = &statement.kind else {
                return None;
            };
            let admitted = (def_counts[name.as_str()] == 1 || families.contains_key(name.as_str()))
                && dtype_keyed_declaration(statement)
                && !served_lanes.contains(name);
            admitted.then(|| name.clone())
        })
        .collect()
}

fn collect_comptime_generic_templates(program: &[Stmt]) -> HashSet<String> {
    let families = collect_overload_families(program);
    let def_counts = def_name_counts(program);
    program
        .iter()
        .filter_map(|statement| {
            let StmtKind::Def { name, .. } = &statement.kind else {
                return None;
            };
            // A unique name joins on its own declaration; an overloaded name
            // joins as a family, whose members the request path tells apart.
            let admitted = (def_counts[name.as_str()] == 1 || families.contains_key(name.as_str()))
                && comptime_keyed_declaration(statement);
            admitted.then(|| name.clone())
        })
        .collect()
}

/// Top-level trait-bound generic `def`s with no comptime constructs. These
/// monomorphize per explicit concrete application like the comptime class, but
/// resolution is soft — an unresolvable call (inference, symbolic arguments)
/// stays on the template's abstract erased-dispatch path — and the template
/// survives whenever any reference stays abstract or none exists, keeping the
/// Mojo-style pre-check of the uninstantiated body. An overloaded name stays
/// entirely on the abstract path: the registry is name-keyed and overload
/// selection is the checker's.
fn collect_bound_generic_templates(program: &[Stmt]) -> HashSet<String> {
    let def_counts = def_name_counts(program);
    let served_packs = served_pack_defs(program);
    let served_lanes = served_lane_defs(program);
    program
        .iter()
        .filter_map(|statement| {
            let StmtKind::Def {
                name, type_params, ..
            } = &statement.kind
            else {
                return None;
            };
            let StmtKind::Def { params, .. } = &statement.kind else {
                return None;
            };
            if is_specializable_declaration(statement, &served_packs, &served_lanes)
                || def_counts[name.as_str()] != 1
            {
                return None;
            }
            let has_type_binder = type_params.iter().any(|parameter| {
                matches!(
                    classify_ct_param(parameter, type_params, name),
                    Some(ParamDecl::Type {
                        variadic: false,
                        ..
                    })
                )
            });
            (has_type_binder || template_serves_binders(type_params, params, name))
                .then(|| name.clone())
        })
        .collect()
}

/// Whether a block directly contains a statement `wanted` accepts, under the
/// same scope rule as `block_has_comptime`.
fn block_has_statement(stmts: &[Stmt], wanted: &dyn Fn(&StmtKind) -> bool) -> bool {
    let has = |block: &[Stmt]| block_has_statement(block, wanted);
    stmts.iter().any(|s| match &s.kind {
        kind if wanted(kind) => true,
        StmtKind::If { branches, orelse } => {
            branches.iter().any(|(_, b)| has(b)) || orelse.as_ref().is_some_and(|b| has(b))
        }
        StmtKind::While { body, .. } | StmtKind::For { body, .. } => has(body),
        StmtKind::With { body, .. } => has(body),
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            has(body)
                || except.as_ref().is_some_and(|(_, b)| has(b))
                || orelse.as_ref().is_some_and(|b| has(b))
                || finalbody.as_ref().is_some_and(|b| has(b))
        }
        _ => false,
    })
}

/// A pending specialization request: template `orig`, specialized for `vals`.
struct Job {
    orig: String,
    /// The selected declaration of an overloaded template name; see
    /// [`DefCallTarget::decl`]. The drain resolves the template through it,
    /// since `orig` names a whole family.
    decl: Option<usize>,
    vals: Vec<CtValue>,
    site: String,
    output_name: String,
    whole_pack_abi: bool,
}

fn source_type_from_ty(ty: &Ty) -> Option<Type> {
    source_type_from_ty_with_origins(ty, &HashMap::new(), &[])
}

/// The concrete call-site information used to select one function-template
/// specialization. Nested pack forwarding supplies its already-known element
/// types; ordinary calls leave that field empty and infer from expressions.
#[derive(Clone, Copy)]
struct SpecRequest<'a> {
    param_args: &'a [ParamArg],
    call_args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
    consts: &'a HashMap<String, CtValue>,
    request_site: &'a str,
    forwarded_pack_types: Option<&'a [Ty]>,
}

fn lit_result(val: &CtValue, span: Span) -> Result<Expr, ComptimeError> {
    val.materialize(span).ok_or_else(|| {
        ComptimeError::NotComptime(
            "type-valued or symbolic comptime values cannot materialize at runtime".to_string(),
        )
    })
}

/// The builtins with an observable effect, which no compile-time evaluation
/// may reach: everything else the VM executes deterministically.
fn vm_ctfe_effectful_builtin(name: &str) -> bool {
    matches!(name, "print" | "input")
}

mod ctfe;

mod eval;

mod mono;

mod nested;

mod rewrite;

mod specialize;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rewrite::*;

impl<'a> Elab<'a> {
    /// The one declaration of overloaded template `name` whose
    /// runtime parameters are `parameter_names`, with its index.
    ///
    /// The checker records the selected overload's parameter names in
    /// declaration order, excluding an `out` named result, so this is how a
    /// request names one overload of a template. Overloads that share a
    /// parameter-name list are told apart by their mangled parameter types.
    /// A variadic parameter is caller-visible but is named by neither key, so
    /// overloads that share both are told apart by their `*args` collector —
    /// its position and element key — and then by whether the request's own
    /// arguments bind the declaration's parameters at all. A family ambiguous
    /// under all four leaves the call abstract, and the driver's unserved-use
    /// check rejects it in the caller's own terms.
    pub(super) fn family_declaration(
        &self,
        name: &str,
        request: &DefSpecializationRequest,
    ) -> Option<(usize, &'a Stmt)> {
        let declarations = self.overload_families.get(name)?;
        let by_name: Vec<(usize, &'a Stmt)> = declarations
            .iter()
            .enumerate()
            .filter(|(_, declaration)| {
                declaration_takes_names(declaration, request.parameter_names())
            })
            .map(|(index, declaration)| (index, *declaration))
            .collect();
        let candidates = match by_name.as_slice() {
            [only] => return Some(*only),
            [] => return None,
            _ => by_name,
        };
        let by_type: Vec<(usize, &'a Stmt)> = candidates
            .iter()
            .copied()
            .filter(|(_, declaration)| {
                declaration_takes_types(declaration, request.parameter_types())
            })
            .collect();
        let candidates = match by_type.as_slice() {
            [only] => return Some(*only),
            [] => candidates,
            _ => by_type,
        };
        let by_collector: Vec<(usize, &'a Stmt)> = candidates
            .iter()
            .copied()
            .filter(|(_, declaration)| declaration_takes_variadic(declaration, request.variadic()))
            .collect();
        let candidates = match by_collector.as_slice() {
            [only] => return Some(*only),
            [] => candidates,
            _ => by_collector,
        };
        let mut by_shape = candidates.into_iter().filter(|(_, declaration)| {
            self.def_request_values(declaration, request.arguments())
                .is_some()
        });
        let only = by_shape.next()?;
        by_shape.next().is_none().then_some(only)
    }

    /// Whether `statement` shares an overloaded template name
    /// without being a template of any class itself — a plain
    /// `def kind(a: Int, b: Int)` beside a `def kind[T](a: T)` whose body holds
    /// a `comptime if`.
    ///
    /// Such a declaration specializes nothing: the walk and the program
    /// rebuild must treat it as an ordinary statement, since both otherwise
    /// decide by name alone and would drop it. A sibling that *is* a template
    /// of some other class — a type pack beside the keyed declaration — must
    /// not answer yes here, or the rebuild would push its unspecialized body
    /// through verbatim.
    pub(super) fn shares_a_family_name(&self, statement: &Stmt) -> bool {
        let StmtKind::Def { name, .. } = &statement.kind else {
            return false;
        };
        self.overload_families.contains_key(name)
            && !is_specializable_declaration(statement, &self.served_packs, &self.served_lanes)
    }

    /// Whether `name` is an overloaded template family: a call to it
    /// is served only from the checker's recorded instantiation, never
    /// resolved syntactically.
    pub(super) fn overload_family(&self, name: &str) -> bool {
        self.overload_families.contains_key(name)
    }

    /// The declaration a job or call target selected, or the sole declaration
    /// the name-keyed registry holds.
    pub(super) fn selected_declaration(&self, name: &str, decl: Option<usize>) -> &'a Stmt {
        decl.and_then(|index| self.overload_families.get(name)?.get(index).copied())
            .unwrap_or_else(|| self.specializable[name])
    }

    /// Whether `name` declares an explicit (non-infer-only) `Origin`/
    /// `OriginSet` parameter — a slot a generated clone can spell only as a
    /// binder of its own (`Elab::clone_binding`).
    pub(super) fn struct_has_explicit_origin_slots(&self, name: &str) -> bool {
        !self.explicit_origin_slots(name).is_empty()
    }

    /// The explicit (non-infer-only) `Origin`/`OriginSet` parameters of the
    /// struct `name`, in the order of a `Ty::Struct`'s origin tail.
    pub(super) fn explicit_origin_slots(&self, name: &str) -> Vec<&'a TypeParam> {
        self.structs
            .get(name)
            .map(|declaration| {
                declaration
                    .source_params
                    .iter()
                    .filter(|parameter| {
                        !parameter.infer_only
                            && matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a checked type mentions an origin-slotted struct applied to
    /// non-origin arguments anywhere (`_ListIter[Int]`, `Named[Int]`,
    /// `Span[Int, o]`): an inferred type argument of that shape spells its
    /// slots only through a clone's own binders (`Elab::clone_binding`), and
    /// otherwise keeps its call on the abstract path (origin-carrying
    /// references do the same). A struct whose only explicit parameters are
    /// origins (`RefIter`, `RefBox`) spells bare, which infers per call, so
    /// it specializes.
    /// A pointer whose origin names a caller place (`Pointer[Int,
    /// origin_of(x)]`), or the clone binder standing for one, counts as
    /// such a slot too.
    pub(super) fn ty_mentions_origin_slotted_struct(&self, ty: &Ty) -> bool {
        mojito_types::types::mentions(ty, &|candidate| match candidate {
            Ty::Struct(name, arguments) => {
                !arguments.is_empty() && self.struct_has_explicit_origin_slots(name)
            }
            Ty::Pointer { origin, .. } => {
                origin.clone_bindable_place().is_some()
                    || matches!(origin, mojito_types::origin::PointerOrigin::Param { id, .. }
                        if CloneOriginBinders::name(*id).is_some())
            }
            _ => false,
        })
    }

    /// The clone binders the bound values of a `def` clone name
    /// (`Elab::clone_binding`), declared in binder order.
    /// The binders in `explicit` are declared explicit: no argument spells
    /// them, so each call supplies them (`Elab::unspelled_clone_binders`).
    pub(super) fn clone_origin_binder_params(
        &self,
        values: &[CtValue],
        explicit: &[u32],
    ) -> Vec<TypeParam> {
        let mut found = Vec::new();
        for value in values {
            if let CtValue::Type(ty) = value {
                self.collect_clone_binders(ty, &mut found);
            }
        }
        found.sort_by_key(|(index, _)| *index);
        found.dedup_by_key(|(index, _)| *index);
        found
            .into_iter()
            .flat_map(|(index, mutability)| {
                let mut declared = CloneOriginBinders::declared(index, mutability.as_ref());
                if let Some(origin) = declared.last_mut() {
                    origin.infer_only = !explicit.contains(&index);
                }
                declared
            })
            .collect()
    }

    /// A type argument a generated clone bakes, as the clone binds and
    /// spells it. An origin-slotted struct applied with its origin tail
    /// (`Span[Int, o]`) rebinds each slot to a fresh binder `binders` declares
    /// on the clone (`Span[Int, __clone_origin0]`), inferred per call from the
    /// clone's own parameters as the template's type parameter was: the
    /// checker's instantiation records erase every place origin, so one clone
    /// serves every origin of that shape. A slot still naming a method's
    /// receiver origin (`l.__iter__()` recorded as `_ListIter[T,
    /// origin_of(self)]`) names some caller place, so it binds as one.
    /// `None` keeps the erased path: a struct applied without its tail
    /// (`_ListIter[Int]`) or an `OriginSet` slot has no binder to stand for
    /// it, nor has a slot already bound to an enclosing declaration's origin
    /// parameter unless `binders` admits one
    /// ([`CloneOriginBinders::over_enclosing`]).
    pub(super) fn clone_binding(
        &self,
        ty: &Ty,
        binders: &mut CloneOriginBinders,
    ) -> Option<(Ty, Type)> {
        let bound = if self.ty_mentions_origin_slotted_struct(ty) {
            self.bind_clone_origins(ty, binders)?
        } else {
            ty.clone()
        };
        let source = source_type_from_ty(&bound)?;
        Some((bound, source))
    }

    /// Whether syntactically guessed pack element types are the whole truth:
    /// a bare generic struct name (`Box(7)` guessed as `Box`, `Named("k",
    /// w)` as `Named`) hides arguments only the checker can solve, so the
    /// call defers to the checker-recorded instantiation instead.
    pub(super) fn pack_values_statically_evident(&self, values: &[CtValue]) -> bool {
        values.iter().all(|value| match value {
            CtValue::Tuple(elements) => elements.iter().all(|element| match element {
                CtValue::Type(ty) => !mojito_types::types::mentions(ty, &|candidate| {
                    matches!(
                        candidate,
                        Ty::Struct(name, arguments)
                            if arguments.is_empty()
                                && self.structs.get(name).is_some_and(|declaration| {
                                    !declaration.decls.is_empty()
                                        || self.struct_has_explicit_origin_slots(name)
                                })
                    )
                }),
                _ => true,
            }),
            _ => true,
        })
    }

    /// The source spelling of a heterogeneous pack element type, with every
    /// origin slot the checker erased spelled as upstream's `_` placeholder
    /// (`Named[Int]` → `Named[Int, _]`): the specialized `$pack` parameter
    /// annotation resolves in parameter position, where a placeholder marks
    /// the slot explicitly inferred. A struct with only origin slots stays
    /// bare (`RefBox`, not `RefBox[_]`): the bare spelling infers in every
    /// position a specialized `Tuple` element occupies, the placeholder only
    /// in parameter position.
    pub(super) fn pack_element_source_type(&self, ty: &Ty) -> Option<Type> {
        source_type_from_ty(ty).map(|source| self.insert_origin_placeholders(source))
    }

    /// See [`Self::pack_element_source_type`]; walks nested applications.
    pub(super) fn insert_origin_placeholders(&self, source: Type) -> Type {
        let Type::Named(name, arguments) = source else {
            return source;
        };
        let arguments: Vec<ParamArg> = arguments
            .into_iter()
            .map(|argument| match argument {
                ParamArg::Type(inner) => ParamArg::Type(self.insert_origin_placeholders(inner)),
                other => other,
            })
            .collect();
        let is_origin = |parameter: &TypeParam| matches!(parameter.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet");
        let Some(declaration) = self.structs.get(&name) else {
            return Type::Named(name, arguments);
        };
        let explicit: Vec<&TypeParam> = declaration
            .source_params
            .iter()
            .filter(|parameter| !parameter.infer_only)
            .collect();
        let non_origin = explicit
            .iter()
            .filter(|parameter| !is_origin(parameter))
            .count();
        if non_origin == explicit.len()
            || non_origin == 0
            || arguments.len() != non_origin
            || arguments
                .iter()
                .any(|argument| matches!(argument, ParamArg::Named { .. }))
        {
            return Type::Named(name, arguments);
        }
        let mut positional = arguments.into_iter();
        let filled = explicit
            .iter()
            .map(|parameter| {
                if is_origin(parameter) {
                    ParamArg::Value(Expr::new(ExprKind::Identifier("_".to_string()), (0, 0)))
                } else {
                    positional
                        .next()
                        .expect("argument count equals the non-origin explicit count")
                }
            })
            .collect();
        Type::Named(name, filled)
    }
}

#[cfg(test)]
mod vm_bridge_tests {
    use super::{ct_to_vm, vm_to_ct};
    use mojito::{CtValue, Value};

    #[test]
    fn list_values_cross_vm_ctfe_only_as_explicit_comptime_storage() {
        let source = CtValue::List(vec![CtValue::Int(1), CtValue::Bool(true)]);
        let runtime = ct_to_vm(&source).expect("compile-time list crosses into VM CTFE");
        assert!(matches!(
            &runtime,
            Value::ComptimeList(values)
                if values == &[Value::Int(1), Value::Bool(true)]
        ));
        assert_eq!(
            vm_to_ct(runtime).expect("VM CTFE list crosses back to CtValue"),
            source
        );
    }
}

/// The request-driven elaboration of an unprepared program, for the unit
/// tests below: prepare, then elaborate.
#[cfg(test)]
fn elaborate_with_requests(
    program: Vec<Stmt>,
    tuple_requests: &[TupleSpecializationRequest],
    tstring_requests: &[TStringSpecializationRequest],
    def_requests: &[DefSpecializationRequest],
    method_requests: &[MethodSpecializationRequest],
    struct_requests: &[StructInstanceRequest],
    hash_leaf_types: &[Ty],
) -> Result<Elaborated, ComptimeError> {
    elaborate_prepared(
        &prepare(program)?,
        ElaborationInputs {
            tuple_requests,
            tstring_requests,
            def_requests,
            method_requests,
            struct_requests,
            hash_leaf_types,
            ..ElaborationInputs::default()
        },
    )
}

#[cfg(test)]
mod tuple_request_tests {
    use super::{TupleSpecializationRequest, elaborate_with_requests, tuple_specialization_symbol};
    use mojito::{Ty, parse};
    use mojito_ast::ast::{ExprKind, StmtKind};
    use mojito_types::types::tuple_type;

    const TEMPLATE: &str =
        "struct Tuple[*Ts: AnyType]:\n    var storage: __RuntimeTuple[*Self.Ts]\n\n";

    fn bare_call(program: &[mojito_ast::ast::Stmt]) -> &mojito_ast::ast::Expr {
        program
            .iter()
            .find_map(|statement| match &statement.kind {
                StmtKind::Def { name, body, .. } if name == "main" => {
                    body.iter().find_map(|statement| match &statement.kind {
                        StmtKind::VarDecl { value, .. }
                            if matches!(&value.kind, ExprKind::Call { name, param_args, .. }
                                if name == "Tuple" && param_args.is_empty()) =>
                        {
                            Some(value)
                        }
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("test program contains one bare Tuple call")
    }

    fn struct_names(program: &[mojito_ast::ast::Stmt]) -> Vec<&str> {
        program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn checked_int_string_request_rewrites_only_its_bare_tuple_call() {
        let source = format!("{TEMPLATE}def main():\n    var value = Tuple(1, \"two\")\n");
        let parsed = parse(&source).expect("parse Tuple request fixture");
        let occurrence = bare_call(&parsed).source_span();
        let elements = vec![Ty::Int, Ty::StringLiteral];
        let expected = tuple_specialization_symbol(&elements);

        let elaborated = elaborate_with_requests(
            parsed,
            &[TupleSpecializationRequest::bare_call(elements, occurrence)],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect("materialize checked Tuple specialization")
        .program;

        assert!(struct_names(&elaborated).contains(&expected.as_str()));
        let rewritten = elaborated
            .iter()
            .find_map(|statement| match &statement.kind {
                StmtKind::Def { name, body, .. } if name == "main" => {
                    body.iter().find_map(|statement| match &statement.kind {
                        StmtKind::VarDecl { value, .. } => Some(value),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("rewritten initializer");
        assert!(
            matches!(&rewritten.kind, ExprKind::Call { name, param_args, .. }
            if name == &expected && param_args.is_empty())
        );
    }

    #[test]
    fn context_free_request_materializes_declaration_without_rewriting_bare_call() {
        let source = format!("{TEMPLATE}def main():\n    var value = Tuple(1, 2)\n");
        let parsed = parse(&source).expect("parse Tuple request fixture");
        let elements = vec![Ty::Int, Ty::Int];
        let expected = tuple_specialization_symbol(&elements);

        let elaborated = elaborate_with_requests(
            parsed,
            &[TupleSpecializationRequest::declaration(elements)],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect("materialize contextual Tuple declaration")
        .program;

        assert!(struct_names(&elaborated).contains(&expected.as_str()));
        assert_eq!(
            match &bare_call(&elaborated).kind {
                ExprKind::Call { name, .. } => name,
                _ => unreachable!("helper selected a Call"),
            },
            "Tuple",
            "an unhinted bare call must survive for the next discovery check"
        );
    }

    #[test]
    fn nested_tuple_request_seeds_inner_and_outer_specializations() {
        let parsed = parse(TEMPLATE).expect("parse Tuple template");
        let inner = tuple_type(vec![Ty::Int]);
        let outer_elements = vec![inner, Ty::StringLiteral];
        let inner_symbol = tuple_specialization_symbol(&[Ty::Int]);
        let outer_symbol = tuple_specialization_symbol(&outer_elements);

        let elaborated = elaborate_with_requests(
            parsed,
            &[TupleSpecializationRequest::declaration(outer_elements)],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect("materialize nested Tuple specializations")
        .program;
        let names = struct_names(&elaborated);

        assert!(names.contains(&inner_symbol.as_str()), "{names:?}");
        assert!(names.contains(&outer_symbol.as_str()), "{names:?}");
    }
}

#[cfg(test)]
mod def_request_tests {
    use super::{DefSpecializationRequest, elaborate_with_requests};
    use mojito::{Ty, parse};
    use mojito_ast::ast::{ExprKind, StmtKind};
    use mojito_types::ct::CtValue;
    use mojito_types::types::TyArg;

    const TEMPLATE: &str = "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\n";

    /// The span of the one inferred (argument-less `[...]`) call to `callee`
    /// inside `main`.
    fn inferred_call_span(
        program: &[mojito_ast::ast::Stmt],
        callee: &str,
    ) -> mojito_common::token::SourceSpan {
        fn find(
            expr: &mojito_ast::ast::Expr,
            callee: &str,
        ) -> Option<mojito_common::token::SourceSpan> {
            let ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } = &expr.kind
            else {
                return None;
            };
            if name == callee && param_args.is_empty() {
                return Some(expr.source_span());
            }
            args.iter().find_map(|argument| find(argument, callee))
        }
        program
            .iter()
            .find_map(|statement| match &statement.kind {
                StmtKind::Def { name, body, .. } if name == "main" => {
                    body.iter().find_map(|statement| match &statement.kind {
                        StmtKind::Expr(value) => find(value, callee),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("test program contains the inferred call")
    }

    fn def_names(program: &[mojito_ast::ast::Stmt]) -> Vec<&str> {
        program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    fn main_call_names(program: &[mojito_ast::ast::Stmt]) -> Vec<String> {
        fn collect(expr: &mojito_ast::ast::Expr, out: &mut Vec<String>) {
            if let ExprKind::Call { name, args, .. } = &expr.kind {
                out.push(name.clone());
                for argument in args {
                    collect(argument, out);
                }
            }
        }
        let mut out = Vec::new();
        for statement in program {
            if let StmtKind::Def { name, body, .. } = &statement.kind
                && name == "main"
            {
                for statement in body {
                    if let StmtKind::Expr(value) = &statement.kind {
                        collect(value, &mut out);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn closed_request_on_a_template_served_def_mints_no_clone() {
        let source = format!("{TEMPLATE}def main():\n    print(ident(2))\n");
        let parsed = parse(&source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "ident");
        let request = DefSpecializationRequest::new(
            occurrence,
            "ident".to_string(),
            vec!["x".to_string()],
            vec!["T".to_string()],
            vec![TyArg::Ty(Ty::Int)],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[], &[])
            .expect("a request on a template-served def must not fail elaboration")
            .program;

        // A plain trait-bound `def` is served by its template, so the
        // request is skipped and the call keeps naming the template.
        let defs = def_names(&elaborated);
        assert!(defs.contains(&"ident"), "{defs:?}");
        assert!(
            !defs.iter().any(|name| name.starts_with("ident$")),
            "{defs:?}"
        );
        assert!(main_call_names(&elaborated).contains(&"ident".to_string()));
    }

    #[test]
    fn a_request_on_a_template_served_pack_overload_mints_no_clone() {
        let source = "def pos[*Ts: Writable](a: Int, *rest: *Ts) -> Int:\n    return 1\n\n\
                      def pos[*Ts: Writable](*rest: *Ts, a: Int) -> Int:\n    return 2\n\n\
                      def main():\n    print(pos(7, a=1))\n";
        let parsed = parse(source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "pos");
        let StmtKind::Def {
            params,
            type_params,
            ..
        } = &parsed[1].kind
        else {
            panic!("the second declaration is a def");
        };
        let request = DefSpecializationRequest::new(
            occurrence,
            "pos".to_string(),
            vec!["a".to_string()],
            vec!["Int".to_string()],
            vec![TyArg::Val(CtValue::Tuple(vec![CtValue::Type(Box::new(
                Ty::Int,
            ))]))],
        )
        .with_variadic(mojito_symbol::symbol::VariadicKey::from_ast_params(
            params,
            type_params,
        ));

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[], &[])
            .expect("materialize the requested specialization")
            .program;

        // Both pack-keyed overloads are served by their templates, so the
        // request selects a declaration but mints no clone of it.
        let defs = def_names(&elaborated);
        assert!(
            !defs.iter().any(|name| name.starts_with("pos$")),
            "{defs:?}"
        );
    }

    #[test]
    fn misaligned_request_is_skipped_and_the_template_retained() {
        let source = format!("{TEMPLATE}def main():\n    print(ident(2))\n");
        let parsed = parse(&source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "ident");
        // A value argument cannot bind the type parameter `T`.
        let request = DefSpecializationRequest::new(
            occurrence,
            "ident".to_string(),
            vec!["x".to_string()],
            vec!["T".to_string()],
            vec![TyArg::Val(CtValue::Int(1))],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[], &[])
            .expect("a skipped request must not fail elaboration")
            .program;

        let defs = def_names(&elaborated);
        assert!(defs.contains(&"ident"), "{defs:?}");
        assert!(
            !defs.iter().any(|name| name.starts_with("ident$")),
            "{defs:?}"
        );
        assert!(main_call_names(&elaborated).contains(&"ident".to_string()));
    }

    #[test]
    fn requested_and_explicit_applications_of_a_template_served_def_mint_no_clone() {
        let source =
            format!("{TEMPLATE}def main():\n    print(ident[Int](1))\n    print(ident(2))\n");
        let parsed = parse(&source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "ident");
        let request = DefSpecializationRequest::new(
            occurrence,
            "ident".to_string(),
            vec!["x".to_string()],
            vec!["T".to_string()],
            vec![TyArg::Ty(Ty::Int)],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[], &[])
            .expect("materialize the requested specialization")
            .program;

        // The template serves the explicit and the inferred application
        // alike, so neither mints a clone.
        let defs = def_names(&elaborated);
        assert!(defs.contains(&"ident"), "{defs:?}");
        assert!(
            !defs.iter().any(|name| name.starts_with("ident$")),
            "{defs:?}"
        );
    }
}
