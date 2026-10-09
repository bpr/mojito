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
    Expr, ExprKind, FnParam, InfixOp, ParamArg, ParamKind, PrefixOp, Stmt, StmtKind,
    StructComptime, TStringPart, Type, TypeParam, WithItem,
};
pub use mojito_symbol::symbol::mangle;

use mojito_ast::call::{CallVariadics, effective_keyword_only_index, match_call_slots};
use mojito_common::token::{SourceSpan, Span, SyntaxId};
use mojito_types::ct::{CtMarker, CtValue};
use mojito_types::param_expr::{ParamContext, ParamError, ParamExpr};
use mojito_types::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
use mojito_vm::backend::VmBackend;
use mojito_vm::runtime::Value;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

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
        // A dictionary iterates (and counts) its keys, as at runtime.
        self.comptime_iteration_elements()
            .or_else(|| self.typelist_elements().map(<[Self]>::to_vec))
            .ok_or_else(|| ComptimeError::BadRange(ctx.to_string()))
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
    let mut catalog = mojito_checked::templates::TemplateCatalog::new(false);
    mojito_checker::checker::validate_comptime_templates_into(&prepared, &mut catalog)
        .map_err(ComptimeError::Type)?;
    elaborate_prepared(&prepared, ElaborationInputs::new(&catalog))
        .map(|elaborated| elaborated.program)
}

/// Prepare a linked program for source validation and elaboration.
///
/// Qualify struct packs, synthesize the derived `copy`/`__hash__` methods,
/// give each conformer the trait defaults it inherits, desugar `SIMD[_, _]`
/// parameters, and fold SIMD alias bounds.
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
    desugar_simd_wildcard_parameters(&mut program);
    fold_simd_alias_bounds(&mut program);
    read_materialize_self_operands(&mut program);
    Ok(program)
}

/// An elaborated program, with what its cloner generated along the way.
pub struct Elaborated {
    pub program: Vec<Stmt>,
    /// How each generated `def` clone came from its template.
    pub def_traces: Vec<DefInstanceTrace>,
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
/// subprogram checks derive their traced clones from, holding the verdict
/// of the source validation run every elaboration follows.
#[derive(Clone, Copy)]
pub struct ElaborationInputs<'a> {
    pub def_requests: &'a [DefSpecializationRequest],
    pub templates: &'a mojito_checked::templates::TemplateCatalog,
}

impl<'a> ElaborationInputs<'a> {
    /// A first elaboration under `templates`, with no discovered requests.
    pub const fn new(templates: &'a mojito_checked::templates::TemplateCatalog) -> Self {
        Self {
            def_requests: &[],
            templates,
        }
    }
}

/// The declarations an elaboration generated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeneratedDeclarations {
    /// `def` clones, by output name.
    pub defs: Vec<String>,
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
) -> Vec<(
    mojito_checked::templates::InstanceName,
    mojito_checked::templates::InstanceTrace,
)> {
    use mojito_checked::templates::{InstanceName, InstanceTrace, TemplateId};
    defs.into_iter()
        .map(|trace| {
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
                },
            )
        })
        .collect()
}

/// The elaborator's generated-declaration list in the checker's terms.
pub fn generated_names(
    generated: GeneratedDeclarations,
) -> mojito_checked::templates::GeneratedNames {
    mojito_checked::templates::GeneratedNames {
        defs: generated.defs.into_iter().collect(),
    }
}

/// The top-level bound-generic template names of a linked program. The
/// compiler's discovery loop filters checker-recorded instantiations to these
/// callees.
pub fn bound_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_bound_generic_templates(program)
}

/// Elaborate a [`prepare`]d, validated program while materializing
/// checker-discovered inferred bound-generic applications.
///
/// This is the already-validated route: ordinary callers use [`elaborate`],
/// and the compiler's discovery loop — which validates the prepared program
/// once — supplies requests here each round.
pub fn elaborate_prepared(
    program: &[Stmt],
    inputs: ElaborationInputs<'_>,
) -> Result<Elaborated, ComptimeError> {
    let ElaborationInputs {
        def_requests,
        templates,
    } = inputs;
    let indexes = mojito_common::timing::span("indexes");
    let conformance =
        mojito_checker::checker::ConformanceOracle::from_program(program).map_err(|error| {
            ComptimeError::NotComptime(format!(
                "could not build the specialization conformance oracle: {error}"
            ))
        })?;
    let bound_generics = collect_bound_generic_templates(program);
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
        pack_defs: program
            .iter()
            .filter(|statement| pack_keyed_declaration(statement))
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect(),
        templates,
        ctfe_template_stats: RefCell::new(mojito_checked::templates::TemplateStats::default()),
        conformance,
        fuel: Cell::new(FUEL),
        template_binders: RefCell::new(Vec::new()),
        crossing_templates: Cell::new(0),
        def_traces: RefCell::new(Vec::new()),
        generated: RefCell::new(GeneratedDeclarations::default()),
        top_consts: RefCell::new(HashMap::new()),
        pending_constants: RefCell::new(HashMap::new()),
        forced_constants: RefCell::new(HashMap::new()),
        forcing_constants: RefCell::new(HashSet::new()),
        evaluating_body: Cell::new(false),
        generic_aliases: RefCell::new(HashMap::new()),
    };
    drop(indexes);
    elab.check_default_effects(program)?;
    let mut env = HashMap::new();
    let mut elaborated = elab.block(program, &mut env, false)?;
    let deferred = elab.request_pending_reads(&mut elaborated);
    // A module constant declared after its use crosses here.
    let consts = elab.top_consts.borrow().clone();
    elab.fold_runtime_crossings(&mut elaborated, &consts)?;
    // Materialize module-level comptime constants into runtime literals.
    let failure = RefCell::new(None);
    let mut materialized = materialize_block(
        elaborated,
        &consts,
        &elab.struct_names,
        &elab.applied_constants(),
        &elab.pending_lookup(&failure),
    );
    if let Some(error) = failure.into_inner() {
        return Err(error);
    }
    elab.restore_forced_constants(&mut materialized, deferred);
    // Monomorphize comptime-dependent generic templates against their call sites.
    let mut result = elab.monomorphize(materialized, def_requests)?.program;
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    let generated = elab.generated.take();
    let def_traces = elab.def_traces.take();
    let clones = census::clone_census(&census::Minted {
        prepared: program,
        def_traces: &def_traces,
    });
    Ok(Elaborated {
        program: result,
        def_traces,
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
mod requests;
mod synth;
mod unparse;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use ctfe_calls::*;
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

/// Whether `expression` is a literal of a scalar type a loop binder takes.
fn literal_element(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) => true,
        ExprKind::Prefix(PrefixOp::Neg, inner) => {
            matches!(inner.kind, ExprKind::Int(_) | ExprKind::Float(_))
        }
        _ => false,
    }
}

/// Whether a brace display is a set or dictionary of literals.
fn literal_entries(entries: &[(Expr, Option<Expr>)]) -> bool {
    !entries.is_empty()
        && entries
            .iter()
            .all(|(key, value)| literal_element(key) && value.as_ref().is_none_or(literal_element))
}

/// The alias a `comptime NAME = Ts[index]` statement declares of an element
/// of a pack `is_pack` accepts, as the type `Ts[index]` it denotes: such an
/// alias names a parameter expression, so a template carries it as that
/// dependent element rather than evaluating it with the index unknown. An
/// enclosing struct's pack is spelled, and asked of `is_pack`, as `Self.Ts`.
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
    if !type_params.is_empty() || !where_clauses.is_empty() {
        return None;
    }
    match &object.kind {
        ExprKind::Identifier(base) if is_pack(base) => Some((
            name.clone(),
            Type::Named(base.clone(), vec![ParamArg::Value((**index).clone())]),
        )),
        ExprKind::Member { object, field }
            if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")
                && is_pack(&format!("Self.{field}")) =>
        {
            Some((
                name.clone(),
                Type::IndexedProjection {
                    base: Box::new(Type::SelfParam(field.clone())),
                    index: index.clone(),
                },
            ))
        }
        _ => None,
    }
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

/// Whether a compile-time evaluation's method body can only
/// check once its own parameters are bound: it holds compile-time control
/// flow, or a `rebind` assertion over them. Either way the template is
/// stubbed and every instantiation clones.
fn block_keys_specialization(stmts: &[Stmt]) -> bool {
    block_has_comptime(stmts) || block_has_rebind(stmts)
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

/// The names a `def` binds itself, so that a bare name outside them is a
/// module's: its compile-time and runtime parameters, the locals and loop
/// variables its body declares, a nested `def`'s included, and each local
/// `comptime` binding other than one of a literal display, which is a closed
/// collection wherever the body names it.
fn def_bound_names(
    type_params: &[TypeParam],
    params: &[FnParam],
    body: &[Stmt],
) -> HashSet<String> {
    #[derive(Default)]
    struct Bound {
        names: HashSet<String>,
    }

    impl mojito_ast::visit::Visitor for Bound {
        fn visit_stmt(&mut self, statement: &Stmt) {
            match &statement.kind {
                StmtKind::Comptime {
                    type_params,
                    ty: None,
                    value,
                    ..
                } if type_params.is_empty()
                    && match &value.kind {
                        ExprKind::ListLit(items) => items.iter().all(literal_element),
                        ExprKind::BraceLit(entries) => literal_entries(entries),
                        _ => false,
                    } => {}
                StmtKind::Comptime { name, .. }
                | StmtKind::VarDecl { name, .. }
                | StmtKind::RefDecl { name, .. }
                | StmtKind::Assign { name, .. }
                | StmtKind::Def { name, .. } => {
                    self.names.insert(name.clone());
                }
                StmtKind::For { var, .. } | StmtKind::ComptimeFor { var, .. } => {
                    self.names.insert(var.clone());
                }
                StmtKind::Unpack { targets, .. } => {
                    self.names
                        .extend(targets.iter().filter_map(|target| match &target.kind {
                            ExprKind::Identifier(name) => Some(name.clone()),
                            _ => None,
                        }));
                }
                StmtKind::Try {
                    except: Some((name, _)),
                    ..
                } => self.names.extend(name.clone()),
                StmtKind::With { items, .. } => {
                    self.names
                        .extend(items.iter().filter_map(|item| item.var.clone()));
                }
                _ => {}
            }
        }
    }

    let mut bound = Bound::default();
    bound.names.extend(
        type_params
            .iter()
            .map(|parameter| parameter.name.trim_start_matches('*').to_string()),
    );
    bound
        .names
        .extend(params.iter().map(|parameter| parameter.name.clone()));
    mojito_ast::visit::walk_block(&mut bound, body);
    bound.names
}

/// Substitute one now-concrete type binder in a source annotation, wherever
/// it appears unshadowed.
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
        | Type::SelfType => {}
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
) -> Option<Type> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => Type::Int,
        Ty::UInt => Type::UInt,
        Ty::Bool => Type::Bool,
        Ty::StringLiteral => Type::ClosedStringLiteral,
        Ty::Float64 | Ty::FloatLiteral => Type::Float64,
        Ty::None => Type::None,
        Ty::Dtype => Type::Named("DType".to_string(), Vec::new()),
        // A callable has no source spelling a clone could carry.
        Ty::Func { .. } | Ty::GenericFunc { .. } => return None,
        Ty::ComptimeList(element) => Type::Named(
            "List".to_string(),
            vec![ParamArg::Type(source_type_from_ty_with_origins(
                element,
                origin_names,
            )?)],
        ),
        Ty::Tuple(elements) => Type::Named(
            "__RuntimeTuple".to_string(),
            elements
                .iter()
                .map(|element| source_type_from_ty_with_origins(element, origin_names))
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
                    .map(|element| source_type_from_ty_with_origins(element, origin_names))
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
                        source_type_from_ty_with_origins(ty, origin_names).map(ParamArg::Type)
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
                    ParamArg::Type(source_type_from_ty_with_origins(element, origin_names)?),
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
/// its compile-time arguments: a generic `def` keyed on a value pack whose
/// binders its template does not serve ([`template_serves_binders`]). A `def` keyed on a type pack never does: its
/// template serves the body, as upstream's does, with the collector a pack of
/// the symbolic `Ts`, and the elaborator below MIR binds the pack from the
/// call. This predicate is intentionally independent of the top-level
/// registry, so a nested `def` answers it the same way.
fn is_specializable_declaration(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Def {
            name, type_params, ..
        } => {
            !type_params.is_empty()
                && !pack_keyed_declaration(statement)
                && variadic_keyed_declaration(statement)
                && !template_serves_binders(type_params, name)
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
    /// Top-level type-pack `def`s, every one served by its template: a
    /// clone's spread of its own pack into one expands element by element.
    pack_defs: HashSet<String>,
    /// Checker-owned declaration facts used to validate inferred pack bounds
    /// before specialization consumes the source generic call.
    conformance: mojito_checker::checker::ConformanceOracle,
    /// The compilation's checked templates, which the checks of a VM-CTFE
    /// subprogram derive its traced clones from.
    templates: &'a mojito_checked::templates::TemplateCatalog,
    /// What those checks derived and inferred.
    ctfe_template_stats: RefCell<mojito_checked::templates::TemplateStats>,
    fuel: Cell<usize>,
    /// The compile-time parameter names of each generic `def` whose body is
    /// being elaborated as a template, innermost last. A `comptime if`
    /// whose condition names one is kept for the check: its arms are the
    /// template's, and the elaborator below MIR selects.
    template_binders: RefCell<Vec<HashSet<String>>>,
    /// How many generic bodies the runtime-crossing pass has descended into.
    crossing_templates: Cell<usize>,
    /// The declaration-level trace of every `def` clone generated so far.
    def_traces: RefCell<Vec<DefInstanceTrace>>,
    /// The `def` clones and per-call method clones generated so far.
    generated: RefCell<GeneratedDeclarations>,
    top_consts: RefCell<HashMap<String, CtValue>>,
    /// The module constants evaluated on demand, by name
    /// ([`Self::defer_constant`]).
    pending_constants: RefCell<HashMap<String, requests::PendingConstant>>,
    /// The pending constants a reader above the check forced, with their
    /// values.
    forced_constants: RefCell<HashMap<String, CtValue>>,
    /// The pending constants being forced, which a cycle demands again.
    forcing_constants: RefCell<HashSet<String>>,
    /// Whether the expression being evaluated is a function body's: a local
    /// `comptime` initializer or a `comptime(...)` operand. None reaches
    /// the AST route ([`Self::ctfe_call`] and the other entries), which
    /// serves readers above the check alone.
    evaluating_body: Cell<bool>,
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
    vals: Vec<CtValue>,
}

/// The monomorphization worklist and its results.
#[derive(Default)]
struct Mono {
    queue: VecDeque<Job>,
    /// Mangled names already requested (dedups identical instantiations).
    done: HashSet<String>,
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
    /// Whether the walk is inside an unstamped bundled stdlib declaration:
    /// instances reached only from there keep the erased path.
    in_bundled: bool,
}

impl Mono {
    /// Whether a specialization named `output_name` is new and should be
    /// queued.
    fn queue_specialization(&mut self, output_name: &str) -> bool {
        self.done.insert(output_name.to_string())
    }

    /// Leave the call or function-value use of `template` at `site` on its
    /// abstract path.
    fn retain_abstract(&mut self, template: &str) {
        self.retained.insert(template.to_string());
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

/// Collect the top-level generic `def`s that are templates: the bound-generic
/// ones, which resolve softly, and the value-pack ones whose binders their
/// template does not serve ([`is_specializable_declaration`]), which
/// specialize per call. An inferred call to one is served from the checker's
/// recorded instantiation, since the elaborator does not infer types.
fn collect_specializable<'a>(
    program: &'a [Stmt],
    bound_generics: &HashSet<String>,
) -> HashMap<String, &'a Stmt> {
    let mut m = HashMap::new();
    for s in program {
        if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &s.kind
            && (is_specializable_declaration(s) || bound_generics.contains(name))
        {
            // An overloaded name has one entry here, the first declaration:
            // this registry answers the name-level question "is this a
            // template at all?".
            m.entry(name.clone()).or_insert(s);
        }
    }
    m
}

/// Whether every compile-time parameter of a `def` is one its template
/// serves: a type parameter — a type pack included, which the elaborator
/// binds from the call's recorded elements — or a value parameter typed by a
/// scalar (`Int`, `UInt`, `Bool`, `Float64`, `StringLiteral`, `DType`) or by
/// an earlier type binder (`[T: AnyType, //, v: T]`), a value pack among
/// them. The elaborator binds such a value from the call's recorded
/// arguments, from the argument's lane slot, or from the checker's inferred
/// instantiation (`n` of `a: Box[n]`).
pub(super) fn template_serves_binders(type_params: &[TypeParam], owner: &str) -> bool {
    !type_params.is_empty()
        && type_params.iter().all(|parameter| {
            match classify_ct_param(parameter, type_params, owner) {
                Some(ParamDecl::Type { .. }) => true,
                Some(ParamDecl::Value { ty, .. }) => matches!(
                    ty.as_ref(),
                    Ty::Int
                        | Ty::UInt
                        | Ty::Bool
                        | Ty::Float64
                        | Ty::StringLiteral
                        | Ty::Dtype
                        | Ty::Param { .. }
                ),
                _ => false,
            }
        })
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

/// Whether a top-level `def` declares a pack among its compile-time
/// parameters: a `*Ts` type pack or a `*values` value pack.
fn variadic_keyed_declaration(statement: &Stmt) -> bool {
    matches!(&statement.kind, StmtKind::Def { type_params, .. }
        if type_params.iter().any(|parameter| parameter.name.starts_with('*')))
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
    program
        .iter()
        .filter_map(|statement| {
            let StmtKind::Def {
                name, type_params, ..
            } = &statement.kind
            else {
                return None;
            };
            if is_specializable_declaration(statement) || def_counts[name.as_str()] != 1 {
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
            (has_type_binder || template_serves_binders(type_params, name)).then(|| name.clone())
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
    vals: Vec<CtValue>,
    site: String,
    output_name: String,
    whole_pack_abi: bool,
}

fn source_type_from_ty(ty: &Ty) -> Option<Type> {
    source_type_from_ty_with_origins(ty, &HashMap::new())
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

mod rewrite;

mod specialize;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rewrite::*;

impl<'a> Elab<'a> {
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
    use super::ct_to_vm;
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
            mojito_vm::crossing::vm_to_ct(runtime).expect("VM CTFE list crosses back to CtValue"),
            source
        );
    }
}

/// The request-driven elaboration of an unprepared program, for the unit
/// tests below: prepare, validate, then elaborate, as [`elaborate`] does.
#[cfg(test)]
fn elaborate_with_requests(
    program: Vec<Stmt>,
    def_requests: &[DefSpecializationRequest],
) -> Result<Elaborated, ComptimeError> {
    let prepared = prepare(program)?;
    let mut catalog = mojito_checked::templates::TemplateCatalog::new(false);
    mojito_checker::checker::validate_comptime_templates_into(&prepared, &mut catalog)
        .map_err(ComptimeError::Type)?;
    elaborate_prepared(
        &prepared,
        ElaborationInputs {
            def_requests,
            ..ElaborationInputs::new(&catalog)
        },
    )
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

        let elaborated = elaborate_with_requests(parsed, &[request])
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
    fn no_top_level_type_pack_def_is_specializable() {
        let source = "struct Ints(Movable):\n    var n: Int\n    \
                      def __init__(out self, *a: Int):\n        self.n = 0\n\n\
                      def mk(n: Int) raises -> Int:\n    return n\n\n\
                      def ints[*Ts: Movable](*args: *Ts) -> Int:\n    return Ints(*args).n\n\n\
                      def nested[*Ts: Writable](*args: *Ts):\n    \
                      def one() -> Int:\n        return 1\n    print(one())\n\n\
                      def raising[*Ts: Writable](*args: *Ts) raises:\n    \
                      comptime for p in [mk(1), 2]:\n        print(p)\n\n\
                      def runtime_bound[s: Float64, *Ts: Writable](*args: *Ts):\n    \
                      comptime for i in range(len(args)):\n        print(args[i], s)\n\n\
                      def keyed[n: Int]():\n    \
                      comptime for p in [mk(n), 2]:\n        print(p)\n";
        let parsed = parse(source).expect("parse");
        let specializable: Vec<&str> = parsed
            .iter()
            .filter(|statement| super::is_specializable_declaration(statement))
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();

        // Every template serves its body, whatever loop it holds.
        assert!(specializable.is_empty());
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

        let elaborated = elaborate_with_requests(parsed, &[request])
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

        let elaborated = elaborate_with_requests(parsed, &[request])
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

        let elaborated = elaborate_with_requests(parsed, &[request])
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

    #[test]
    fn a_method_display_element_is_template_served_through_the_request_seam() {
        let source = "@fieldwise_init\nstruct P(Copyable, Movable):\n    var v: Int\n\n    \
                      def get(self) -> Int:\n        return self.v\n\n\
                      def show[n: Int]():\n    \
                      comptime for p in [P(n).get(), n]:\n        print(p)\n\n\
                      def main():\n    show[3]()\n";
        let linked = mojito::module::inject_prelude(parse(source).expect("parse")).expect("link");

        let elaborated = elaborate_with_requests(linked, &[])
            .expect("elaborate")
            .program;

        // Validation types the method call, so the template serves the loop
        // and the application keys no clone.
        let defs = def_names(&elaborated);
        assert!(defs.contains(&"show"), "{defs:?}");
        assert!(
            !defs.iter().any(|name| name.starts_with("show$")),
            "{defs:?}"
        );
    }
}

#[cfg(test)]
mod value_typed_binder_tests {
    use super::{classify_ct_params, template_serves_binders};
    use mojito::parse;
    use mojito_ast::ast::StmtKind;
    use mojito_types::types::{ParamDecl, Ty};

    #[test]
    fn a_value_pack_typed_by_a_sibling_binder_is_a_served_value_parameter() {
        let parsed =
            parse("def g[T: AnyType, //, *vs: T]() -> Int:\n    return 0\n").expect("parse");
        let type_params = parsed
            .iter()
            .find_map(|statement| match &statement.kind {
                StmtKind::Def { type_params, .. } => Some(type_params),
                _ => None,
            })
            .expect("one def");
        let decls = classify_ct_params(type_params, "g");
        assert!(matches!(
            decls.as_slice(),
            [
                ParamDecl::Type { id: binder, .. },
                ParamDecl::Value { ty, variadic: true, .. },
            ] if matches!(ty.as_ref(), Ty::Param { binder: typed, .. } if typed.id == *binder)
        ));
        assert!(template_serves_binders(type_params, "g"));
    }
}
