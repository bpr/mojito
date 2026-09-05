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
//! - **`comptime if`** — keeps only the taken branch; the others are dropped before
//!   type-checking.
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
//!   resolving the comptime construct so only the *selected* branch is type-checked
//!   (`f[0]` and `f[1]` take different branches, and a type error in a dropped
//!   branch is never seen).
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
use mojito_common::token::{SourceSpan, Span};
use mojito_types::ct::{CtExpr, CtValue};
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

/// One value-producing Tuple method selected during checked discovery. These
/// requests are receiver-specific: emitting every transform whose result type
/// happens to exist would manufacture reciprocal declaration dependencies
/// (for example `[Int, String].reverse()` and the uncalled reverse direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TupleTransformRequest {
    Reverse,
    Concat(Vec<Ty>),
}

impl TupleSpecializationRequest {
    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub fn declaration(elements: Vec<Ty>) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: None,
        }
    }

    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub fn bare_call(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            bare_call: Some(occurrence),
            transform: None,
        }
    }

    pub fn transform(elements: Vec<Ty>, transform: TupleTransformRequest) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: Some(transform),
        }
    }

    pub fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub fn occurrence(&self) -> Option<&SourceSpan> {
        self.bare_call.as_ref()
    }

    pub fn requested_transform(&self) -> Option<&TupleTransformRequest> {
        self.transform.as_ref()
    }
}

/// One checker-discovered lazy template-string occurrence: the interleaved
/// element types of a `t"…"` expression (literal segments as `String`,
/// interpolation snapshots at their checked types) and the source occurrence
/// whose AST node monomorphization rewrites into a construction of the
/// concrete `TString` specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TStringSpecializationRequest {
    elements: Vec<Ty>,
    occurrence: SourceSpan,
}

impl TStringSpecializationRequest {
    pub fn new(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            occurrence: occurrence.without_syntax(),
        }
    }

    pub fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }
}

/// One checker-discovered inferred application of a bound-generic `def`
/// template. The pre-check elaborator cannot infer types, so the compiler's
/// discovery loop replays the checker's resolved instantiation at the exact
/// call occurrence. A request can only upgrade a call from the abstract
/// erased-dispatch path to a concrete clone; any mismatch, misalignment, or
/// collision is skipped and the call stays abstract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefSpecializationRequest {
    /// The call occurrence, stored without its phase-local syntax id.
    occurrence: SourceSpan,
    callee: String,
    /// The checker's declaration-order argument list from `resolve_use_params`.
    arguments: Vec<TyArg>,
}

impl DefSpecializationRequest {
    pub fn new(occurrence: SourceSpan, callee: String, arguments: Vec<TyArg>) -> Self {
        Self {
            occurrence: occurrence.without_syntax(),
            callee,
            arguments,
        }
    }

    pub fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }

    pub fn callee(&self) -> &str {
        &self.callee
    }

    pub fn arguments(&self) -> &[TyArg] {
        &self.arguments
    }
}

/// One checker-discovered application of a generic *method* of a specialized
/// variadic struct (`bag.find[Int]()`, `v.set(3)` inferring `T`). The
/// specializer mints one clone per distinct instantiation
/// (`find$y3:Int`) inside the owner, and the checker retargets the call to
/// it by exact name on the next discovery round. A request that names no
/// method, or whose arguments do not align with the method's declaration,
/// is skipped: the call keeps the template's erased path.
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
    /// The checker's declaration-order argument list from `resolve_use_params`.
    arguments: Vec<TyArg>,
}

impl MethodSpecializationRequest {
    pub fn new(
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
            arguments,
        }
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
    }

    pub fn occurrence(&self) -> &SourceSpan {
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
/// The specializer appends one clone per available method to the live
/// template with the struct's parameters baked (`get$y3:Int`), and the
/// checker retargets calls on that instance to the clones by exact name on
/// the next discovery round.
#[derive(Debug, Clone, PartialEq)]
pub struct StructInstanceRequest {
    template: String,
    arguments: Vec<TyArg>,
}

impl StructInstanceRequest {
    pub fn new(template: String, arguments: Vec<TyArg>) -> Self {
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
/// through opaque compiler-only AST ids. Source `def(...)` annotations cannot
/// encode all of this metadata, so the compiler passes this map directly to the
/// second checker pass instead of round-tripping through syntax.
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
            Ty::Dependent(mojito_types::types::DependentType::Indexed { elements, .. }) => {
                for element in elements {
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
/// [`ComptimeError`] when a value is not of the required kind. An extension
/// trait: `CtValue` lives in the types layer below this phase, so an
/// inherent impl cannot.
pub trait CtValueExt {
    fn as_bool(&self, ctx: &str) -> Result<bool, ComptimeError>;
    fn as_int(&self, ctx: &str) -> Result<i64, ComptimeError>;
    fn as_sequence(&self, ctx: &str) -> Result<Vec<CtValue>, ComptimeError>;
    fn typelist_elements(&self) -> Option<&[CtValue]>;
}

impl CtValueExt for CtValue {
    fn as_bool(&self, ctx: &str) -> Result<bool, ComptimeError> {
        match self {
            CtValue::Bool(b) => Ok(*b),
            _ => Err(ComptimeError::NotBool(ctx.to_string())),
        }
    }
    fn as_int(&self, ctx: &str) -> Result<i64, ComptimeError> {
        match self {
            CtValue::Int(n) => Ok(*n),
            CtValue::IntLiteral(n) => n.wrapping_signed(64).ok_or_else(|| {
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
            CtValue::Tuple(v) | CtValue::List(v) => Ok(v.clone()),
            _ => self
                .typelist_elements()
                .map(<[CtValue]>::to_vec)
                .ok_or_else(|| ComptimeError::BadRange(ctx.to_string())),
        }
    }

    /// The element types carried by a compile-time `TypeList` value, or
    /// `None` for any other value.
    fn typelist_elements(&self) -> Option<&[CtValue]> {
        match self {
            CtValue::Struct { name, fields } if name == "TypeList" => match fields.as_slice() {
                [(field, CtValue::Tuple(values))] if field == "values" => Some(values),
                _ => None,
            },
            // A bound type pack (`*Ts` specialized to concrete types) is
            // upstream's `TypeList` in every compile-time position, so
            // `Ts.length`, `Ts[i]`, `Ts.all_conforms_to[..]()`, and
            // `Ts.contains[T]()` read it directly.
            CtValue::Tuple(values)
                if values.iter().all(|value| matches!(value, CtValue::Type(_))) =>
            {
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
    /// A condition did not evaluate to `Bool`.
    NotBool(String),
    /// A context required a compile-time `Int`.
    NotInt(String),
    /// Integer `//`/`%` by zero, or a negative `**` exponent, at compile time.
    BadArithmetic(String),
    /// A `comptime for` iterable was not a `range(...)` / tuple / list.
    BadRange(String),
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
    /// The compile-time step/iteration quota was exceeded (a likely infinite loop).
    QuotaExceeded,
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
            ComptimeError::NotComptime(s) => write!(f, "not a compile-time value: {s}"),
            ComptimeError::NotBool(s) => write!(f, "expected a compile-time Bool ({s})"),
            ComptimeError::NotInt(s) => write!(f, "expected a compile-time Int ({s})"),
            ComptimeError::BadArithmetic(s) => write!(f, "compile-time arithmetic error: {s}"),
            ComptimeError::BadRange(s) => {
                write!(f, "'comptime for' needs a range(...)/tuple/list: {s}")
            }
            ComptimeError::Arity(s) => write!(f, "compile-time call arity: {s}"),
            ComptimeError::PackBound(error) => {
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
            ComptimeError::GenericBound(error) => {
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
            ComptimeError::Constraint(message) => {
                write!(f, "compile-time constraint failed: {message}")
            }
            ComptimeError::QuotaExceeded => {
                write!(f, "compile-time execution exceeded the step quota ({FUEL})")
            }
        }
    }
}

/// Elaborate all compile-time constructs in a program, returning an ordinary AST.
pub fn elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    elaborate_with_requests(program, &[], &[], &[], &[], &[]).map(|elaborated| elaborated.program)
}

/// An elaborated program plus the generic-struct instances the specializer
/// minted method clones for along the way (closed applications reached from
/// user code and from other clones), so the driver's discovery loop does not
/// treat the checker's recordings of those instances as new work.
pub struct Elaborated {
    pub program: Vec<Stmt>,
    pub instances: Vec<StructInstanceRequest>,
}

/// The top-level variadic struct template names (`struct S[*Ts: Bound]`) of a
/// linked program. A specialized instance is named `<template>$t<n>[...]`;
/// the compiler's discovery loop filters checker-recorded method
/// instantiations to receivers of that shape.
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

/// The top-level bound-generic template names of a linked program, as the
/// elaborator will classify them. The compiler's discovery loop filters
/// checker-recorded instantiations to these callees.
pub fn bound_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_bound_generic_templates(program)
}

/// Elaborate a program while materializing checker-discovered public `Tuple`
/// and `TString` specializations and inferred bound-generic applications.
/// This is a crate-internal staging seam: ordinary callers use [`elaborate`],
/// and the compiler's discovery loop supplies requests here.
pub fn elaborate_with_requests(
    mut program: Vec<Stmt>,
    tuple_requests: &[TupleSpecializationRequest],
    tstring_requests: &[TStringSpecializationRequest],
    def_requests: &[DefSpecializationRequest],
    method_requests: &[MethodSpecializationRequest],
    struct_requests: &[StructInstanceRequest],
) -> Result<Elaborated, ComptimeError> {
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
    synthesize_copyable_copy(&mut program);
    synthesize_hashable_hash(&mut program);
    let conformance =
        mojito_checker::checker::ConformanceOracle::from_program(&program).map_err(|error| {
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
    let bound_generics = collect_bound_generic_templates(&program);
    let elab = Elab {
        program: &program,
        fns: collect_fns(&program),
        structs: collect_structs(&program),
        struct_names: program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect(),
        specializable: collect_specializable(&program, &bound_generics),
        bound_generics,
        method_requests: method_requests_by_owner,
        instance_requests,
        conformance,
        tuple_universe,
        tuple_transforms,
        materialized_callables,
        fuel: Cell::new(FUEL),
        top_consts: RefCell::new(HashMap::new()),
        generic_aliases: RefCell::new(HashMap::new()),
    };
    let mut env = HashMap::new();
    let elaborated = elab.block(&program, &mut env, false)?;
    // Materialize module-level comptime constants into runtime literals.
    let consts = elab.top_consts.borrow().clone();
    let materialized = materialize_block(elaborated, &consts, &elab.struct_names);
    // Monomorphize comptime-dependent generic templates against their call sites.
    let (mut result, instances) =
        elab.monomorphize(materialized, tuple_requests, tstring_requests, def_requests)?;
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    // Per-instantiation method clones reuse their template's spans; each
    // clone's body gets its own source tag after the uniform module stamp
    // above (the discipline struct specializations follow), keeping
    // span-keyed checked facts separate across instantiations.
    for statement in &mut result {
        let module = statement.module.clone();
        if let StmtKind::Struct { name, methods, .. } = &mut statement.kind {
            for method in methods.iter_mut() {
                if method.self_ty.is_some() {
                    let tag = match &module {
                        Some(module) => format!("{module}${name}${}", method.name),
                        None => format!("{name}${}", method.name),
                    };
                    mojito_ast::ast::stamp_source(&mut method.body, &tag);
                }
            }
        }
    }
    // Nested templates are specialized only after enclosing top-level
    // specializations and source stamping. At that point every clone carries its
    // concrete outer substitutions, and per-instance source tags will not be
    // overwritten by the uniform module stamp above.
    elab.monomorphize_nested_program(&mut result)?;
    Ok(Elaborated {
        program: result,
        instances,
    })
}

mod ctfe_calls;
mod elab;
mod packs;
mod params;
mod simd_width;
mod synth;

use ctfe_calls::*;
use packs::*;
use params::*;
use simd_width::*;
use synth::*;

fn mk(kind: StmtKind, span: Span) -> Stmt {
    Stmt {
        kind,
        span,
        module: None,
        syntax_id: mojito_common::token::SyntaxId::fresh(),
    }
}

/// Whether a block directly contains a `comptime if`/`comptime for` (not descending
/// into nested `def`/`struct`, which have their own compile-time scope).
fn block_has_comptime(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_comptime)
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
        Ty::Dependent(mojito_types::types::DependentType::Indexed { elements, .. }) => elements
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

fn ct_expr_from_ast(expr: &Expr) -> Option<CtExpr> {
    let pair = |left: &Expr, right: &Expr| {
        Some((
            Box::new(ct_expr_from_ast(left)?),
            Box::new(ct_expr_from_ast(right)?),
        ))
    };
    Some(match &expr.kind {
        ExprKind::Identifier(name) => CtExpr::Param(name.clone()),
        ExprKind::Prefix(PrefixOp::Neg, value) => CtExpr::Neg(Box::new(ct_expr_from_ast(value)?)),
        ExprKind::Infix(op, left, right) => {
            let (left, right) = pair(left, right)?;
            match op {
                InfixOp::Add => CtExpr::Add(left, right),
                InfixOp::Sub => CtExpr::Sub(left, right),
                InfixOp::Mul => CtExpr::Mul(left, right),
                InfixOp::FloorDiv => CtExpr::FloorDiv(left, right),
                InfixOp::Mod => CtExpr::Mod(left, right),
                InfixOp::Pow => CtExpr::Pow(left, right),
                _ => return None,
            }
        }
        _ => CtExpr::Value(literal_ct_value(expr)?),
    })
}

fn ct_param_source_type(source: &Type) -> Option<Ty> {
    match source {
        Type::Int => Some(Ty::Int),
        Type::UInt => Some(Ty::UInt),
        Type::Bool => Some(Ty::Bool),
        Type::StringLiteral => Some(Ty::StringLiteral),
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
        Ty::StringLiteral => Type::StringLiteral,
        Ty::Float64 | Ty::FloatLiteral => Type::Float64,
        Ty::None => Type::None,
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
                    // Origin arguments have no source-syntax reconstruction yet;
                    // origin-parameterized types are not monomorphized in this slice.
                    TyArg::Origin(_) => None,
                })
                .collect::<Option<Vec<_>>>()?,
        ),
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

fn ct_to_vm(value: &CtValue) -> Result<Value, ComptimeError> {
    match value {
        CtValue::Int(n) => Ok(Value::Int(*n)),
        CtValue::UInt(n) => Ok(Value::UInt(*n)),
        CtValue::Float(bits) => Ok(Value::Float64(f64::from_bits(*bits))),
        CtValue::IntLiteral(value) => Ok(Value::IntLiteral(value.clone())),
        CtValue::FloatLiteral(value) => Ok(Value::FloatLiteral(value.clone())),
        CtValue::Bool(b) => Ok(Value::Bool(*b)),
        CtValue::Str(s) => Ok(Value::Str(s.clone())),
        CtValue::Tuple(items) => Ok(Value::Tuple(
            items.iter().map(ct_to_vm).collect::<Result<Vec<_>, _>>()?,
        )),
        CtValue::List(items) => Ok(Value::ComptimeList(
            items.iter().map(ct_to_vm).collect::<Result<Vec<_>, _>>()?,
        )),
        CtValue::Struct { name, fields } => Ok(Value::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Ok((field.clone(), ct_to_vm(value)?)))
                .collect::<Result<Vec<_>, _>>()?,
            value_params: Vec::new(),
        }),
        CtValue::Dtype(_) | CtValue::Type(_) | CtValue::Reflected(_) | CtValue::Param(_) => {
            Err(ComptimeError::NotComptime(
                "type-valued or symbolic values cannot cross into VM CTFE".to_string(),
            ))
        }
    }
}

fn vm_to_ct(value: Value) -> Result<CtValue, ComptimeError> {
    match value {
        Value::Int(n) => Ok(CtValue::Int(n)),
        Value::UInt(n) => Ok(CtValue::UInt(n)),
        Value::Float64(value) => Ok(CtValue::Float(value.to_bits())),
        Value::IntLiteral(value) => Ok(CtValue::IntLiteral(value)),
        Value::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value)),
        Value::Bool(b) => Ok(CtValue::Bool(b)),
        Value::Str(s) => Ok(CtValue::Str(s)),
        Value::Tuple(items) => Ok(CtValue::Tuple(
            items
                .into_iter()
                .map(vm_to_ct)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::ComptimeList(items) => Ok(CtValue::List(
            items
                .into_iter()
                .map(vm_to_ct)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::None => Err(ComptimeError::NotComptime(
            "VM CTFE function returned None; a compile-time value is required".to_string(),
        )),
        other => Err(ComptimeError::NotComptime(format!(
            "VM CTFE returned unsupported runtime value {other}"
        ))),
    }
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
fn is_specializable_declaration(statement: &Stmt) -> bool {
    is_specializable_declaration_in(statement, &|_| false)
}

/// The registry-aware form: `is_value_struct` recognizes a single bound that
/// names a struct (a struct-typed value parameter such as `[e: Extent]`),
/// which — like a `DType` parameter — forces per-application monomorphization.
fn is_specializable_declaration_in(
    statement: &Stmt,
    is_value_struct: &dyn Fn(&str) -> bool,
) -> bool {
    match &statement.kind {
        StmtKind::Def {
            type_params, body, ..
        } => {
            !type_params.is_empty()
                && (block_has_comptime(body)
                    || type_params
                        .iter()
                        .any(|parameter| parameter.name.starts_with('*'))
                    // A `[dtype: DType]` parameter can only check concretely
                    // (`Ty::Simd` holds a concrete dtype), so the def
                    // monomorphizes per call.
                    || type_params.iter().any(|parameter| {
                        matches!(parameter.bounds.as_slice(), [only] if only == "DType")
                    })
                    || def_uses_layout_dependent_param(statement))
        }
        StmtKind::Struct { type_params, .. } => {
            type_params
                .iter()
                .any(|parameter| parameter.name.starts_with('*'))
                // DType- and struct-typed value parameters can only check
                // concretely, so the struct monomorphizes per application.
                || type_params.iter().any(|parameter| {
                    matches!(parameter.bounds.as_slice(), [only]
                        if only == "DType" || is_value_struct(only))
                })
        }
        _ => false,
    }
}

/// The maximum number of compile-time "steps" (loop iterations, statements
/// executed, function calls) across a whole program — a hard bound so compile-time
/// execution can't hang the compiler (cf. Zig's quota).
const FUEL: usize = 100_000;

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
    /// Checker-discovered generic-method instantiations on specialized
    /// variadic structs, by owner name: each becomes a per-call clone.
    method_requests: HashMap<String, Vec<MethodSpecializationRequest>>,
    /// Checker-discovered closed applications of ordinary generic structs, by
    /// template name: each mints per-instantiation method clones on the
    /// template.
    instance_requests: HashMap<String, Vec<Vec<TyArg>>>,
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
    fuel: Cell<usize>,
    top_consts: RefCell<HashMap<String, CtValue>>,
    /// Module-scope generic `comptime` aliases in declaration order, name →
    /// (parameters, body). The declarations pass through elaboration for the
    /// checker's alias registry, but an application inside a `comptime if`
    /// condition must already evaluate here — the branches are pruned before
    /// checking.
    generic_aliases: RefCell<HashMap<String, (Vec<TypeParam>, Expr)>>,
}

fn classify_ct_params(tps: &[TypeParam]) -> Vec<ParamDecl> {
    tps.iter()
        .filter_map(|tp| classify_ct_param(tp, tps))
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

/// The concrete `TString` specialization a checked `t"…"` occurrence
/// constructs, with the interleaved element types directing the argument
/// rewrite.
struct TStringTarget {
    symbol: String,
    elements: Vec<Ty>,
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
    /// Checker-discovered scalar `range(...)` occurrences: call occurrence →
    /// the linked range-family struct template plus the dtype value its
    /// generated specialization bakes. `mono_expr` rewrites the call into
    /// that concrete constructor.
    range_call_targets: HashMap<SourceSpan, (String, Vec<CtValue>)>,
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
}

impl Mono {
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
        // A `[dtype: DType]` value parameter; compile-time-only.
        "DType" => Some(Ty::Dtype),
        // A SIMD width parameter is a compile-time Int value parameter (the
        // removed `SIMDSize` spelling rejects).
        "SIMDLength" => Some(Ty::Int),
        "UInt" => Some(Ty::UInt),
        "Bool" => Some(Ty::Bool),
        "StringLiteral" => Some(Ty::StringLiteral),
        "Float64" => Some(Ty::Float64),
        "None" => Some(Ty::None),
        // The (qualified) `String` spelling deliberately falls through to
        // ordinary struct resolution: in type-argument and type-value
        // positions it denotes the nominal stdlib struct. Value-parameter
        // classification keeps the literal type via `ct_value_param_type`.
        _ => None,
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
                    ct_params: classify_ct_params(type_params),
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
                    decls: classify_ct_params(type_params),
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
/// comptime construct, so only the *selected* branch is type-checked. Because the
/// elaborator does not infer types, such a `def` must be called with explicit
/// `[...]` arguments.
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
    let mut m = HashMap::new();
    for s in program {
        if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &s.kind
            && (is_specializable_declaration_in(s, &|bound| struct_names.contains(bound))
                || bound_generics.contains(name))
        {
            m.insert(name.clone(), s);
        }
    }
    m
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
    let mut def_counts: HashMap<&str, usize> = HashMap::new();
    for statement in program {
        if let StmtKind::Def { name, .. } = &statement.kind {
            *def_counts.entry(name.as_str()).or_default() += 1;
        }
    }
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
            type_params
                .iter()
                .any(|parameter| {
                    matches!(
                        classify_ct_param(parameter, type_params),
                        Some(ParamDecl::Type {
                            variadic: false,
                            ..
                        })
                    )
                })
                .then(|| name.clone())
        })
        .collect()
}

fn stmt_has_comptime(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::ComptimeIf { .. } | StmtKind::ComptimeFor { .. } => true,
        StmtKind::If { branches, orelse } => {
            branches.iter().any(|(_, b)| block_has_comptime(b))
                || orelse.as_ref().is_some_and(|b| block_has_comptime(b))
        }
        StmtKind::While { body, .. } | StmtKind::For { body, .. } => block_has_comptime(body),
        StmtKind::With { body, .. } => block_has_comptime(body),
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            block_has_comptime(body)
                || except.as_ref().is_some_and(|(_, b)| block_has_comptime(b))
                || orelse.as_ref().is_some_and(|b| block_has_comptime(b))
                || finalbody.as_ref().is_some_and(|b| block_has_comptime(b))
        }
        _ => false,
    }
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
    source_type_from_ty_with_origins(ty, &HashMap::new(), &[])
}

/// The concrete call-site information used to select one function-template
/// specialization. Nested pack forwarding supplies its already-known element
/// types; ordinary calls leave that field empty and infer from expressions.
struct SpecRequest<'a> {
    param_args: &'a [ParamArg],
    call_args: &'a [Expr],
    kwargs: &'a [mojito_ast::ast::KwArg],
    consts: &'a HashMap<String, CtValue>,
    request_site: &'a str,
    forwarded_pack_types: Option<&'a [Ty]>,
}

fn compare_numeric_values(
    op: InfixOp,
    left: &CtValue,
    right: &CtValue,
) -> Result<bool, ComptimeError> {
    let exact = |value: &CtValue| match value {
        CtValue::Int(value) => Some(mojito_common::literal::FloatLiteral::from_int(
            &mojito_common::literal::IntLiteral::from(*value),
        )),
        CtValue::UInt(value) => Some(mojito_common::literal::FloatLiteral::from_int(
            &mojito_common::literal::IntLiteral::from(*value),
        )),
        CtValue::Float(bits) => {
            mojito_common::literal::FloatLiteral::from_f64(f64::from_bits(*bits))
        }
        CtValue::IntLiteral(value) => Some(mojito_common::literal::FloatLiteral::from_int(value)),
        CtValue::FloatLiteral(value) => Some(value.clone()),
        _ => None,
    };
    let (Some(left), Some(right)) = (exact(left), exact(right)) else {
        return Err(ComptimeError::NotComptime(
            "numeric comparison expects numeric operands".to_string(),
        ));
    };
    let ordering = left.as_rational().cmp(right.as_rational());
    use InfixOp::*;
    Ok(match op {
        Eq => ordering.is_eq(),
        Ne => !ordering.is_eq(),
        Lt => ordering.is_lt(),
        Gt => ordering.is_gt(),
        Le => !ordering.is_gt(),
        Ge => !ordering.is_lt(),
        _ => {
            return Err(ComptimeError::NotComptime(
                "not a comparison operator".to_string(),
            ));
        }
    })
}

fn lit_result(val: &CtValue, span: Span) -> Result<Expr, ComptimeError> {
    val.materialize(span).ok_or_else(|| {
        ComptimeError::NotComptime(
            "type-valued or symbolic comptime values cannot materialize at runtime".to_string(),
        )
    })
}

fn vm_ctfe_safe_builtin(name: &str) -> bool {
    matches!(
        name,
        "range" | "abs" | "min" | "max" | "round" | "Int" | "UInt" | "Float64"
    )
}

mod ctfe;

mod eval;

mod mono;

mod nested;

mod rewrite;

mod specialize;

use rewrite::*;

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

#[cfg(test)]
mod tuple_request_tests {
    use super::{TupleSpecializationRequest, elaborate_with_requests, tuple_specialization_symbol};
    use mojito::{Ty, parse};
    use mojito_ast::ast::{ExprKind, StmtKind};
    use mojito_types::types::tuple_type;

    const TEMPLATE: &str = "struct Tuple[*Ts: AnyType]:\n    var storage: __RuntimeTuple[*Ts]\n\n";

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
    fn closed_request_rewrites_the_inferred_call_and_drops_the_template() {
        let source = format!("{TEMPLATE}def main():\n    print(ident(2))\n");
        let parsed = parse(&source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "ident");
        let request = DefSpecializationRequest::new(
            occurrence,
            "ident".to_string(),
            vec![TyArg::Ty(Ty::Int)],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[])
            .expect("materialize the requested specialization")
            .program;

        let defs = def_names(&elaborated);
        assert!(
            defs.iter().any(|name| name.starts_with("ident$")),
            "{defs:?}"
        );
        assert!(!defs.contains(&"ident"), "{defs:?}");
        let calls = main_call_names(&elaborated);
        assert!(
            calls.iter().any(|name| name.starts_with("ident$")),
            "{calls:?}"
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
            vec![TyArg::Val(CtValue::Int(1))],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[])
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
    fn requested_and_explicit_applications_share_one_clone() {
        let source =
            format!("{TEMPLATE}def main():\n    print(ident[Int](1))\n    print(ident(2))\n");
        let parsed = parse(&source).expect("parse");
        let occurrence = inferred_call_span(&parsed, "ident");
        let request = DefSpecializationRequest::new(
            occurrence,
            "ident".to_string(),
            vec![TyArg::Ty(Ty::Int)],
        );

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request], &[], &[])
            .expect("materialize the requested specialization")
            .program;

        let defs = def_names(&elaborated);
        assert_eq!(
            defs.iter()
                .filter(|name| name.starts_with("ident$"))
                .count(),
            1,
            "{defs:?}"
        );
        assert!(!defs.contains(&"ident"), "{defs:?}");
        let calls = main_call_names(&elaborated);
        assert_eq!(
            calls
                .iter()
                .filter(|name| name.starts_with("ident$"))
                .count(),
            2,
            "{calls:?}"
        );
    }
}
