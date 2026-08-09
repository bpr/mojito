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
//! Compile-time values are the shared [`CtValue`](crate::ct::CtValue) universe:
//! runtime-materializable `Int`/`Bool`/`String`/`Tuple`/`List`, plus
//! compile-time-only `Type` and symbolic `Param` facts.

use crate::ast::{
    ArgConvention, Expr, ExprKind, FnParam, InfixOp, ParamArg, ParamKind, PrefixOp, Stmt, StmtKind,
    StructComptime, TStringPart, Type, TypeParam, WithItem,
};
use crate::backend::VmBackend;
use crate::call::{CallVariadics, effective_keyword_only_index, match_call_slots};
use crate::ct::{CtExpr, CtValue};
use crate::runtime::Value;
use crate::token::{SourceSpan, Span};
use crate::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
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
pub(crate) struct TupleSpecializationRequest {
    elements: Vec<Ty>,
    bare_call: Option<SourceSpan>,
    transform: Option<TupleTransformRequest>,
}

/// One value-producing Tuple method selected during checked discovery. These
/// requests are receiver-specific: emitting every transform whose result type
/// happens to exist would manufacture reciprocal declaration dependencies
/// (for example `[Int, String].reverse()` and the uncalled reverse direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TupleTransformRequest {
    Reverse,
    Concat(Vec<Ty>),
}

impl TupleSpecializationRequest {
    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub(crate) fn declaration(elements: Vec<Ty>) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: None,
        }
    }

    #[allow(dead_code)] // used by the compiler once checked discovery is wired in
    pub(crate) fn bare_call(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            bare_call: Some(occurrence),
            transform: None,
        }
    }

    pub(crate) fn transform(elements: Vec<Ty>, transform: TupleTransformRequest) -> Self {
        Self {
            elements,
            bare_call: None,
            transform: Some(transform),
        }
    }

    pub(crate) fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub(crate) fn occurrence(&self) -> Option<&SourceSpan> {
        self.bare_call.as_ref()
    }

    pub(crate) fn requested_transform(&self) -> Option<&TupleTransformRequest> {
        self.transform.as_ref()
    }
}

/// One checker-discovered lazy template-string occurrence: the interleaved
/// element types of a `t"…"` expression (literal segments as `String`,
/// interpolation snapshots at their checked types) and the source occurrence
/// whose AST node monomorphization rewrites into a construction of the
/// concrete `TString` specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TStringSpecializationRequest {
    elements: Vec<Ty>,
    occurrence: SourceSpan,
}

impl TStringSpecializationRequest {
    pub(crate) fn new(elements: Vec<Ty>, occurrence: SourceSpan) -> Self {
        Self {
            elements,
            occurrence: occurrence.without_syntax(),
        }
    }

    pub(crate) fn elements(&self) -> &[Ty] {
        &self.elements
    }

    pub(crate) fn occurrence(&self) -> &SourceSpan {
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
pub(crate) struct DefSpecializationRequest {
    /// The call occurrence, stored without its phase-local syntax id.
    occurrence: SourceSpan,
    callee: String,
    /// The checker's declaration-order argument list from `resolve_use_params`.
    arguments: Vec<TyArg>,
}

impl DefSpecializationRequest {
    pub(crate) fn new(occurrence: SourceSpan, callee: String, arguments: Vec<TyArg>) -> Self {
        Self {
            occurrence: occurrence.without_syntax(),
            callee,
            arguments,
        }
    }

    pub(crate) fn occurrence(&self) -> &SourceSpan {
        &self.occurrence
    }

    pub(crate) fn callee(&self) -> &str {
        &self.callee
    }

    pub(crate) fn arguments(&self) -> &[TyArg] {
        &self.arguments
    }
}

/// Exact callable types which a generated public-Tuple declaration references
/// through opaque compiler-only AST ids. Source `def(...)` annotations cannot
/// encode all of this metadata, so the compiler passes this map directly to the
/// second checker pass instead of round-tripping through syntax.
pub(crate) fn tuple_materialized_callables(
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
            Ty::Dependent(crate::types::DependentType::Indexed { elements, .. }) => {
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

/// Canonical concrete symbol selected for public `Tuple[*Ts]` element types.
pub(crate) fn tuple_specialization_symbol(elements: &[Ty]) -> String {
    mangle("Tuple", &tuple_specialization_values(elements))
}

pub(crate) fn tstring_specialization_symbol(elements: &[Ty]) -> String {
    mangle("TString", &tuple_specialization_values(elements))
}

/// Comptime-specific accessors on the shared [`CtValue`], reporting a
/// [`ComptimeError`] when a value is not of the required kind.
impl CtValue {
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
    /// The elements of a compile-time collection (`Tuple`/`List`), for iteration.
    fn as_sequence(&self, ctx: &str) -> Result<Vec<CtValue>, ComptimeError> {
        match self {
            CtValue::Tuple(v) | CtValue::List(v) => Ok(v.clone()),
            _ => Err(ComptimeError::BadRange(ctx.to_string())),
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
            ComptimeError::QuotaExceeded => {
                write!(f, "compile-time execution exceeded the step quota ({FUEL})")
            }
        }
    }
}

/// Elaborate all compile-time constructs in a program, returning an ordinary AST.
pub fn elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    elaborate_with_requests(program, &[], &[], &[])
}

/// The top-level bound-generic template names of a linked program, as the
/// elaborator will classify them. The compiler's discovery loop filters
/// checker-recorded instantiations to these callees.
pub(crate) fn bound_generic_template_names(program: &[Stmt]) -> HashSet<String> {
    collect_bound_generic_templates(program)
}

/// Elaborate a program while materializing checker-discovered public `Tuple`
/// and `TString` specializations and inferred bound-generic applications.
/// This is a crate-internal staging seam: ordinary callers use [`elaborate`],
/// and the compiler's discovery loop supplies requests here.
pub(crate) fn elaborate_with_requests(
    program: Vec<Stmt>,
    tuple_requests: &[TupleSpecializationRequest],
    tstring_requests: &[TStringSpecializationRequest],
    def_requests: &[DefSpecializationRequest],
) -> Result<Vec<Stmt>, ComptimeError> {
    let conformance =
        crate::checker::ConformanceOracle::from_program(&program).map_err(|error| {
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
        specializable: collect_specializable(&program, &bound_generics),
        bound_generics,
        conformance,
        tuple_universe,
        tuple_transforms,
        materialized_callables,
        fuel: Cell::new(FUEL),
        top_consts: RefCell::new(HashMap::new()),
    };
    let mut env = HashMap::new();
    let elaborated = elab.block(&program, &mut env, false)?;
    // Materialize module-level comptime constants into runtime literals.
    let consts = elab.top_consts.borrow().clone();
    let materialized = materialize_block(elaborated, &consts);
    // Monomorphize comptime-dependent generic templates against their call sites.
    let mut result =
        elab.monomorphize(materialized, tuple_requests, tstring_requests, def_requests)?;
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            crate::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    // Nested templates are specialized only after enclosing top-level
    // specializations and source stamping. At that point every clone carries its
    // concrete outer substitutions, and per-instance source tags will not be
    // overwritten by the uniform module stamp above.
    elab.monomorphize_nested_program(&mut result)?;
    Ok(result)
}

/// Collect bare free-function callees from an expression. This is a declaration
/// dependency walk, not a purity classifier: it traverses every child so the
/// checked VM-CTFE subprogram retains helpers mentioned anywhere in a retained
/// function or nominal method body.
fn collect_vm_ctfe_expr_calls(expression: &Expr, calls: &mut HashSet<String>) {
    let param_args = |arguments: &[ParamArg], calls: &mut HashSet<String>| {
        fn collect(argument: &ParamArg, calls: &mut HashSet<String>) {
            match argument {
                ParamArg::Type(_) => {}
                ParamArg::Value(value) => collect_vm_ctfe_expr_calls(value, calls),
                ParamArg::Named { value, .. } => collect(value, calls),
            }
        }
        for argument in arguments {
            collect(argument, calls);
        }
    };

    match &expression.kind {
        ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
            collect_vm_ctfe_expr_calls(value, calls)
        }
        ExprKind::Infix(_, left, right)
        | ExprKind::Index {
            object: left,
            index: right,
        } => {
            collect_vm_ctfe_expr_calls(left, calls);
            collect_vm_ctfe_expr_calls(right, calls);
        }
        ExprKind::Call {
            name,
            param_args: arguments,
            args,
            kwargs,
        } => {
            calls.insert(name.clone());
            param_args(arguments, calls);
            for argument in args {
                collect_vm_ctfe_expr_calls(argument, calls);
            }
            for argument in kwargs {
                collect_vm_ctfe_expr_calls(&argument.value, calls);
            }
        }
        ExprKind::Invoke {
            callee,
            param_args: arguments,
            args,
            kwargs,
        } => {
            collect_vm_ctfe_expr_calls(callee, calls);
            param_args(arguments, calls);
            for argument in args {
                collect_vm_ctfe_expr_calls(argument, calls);
            }
            for argument in kwargs {
                collect_vm_ctfe_expr_calls(&argument.value, calls);
            }
        }
        ExprKind::Member { object, .. } => collect_vm_ctfe_expr_calls(object, calls),
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            collect_vm_ctfe_expr_calls(object, calls);
            for argument in args {
                collect_vm_ctfe_expr_calls(argument, calls);
            }
            for argument in kwargs {
                collect_vm_ctfe_expr_calls(&argument.value, calls);
            }
        }
        ExprKind::TypeApply { args, .. } => param_args(args, calls),
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                collect_vm_ctfe_expr_calls(value, calls);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                collect_vm_ctfe_expr_calls(key, calls);
                if let Some(value) = value {
                    collect_vm_ctfe_expr_calls(value, calls);
                }
            }
        }
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            if let Some(key) = key {
                collect_vm_ctfe_expr_calls(key, calls);
            }
            collect_vm_ctfe_expr_calls(value, calls);
            for clause in clauses {
                match clause {
                    crate::ast::ComprehensionClause::For { iter, .. } => {
                        collect_vm_ctfe_expr_calls(iter, calls)
                    }
                    crate::ast::ComprehensionClause::If(condition) => {
                        collect_vm_ctfe_expr_calls(condition, calls)
                    }
                }
            }
        }
        ExprKind::Named { value, .. } => collect_vm_ctfe_expr_calls(value, calls),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_vm_ctfe_expr_calls(cond, calls);
            collect_vm_ctfe_expr_calls(then_branch, calls);
            collect_vm_ctfe_expr_calls(else_branch, calls);
        }
        ExprKind::Compare { first, rest } => {
            collect_vm_ctfe_expr_calls(first, calls);
            for (_, value) in rest {
                collect_vm_ctfe_expr_calls(value, calls);
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            collect_vm_ctfe_expr_calls(object, calls);
            for value in [lower, upper, step].into_iter().flatten() {
                collect_vm_ctfe_expr_calls(value, calls);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            collect_vm_ctfe_expr_calls(object, calls);
            for argument in args {
                match argument {
                    crate::ast::SubscriptArg::Index(value)
                    | crate::ast::SubscriptArg::Keyword { value, .. } => {
                        collect_vm_ctfe_expr_calls(value, calls)
                    }
                    crate::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            collect_vm_ctfe_expr_calls(value, calls);
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let crate::ast::TStringPart::Expr(value) = part {
                    collect_vm_ctfe_expr_calls(value, calls);
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Uninitialized
        | ExprKind::Identifier(_)
        | ExprKind::TypeValue(_) => {}
    }
}

fn collect_vm_ctfe_block_calls(statements: &[Stmt], calls: &mut HashSet<String>) {
    for statement in statements {
        collect_vm_ctfe_stmt_calls(statement, calls);
    }
}

fn mk(kind: StmtKind, span: Span) -> Stmt {
    Stmt {
        kind,
        span,
        module: None,
        syntax_id: crate::token::SyntaxId::fresh(),
    }
}

/// Whether a block directly contains a `comptime if`/`comptime for` (not descending
/// into nested `def`/`struct`, which have their own compile-time scope).
fn block_has_comptime(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_comptime)
}

fn collect_reference_origin_parameters(
    ty: &Ty,
    origins: &mut HashMap<crate::origin::OriginParamId, crate::origin::Mutability>,
) -> Option<()> {
    match ty {
        Ty::Ref(reference) => {
            let crate::origin::Origin::Param(id) = &reference.origin else {
                if matches!(&reference.origin, crate::origin::Origin::Untracked { .. }) {
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
        Ty::Dependent(crate::types::DependentType::Indexed { elements, .. }) => elements
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
        Type::Assoc { base, .. } => {
            substitute_source_type_binding(base, binding, replacement);
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

fn infer_pack_argument_type(expr: &Expr) -> Result<Ty, ComptimeError> {
    match &expr.kind {
        ExprKind::Int(_) => Ok(Ty::Int),
        ExprKind::Float(_) => Ok(Ty::Float64),
        ExprKind::Bool(_) => Ok(Ty::Bool),
        ExprKind::Str(_) => Ok(Ty::StringLiteral),
        ExprKind::None => Ok(Ty::None),
        ExprKind::Call { name, .. } => Ok(match name.as_str() {
            "Int" => Ty::Int,
            "UInt" => Ty::UInt,
            "Float64" => Ty::Float64,
            "Bool" => Ty::Bool,
            "String" => Ty::StringLiteral,
            other => Ty::Struct(other.to_string(), Vec::new()),
        }),
        ExprKind::Prefix(_, value) | ExprKind::Transfer(value) => infer_pack_argument_type(value),
        ExprKind::Infix(op, left, right) => {
            let left = infer_pack_argument_type(left)?;
            let right = infer_pack_argument_type(right)?;
            if matches!(op, InfixOp::Eq | InfixOp::Ne | InfixOp::Lt | InfixOp::Le | InfixOp::Gt | InfixOp::Ge | InfixOp::And | InfixOp::Or) {
                return Ok(Ty::Bool);
            }
            if left == right {
                Ok(left)
            } else if matches!((&left, &right), (Ty::Int, Ty::Float64) | (Ty::Float64, Ty::Int)) {
                Ok(Ty::Float64)
            } else {
                Err(ComptimeError::NotComptime(format!(
                    "cannot infer a pack element type for operands {left} and {right}"
                )))
            }
        }
        ExprKind::ListLit(values) => {
            let mut types = values.iter().map(infer_pack_argument_type);
            let first = types.next().transpose()?.ok_or_else(|| {
                ComptimeError::NotComptime("cannot infer an empty list pack argument".to_string())
            })?;
            if types.all(|ty| matches!(ty, Ok(ty) if ty == first)) {
            Ok(list_type(first))
            } else {
                Err(ComptimeError::NotComptime(
                    "a list pack argument must have one element type".to_string(),
                ))
            }
        }
        ExprKind::TupleLit(values) => values
            .iter()
            .map(infer_pack_argument_type)
            .collect::<Result<Vec<_>, _>>()
            .map(tuple_type),
        ExprKind::IfExpr {
            then_branch,
            else_branch,
            ..
        } => {
            let then_ty = infer_pack_argument_type(then_branch)?;
            let else_ty = infer_pack_argument_type(else_branch)?;
            if then_ty == else_ty {
                Ok(then_ty)
            } else {
                Err(ComptimeError::NotComptime(
                    "conditional pack argument branches have different types".to_string(),
                ))
            }
        }
        _ => Err(ComptimeError::NotComptime(
            "a heterogeneous pack specialization needs an expression whose type is statically evident before checking"
                .to_string(),
        )),
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
    origin_names: &HashMap<crate::origin::OriginParamId, String>,
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
                crate::origin::Origin::Param(id) => origin_names.get(id)?.clone(),
                crate::origin::Origin::Untracked { mutable: false } => {
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
    associated: &'a [StructComptime],
    fields: &'a [crate::ast::Param],
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
/// names a struct (a struct-typed value parameter such as `[layout: Layout]`),
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
                    || def_uses_param_simd_width(statement))
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

/// Whether a generic `def` uses one of its parameters as a `SIMD`/`Scalar`
/// width argument, in a signature/annotation type or an expression-position
/// type application. Such a declaration must specialize per call: the width
/// becomes a concrete compile-time value in each instance, so `simd_width`'s
/// power-of-two validation runs during checked elaboration instead of
/// failing on the symbolic template.
fn def_uses_param_simd_width(statement: &Stmt) -> bool {
    let StmtKind::Def {
        type_params,
        params,
        ret,
        body,
        ..
    } = &statement.kind
    else {
        return false;
    };
    let names: Vec<&str> = type_params
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    if names.is_empty() {
        return false;
    }
    params
        .iter()
        .any(|parameter| type_uses_param_simd_width(&parameter.ty, &names))
        || ret
            .as_ref()
            .is_some_and(|ty| type_uses_param_simd_width(ty, &names))
        || body
            .iter()
            .any(|inner| stmt_uses_param_simd_width(inner, &names))
}

fn type_uses_param_simd_width(ty: &Type, names: &[&str]) -> bool {
    match ty {
        Type::Named(name, arguments) => arguments
            .iter()
            .any(|argument| param_arg_uses_simd_width(name == "SIMD", argument, names)),
        Type::Assoc { base, args, .. } => {
            type_uses_param_simd_width(base, names)
                || args
                    .iter()
                    .any(|argument| param_arg_uses_simd_width(false, argument, names))
        }
        Type::IndexedProjection { base, .. } => type_uses_param_simd_width(base, names),
        Type::Func { params, ret, .. } => {
            type_uses_param_simd_width(ret, names)
                || params
                    .iter()
                    .any(|parameter| type_uses_param_simd_width(&parameter.ty, names))
        }
        _ => false,
    }
}

fn param_arg_uses_simd_width(width_position: bool, argument: &ParamArg, names: &[&str]) -> bool {
    match argument {
        ParamArg::Type(inner) => type_uses_param_simd_width(inner, names),
        ParamArg::Value(value) => {
            width_position
                && matches!(&value.kind, ExprKind::Identifier(name) if names.contains(&name.as_str()))
        }
        ParamArg::Named { value, .. } => param_arg_uses_simd_width(width_position, value, names),
    }
}

fn stmt_uses_param_simd_width(statement: &Stmt, names: &[&str]) -> bool {
    let block = |stmts: &[Stmt]| stmts.iter().any(|s| stmt_uses_param_simd_width(s, names));
    let expr = |e: &Expr| expr_uses_param_simd_width(e, names);
    match &statement.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            ty.as_ref()
                .is_some_and(|ty| type_uses_param_simd_width(ty, names))
                || expr(value)
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Comptime { value, .. } => expr(value),
        StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
            expr(place) || expr(value)
        }
        StmtKind::Unpack { targets, value, .. } => targets.iter().any(expr) || expr(value),
        StmtKind::Expr(e) => expr(e),
        StmtKind::Return(value) => value.as_ref().is_some_and(expr),
        StmtKind::Raise(value) => expr(value),
        StmtKind::If { branches, orelse } => {
            branches
                .iter()
                .any(|(cond, body)| expr(cond) || block(body))
                || orelse.as_ref().is_some_and(|body| block(body))
        }
        StmtKind::While { cond, body, orelse } => {
            expr(cond) || block(body) || orelse.as_ref().is_some_and(|body| block(body))
        }
        StmtKind::For { iter, body, .. } => expr(iter) || block(body),
        StmtKind::With { items, body } => {
            items.iter().any(|item| expr(&item.context)) || block(body)
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            block(body)
                || except.as_ref().is_some_and(|(_, handler)| block(handler))
                || orelse.as_ref().is_some_and(|body| block(body))
                || finalbody.as_ref().is_some_and(|body| block(body))
        }
        // A nested def re-binds its own parameter scope.
        _ => false,
    }
}

fn expr_uses_param_simd_width(e: &Expr, names: &[&str]) -> bool {
    let expr = |inner: &Expr| expr_uses_param_simd_width(inner, names);
    let args_use = |width_position: bool, arguments: &[ParamArg]| {
        arguments
            .iter()
            .any(|argument| param_arg_uses_simd_width(width_position, argument, names))
    };
    match &e.kind {
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => {
            args_use(name == "SIMD" || name == "Scalar", param_args)
                || args.iter().any(expr)
                || kwargs.iter().any(|kwarg| expr(&kwarg.value))
        }
        ExprKind::TypeApply { name, args } => args_use(name == "SIMD" || name == "Scalar", args),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            expr(callee)
                || args_use(false, param_args)
                || args.iter().any(expr)
                || kwargs.iter().any(|kwarg| expr(&kwarg.value))
        }
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => expr(object) || args.iter().any(expr) || kwargs.iter().any(|kwarg| expr(&kwarg.value)),
        ExprKind::TypeValue(ty) => type_uses_param_simd_width(ty, names),
        ExprKind::Prefix(_, value) | ExprKind::Spread(value) | ExprKind::Transfer(value) => {
            expr(value)
        }
        ExprKind::Infix(_, left, right) => expr(left) || expr(right),
        ExprKind::Member { object, .. } => expr(object),
        ExprKind::Index { object, index } => expr(object) || expr(index),
        ExprKind::ListLit(elements) | ExprKind::TupleLit(elements) => elements.iter().any(expr),
        ExprKind::BraceLit(entries) => entries
            .iter()
            .any(|(key, value)| expr(key) || value.as_ref().is_some_and(&expr)),
        ExprKind::Named { value, .. } => expr(value),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => expr(cond) || expr(then_branch) || expr(else_branch),
        ExprKind::Compare { first, rest } => {
            expr(first) || rest.iter().any(|(_, operand)| expr(operand))
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            expr(object)
                || [lower, upper, step]
                    .into_iter()
                    .any(|bound| bound.as_ref().is_some_and(|bound| expr(bound)))
        }
        ExprKind::MultiIndex { object, .. } => expr(object),
        _ => false,
    }
}

fn runtime_pack_spread_source(expression: &Expr) -> Option<&str> {
    let ExprKind::Spread(value) = &expression.kind else {
        return None;
    };
    match &value.kind {
        ExprKind::Identifier(name) => Some(name),
        ExprKind::Transfer(value) => match &value.kind {
            ExprKind::Identifier(name) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

fn forwarded_runtime_pack_type(source: &Type) -> Option<Ty> {
    ct_param_source_type(source).or_else(|| match source {
        Type::Named(name, arguments) => arguments
            .iter()
            .map(|argument| match argument {
                ParamArg::Type(ty) => forwarded_runtime_pack_type(ty).map(TyArg::Ty),
                ParamArg::Value(value) => literal_ct_value(value).map(TyArg::Val),
                ParamArg::Named { value, .. } => match &**value {
                    ParamArg::Type(ty) => forwarded_runtime_pack_type(ty).map(TyArg::Ty),
                    ParamArg::Value(value) => literal_ct_value(value).map(TyArg::Val),
                    ParamArg::Named { .. } => None,
                },
            })
            .collect::<Option<Vec<_>>>()
            .map(|arguments| Ty::Struct(name.clone(), arguments)),
        _ => None,
    })
}

fn encode_specialization_value(value: &CtValue, out: &mut String) {
    match value {
        CtValue::Int(value) => out.push_str(&format!("i{value};")),
        CtValue::UInt(value) => out.push_str(&format!("u{value};")),
        CtValue::Dtype(dtype) => out.push_str(&format!("d{};", dtype.name())),
        CtValue::Struct { name, fields } => {
            out.push_str(&format!("S{}:{name}{{", name.len()));
            for (_, value) in fields {
                encode_specialization_value(value, out);
            }
            out.push('}');
        }
        CtValue::Float(bits) => out.push_str(&format!("f{bits:016x};")),
        CtValue::IntLiteral(value) => {
            let rendered = value.to_string();
            out.push_str(&format!("I{}:{rendered}", rendered.len()));
        }
        CtValue::FloatLiteral(value) => {
            let rendered = value.to_string();
            out.push_str(&format!("F{}:{rendered}", rendered.len()));
        }
        CtValue::Bool(value) => out.push_str(if *value { "b1;" } else { "b0;" }),
        CtValue::Str(value) => out.push_str(&format!("s{}:{value}", value.len())),
        CtValue::Tuple(values) => {
            out.push_str(&format!("t{}[", values.len()));
            for value in values {
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        CtValue::List(values) => {
            out.push_str(&format!("l{}[", values.len()));
            for value in values {
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        CtValue::Type(ty) => {
            let rendered = ty.to_string();
            out.push_str(&format!("y{}:{rendered}", rendered.len()));
        }
        CtValue::Reflected(ty) => {
            let rendered = ty.to_string();
            out.push_str(&format!("r{}:{rendered}", rendered.len()));
        }
        CtValue::Param(name) => out.push_str(&format!("p{}:{name}", name.len())),
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
    /// Top-level generic `def`s whose value parameters feed a `comptime if`/`for`
    /// (so they must be monomorphized per call), by name → the template `Stmt`.
    specializable: HashMap<String, &'a Stmt>,
    /// The subset of `specializable` that is a plain trait-bound generic `def`
    /// (no comptime constructs). Calls resolve softly: an explicit concrete
    /// application monomorphizes, every other reference stays on the template's
    /// abstract erased-dispatch path and retains the template.
    bound_generics: HashSet<String>,
    /// Checker-owned declaration facts used to validate inferred pack bounds
    /// before specialization consumes the source generic call.
    conformance: crate::checker::ConformanceOracle,
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
}

fn classify_ct_params(tps: &[TypeParam]) -> Vec<ParamDecl> {
    tps.iter()
        .filter_map(|tp| classify_ct_param(tp, tps))
        .collect()
}

impl<'a> Elab<'a> {
    fn burn(&self) -> Result<(), ComptimeError> {
        let f = self
            .fuel
            .get()
            .checked_sub(1)
            .ok_or(ComptimeError::QuotaExceeded)?;
        self.fuel.set(f);
        Ok(())
    }

    /// Elaborate a block, resolving `comptime` constructs. `in_fn` is true inside a
    /// function/method body (so a comptime constant there is *not* module-level).
    fn block(
        &self,
        stmts: &[Stmt],
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
    ) -> Result<Vec<Stmt>, ComptimeError> {
        let mut out = Vec::new();
        for stmt in stmts {
            let first_new = out.len();
            self.stmt(stmt, env, in_fn, &mut out)?;
            if let Some(source) = stmt.module.as_deref() {
                crate::ast::stamp_source(&mut out[first_new..], source);
            }
        }
        Ok(out)
    }

    fn stmt(
        &self,
        stmt: &Stmt,
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
        out: &mut Vec<Stmt>,
    ) -> Result<(), ComptimeError> {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Comptime { name, value } => {
                let v = self.eval(value, env)?;
                if !in_fn {
                    self.top_consts.borrow_mut().insert(name.clone(), v.clone());
                }
                // Fold the definition to its literal value, so the checker and
                // runtime see a constant (and a CTFE-computed `Int`, which the
                // checker's own folder can't evaluate, becomes usable as a value
                // parameter and materializes cleanly).
                env.insert(name.clone(), v);
                // Type and reflection handles have no runtime representation.
                // Keep them only in the elaboration environment; subsequent
                // comptime expressions consume them before checking/lowering.
                if let Some(value) = env[name].materialize(span) {
                    out.push(mk(
                        StmtKind::Comptime {
                            name: name.clone(),
                            value,
                        },
                        span,
                    ));
                }
            }
            StmtKind::ComptimeIf { branches, orelse } => {
                for (cond, body) in branches {
                    if self.eval(cond, env)?.as_bool("comptime if condition")? {
                        out.extend(self.block(body, env, in_fn)?);
                        return Ok(());
                    }
                }
                if let Some(body) = orelse {
                    out.extend(self.block(body, env, in_fn)?);
                }
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                for v in self.eval_iter(iter, env)? {
                    self.burn()?;
                    let subs: Subs = &|n| (n == var).then(|| v.clone());
                    let substituted: Vec<Stmt> = body
                        .iter()
                        .map(|s| rewrite_stmt_cloned(s, subs, false))
                        .collect();
                    out.extend(self.block(&substituted, env, in_fn)?);
                }
            }
            StmtKind::VarDecl { name, ty, value } => {
                let ty = ty
                    .as_ref()
                    .map(|ty| self.resolve_reflected_type(ty, env))
                    .transpose()?;
                out.push(mk(
                    StmtKind::VarDecl {
                        name: name.clone(),
                        ty,
                        value: value.clone(),
                    },
                    span,
                ));
            }
            StmtKind::If { branches, orelse } => {
                let branches = branches
                    .iter()
                    .map(|(c, b)| Ok((c.clone(), self.block(b, env, in_fn)?)))
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(StmtKind::If { branches, orelse }, span));
            }
            StmtKind::While { cond, body, orelse } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(
                    StmtKind::While {
                        cond: cond.clone(),
                        body,
                        orelse,
                    },
                    span,
                ));
            }
            StmtKind::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(
                    StmtKind::For {
                        var: var.clone(),
                        binding: *binding,
                        iter: iter.clone(),
                        body,
                        orelse,
                    },
                    span,
                ));
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                let body = self.block(body, env, in_fn)?;
                let except = match except {
                    Some((n, b)) => Some((n.clone(), self.block(b, env, in_fn)?)),
                    None => None,
                };
                let orelse = self.opt_block(orelse, env, in_fn)?;
                let finalbody = self.opt_block(finalbody, env, in_fn)?;
                out.push(mk(
                    StmtKind::Try {
                        body,
                        except,
                        orelse,
                        finalbody,
                    },
                    span,
                ));
            }
            StmtKind::With { items, body } => {
                let mut nested = self.block(body, env, in_fn)?;
                for (index, item) in items.iter().enumerate().rev() {
                    let manager = format!("$with{}_{}", span.0, index);
                    let manager_expr = Expr::new(ExprKind::Identifier(manager.clone()), span);
                    let enter = Expr::new(
                        ExprKind::MethodCall {
                            object: Box::new(manager_expr.clone()),
                            method: "__enter__".to_string(),
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        },
                        span,
                    );
                    let enter_statement = match &item.var {
                        Some(name) => mk(
                            StmtKind::VarDecl {
                                name: name.clone(),
                                ty: None,
                                value: enter,
                            },
                            span,
                        ),
                        None => mk(StmtKind::Expr(enter), span),
                    };
                    let exit = Expr::new(
                        ExprKind::MethodCall {
                            object: Box::new(manager_expr),
                            method: "__exit__".to_string(),
                            args: Vec::new(),
                            kwargs: Vec::new(),
                        },
                        span,
                    );
                    nested = vec![
                        mk(
                            StmtKind::VarDecl {
                                name: manager,
                                ty: None,
                                value: item.context.clone(),
                            },
                            span,
                        ),
                        enter_statement,
                        mk(
                            StmtKind::Try {
                                body: nested,
                                except: None,
                                orelse: None,
                                finalbody: Some(vec![mk(StmtKind::Expr(exit), span)]),
                            },
                            span,
                        ),
                    ];
                }
                out.extend(nested);
            }
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
                where_clause,
                body,
            } => {
                // A comptime-dependent generic template can't be elaborated now (its
                // parameter value is unknown); keep it verbatim for monomorphization.
                if is_specializable_declaration(stmt) {
                    out.push(stmt.clone());
                    return Ok(());
                }
                let body = self.block(body, env, true)?;
                out.push(mk(
                    StmtKind::Def {
                        name: name.clone(),
                        decorators: decorators.clone(),
                        type_params: type_params.clone(),
                        params: params.clone(),
                        positional_only: *positional_only,
                        keyword_only: *keyword_only,
                        captures: captures.clone(),
                        raises: *raises,
                        raises_type: raises_type.clone(),
                        ret: ret.clone(),
                        where_clause: where_clause.clone(),
                        body,
                    },
                    span,
                ));
            }
            StmtKind::Struct {
                name,
                decorators,
                type_params,
                conforms,
                callable_conformance,
                conformance_conditions,
                fields,
                associated,
                methods,
                fieldwise_init,
            } => {
                // A variadic struct template's members reference the unbound pack;
                // keep it verbatim for monomorphization (mirrors def templates).
                // DType-/struct-valued parameter templates are kept the same way.
                if self.is_specializable(stmt) {
                    out.push(stmt.clone());
                    return Ok(());
                }
                let methods = methods
                    .iter()
                    .map(|m| {
                        let mut m = m.clone();
                        m.body = self.block(&m.body, env, true)?;
                        Ok(m)
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?;
                out.push(mk(
                    StmtKind::Struct {
                        name: name.clone(),
                        decorators: decorators.clone(),
                        type_params: type_params.clone(),
                        conforms: conforms.clone(),
                        callable_conformance: callable_conformance.clone(),
                        conformance_conditions: conformance_conditions.clone(),
                        fields: fields.clone(),
                        associated: associated.clone(),
                        methods,
                        fieldwise_init: *fieldwise_init,
                    },
                    span,
                ));
            }
            _ => out.push(stmt.clone()),
        }
        Ok(())
    }

    fn opt_block(
        &self,
        block: &Option<Vec<Stmt>>,
        env: &mut HashMap<String, CtValue>,
        in_fn: bool,
    ) -> Result<Option<Vec<Stmt>>, ComptimeError> {
        match block {
            Some(b) => Ok(Some(self.block(b, env, in_fn)?)),
            None => Ok(None),
        }
    }

    fn resolve_ct_arg(
        &self,
        decl: &ParamDecl,
        arg: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        match decl {
            ParamDecl::Type { name, .. } => match arg {
                ParamArg::Type(ty) => self
                    .type_from_anno(ty, scope)
                    .map(|ty| CtValue::Type(Box::new(ty))),
                ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(id),
                    ..
                }) => self.type_value(id, &[], scope),
                ParamArg::Value(Expr {
                    kind: ExprKind::TypeApply { name, args },
                    ..
                }) => self.type_value(name, args, scope),
                ParamArg::Value(expr) => Err(ComptimeError::NotComptime(format!(
                    "type parameter '{name}' needs a type argument, got {expr:?}"
                ))),
                ParamArg::Named { value, .. } => self.resolve_ct_arg(decl, value, scope),
            },
            ParamDecl::Value { name, ty, .. } => match arg {
                ParamArg::Value(expr) => {
                    let value = self.eval(expr, scope)?;
                    materialize_ct_value(value.clone(), ty).ok_or_else(|| {
                        ComptimeError::NotComptime(format!(
                            "value parameter '{name}' expects {ty}, got {value}"
                        ))
                    })
                }
                ParamArg::Type(_) => Err(ComptimeError::NotComptime(format!(
                    "value parameter '{name}' expects a compile-time {ty}, got a type argument"
                ))),
                ParamArg::Named { value, .. } => self.resolve_ct_arg(decl, value, scope),
            },
        }
    }

    fn type_value(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        self.type_from_name(name, args, scope)
            .map(|ty| CtValue::Type(Box::new(ty)))
    }

    /// The built-in type predicate `is_same_type[T, U]()` (roadmap milestone 7): resolve both
    /// type parameters and compare them for equality, yielding a compile-time
    /// `Bool`. Takes exactly two type parameters and no value arguments.
    fn eval_is_same_type(
        &self,
        param_args: &[ParamArg],
        args: &[Expr],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if param_args.len() != 2 || !args.is_empty() {
            return Err(ComptimeError::Arity(
                "is_same_type[T, U]() takes two type parameters and no arguments".to_string(),
            ));
        }
        let a = self.param_arg_type(&param_args[0], scope)?;
        let b = self.param_arg_type(&param_args[1], scope)?;
        Ok(CtValue::Bool(a == b))
    }

    /// Resolve a `[...]` argument that is expected to be a **type** (a type
    /// annotation, a bare type name, or a parameterized type) to a `Ty`.
    fn param_arg_type(
        &self,
        arg: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        match arg {
            ParamArg::Type(t) => self.type_from_anno(t, scope),
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(id),
                ..
            }) => self.type_from_name(id, &[], scope),
            ParamArg::Value(Expr {
                kind: ExprKind::TypeApply { name, args },
                ..
            }) => self.type_from_name(name, args, scope),
            ParamArg::Value(expr) => match self.eval(expr, scope)? {
                CtValue::Type(ty) => Ok(*ty),
                _ => Err(ComptimeError::NotComptime(
                    "expected a type argument".to_string(),
                )),
            },
            ParamArg::Named { value, .. } => self.param_arg_type(value, scope),
        }
    }

    fn type_from_anno(
        &self,
        ty: &Type,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        match ty {
            Type::Int => Ok(Ty::Int),
            Type::UInt => Ok(Ty::UInt),
            Type::Bool => Ok(Ty::Bool),
            Type::StringLiteral => Ok(Ty::StringLiteral),
            Type::Float64 => Ok(Ty::Float64),
            Type::None => Ok(Ty::None),
            Type::Named(name, args) => self.type_from_name(name, args, scope),
            Type::SelfParam(name) => match scope.get(name) {
                Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "Self.{name} is not type-valued"
                ))),
                None => Err(ComptimeError::NotComptime(format!(
                    "unknown compile-time type Self.{name}"
                ))),
            },
            Type::Assoc { base, name, .. } => {
                if let Type::Named(binding, args) = &**base
                    && args.is_empty()
                    && name == "T"
                    && let Some(CtValue::Reflected(ty)) = scope.get(binding)
                {
                    return Ok((**ty).clone());
                }
                let base = self.type_from_anno(base, scope)?;
                match self.associated_value(&base, name)? {
                    CtValue::Type(ty) => Ok(*ty),
                    _ => Err(ComptimeError::NotComptime(format!(
                        "{}.{name} is not type-valued",
                        base
                    ))),
                }
            }
            Type::IndexedProjection { base, index } => {
                let Type::Assoc {
                    base: associated_base,
                    name,
                    ..
                } = base.as_ref()
                else {
                    return Err(ComptimeError::NotComptime(
                        "dependent type indexing requires an associated type sequence".to_string(),
                    ));
                };
                let base_ty = self.type_from_anno(associated_base, scope)?;
                let values = match self.associated_value(&base_ty, name)? {
                    CtValue::Tuple(values) | CtValue::List(values) => values,
                    _ => {
                        return Err(ComptimeError::NotComptime(format!(
                            "{base_ty}.{name} is not a type sequence"
                        )));
                    }
                };
                let index = self.eval(index, scope)?.as_int("dependent type index")?;
                match usize::try_from(index)
                    .ok()
                    .and_then(|position| values.get(position))
                {
                    Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                    Some(_) => Err(ComptimeError::NotComptime(format!(
                        "{base_ty}.{name}[{index}] is not type-valued"
                    ))),
                    None => Err(ComptimeError::BadArithmetic(format!(
                        "dependent type index {index} out of range"
                    ))),
                }
            }
            Type::Ref { referent, origin } => {
                let [origin] = origin.as_deref().ok_or_else(|| {
                    ComptimeError::NotComptime(
                        "reference type arguments require one explicit origin".to_string(),
                    )
                })?
                else {
                    return Err(ComptimeError::NotComptime(
                        "reference type arguments require one explicit origin".to_string(),
                    ));
                };
                let ExprKind::Identifier(origin_name) = &origin.kind else {
                    return Err(ComptimeError::NotComptime(
                        "reference type arguments require a named origin".to_string(),
                    ));
                };
                let referent = Box::new(self.type_from_anno(referent, scope)?);
                if origin_name == "UntrackedOrigin" {
                    return Ok(Ty::Ref(crate::origin::RefTy {
                        referent,
                        origin: crate::origin::Origin::Untracked { mutable: false },
                        mutability: crate::origin::Mutability::Immutable,
                    }));
                }
                let mut reference = scope
                    .get(origin_name)
                    .and_then(decode_ct_origin_marker)
                    .ok_or_else(|| {
                        ComptimeError::NotComptime(format!(
                            "unknown compile-time origin '{origin_name}' in reference type argument"
                        ))
                    })?;
                reference.referent = referent;
                Ok(Ty::Ref(reference))
            }
            Type::SelfType | Type::Func { .. } | Type::MaterializedCallable(_) => Err(
                ComptimeError::NotComptime("unsupported compile-time type argument".to_string()),
            ),
        }
    }

    fn type_from_name(
        &self,
        name: &str,
        args: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<Ty, ComptimeError> {
        if args.is_empty() {
            if let Some(CtValue::Type(ty)) = scope.get(name) {
                return Ok((**ty).clone());
            }
            if let Some(ty) = scalar_type_name(name) {
                return Ok(ty);
            }
        }
        // In type-argument grammar, `types[i]` is represented as a named type
        // application. A reflected `field_types()` result is a compile-time
        // sequence of type values, so interpret that spelling as dependent
        // type-list indexing.
        if let Some(CtValue::Tuple(values) | CtValue::List(values)) = scope.get(name)
            && let [ParamArg::Value(index)] = args
        {
            let index = self.eval(index, scope)?.as_int("type-list index")?;
            return match values.get(index as usize) {
                Some(CtValue::Type(ty)) => Ok((**ty).clone()),
                Some(_) => Err(ComptimeError::NotComptime(format!(
                    "'{name}[{index}]' is not type-valued"
                ))),
                None => Err(ComptimeError::BadArithmetic(format!(
                    "type-list index {index} out of range"
                ))),
            };
        }
        let Some(info) = self.structs.get(name) else {
            return Err(ComptimeError::NotComptime(format!(
                "'{name}' is not a compile-time type"
            )));
        };
        if args.len() != info.decls.len() {
            return Err(ComptimeError::Arity(format!(
                "type '{name}' expects {} compile-time argument(s), got {}",
                info.decls.len(),
                args.len()
            )));
        }
        let tyargs = info
            .decls
            .iter()
            .zip(args)
            .map(|(decl, arg)| {
                let value = self.resolve_ct_arg(decl, arg, scope)?;
                match (decl, value) {
                    (ParamDecl::Type { .. }, CtValue::Type(ty)) => Ok(TyArg::Ty(*ty)),
                    (ParamDecl::Type { name, .. }, _) => Err(ComptimeError::NotComptime(format!(
                        "type parameter '{name}' needs a type argument"
                    ))),
                    (ParamDecl::Value { .. }, value) => Ok(TyArg::Val(value)),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Ty::Struct(name.to_string(), tyargs))
    }

    fn associated_value(&self, base: &Ty, member: &str) -> Result<CtValue, ComptimeError> {
        let Ty::Struct(name, args) = base else {
            return Err(ComptimeError::NotComptime(format!(
                "type '{base}' has no compile-time member '{member}'"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("unknown compile-time struct '{name}'"))
        })?;
        let assoc = info
            .associated
            .iter()
            .find(|a| a.name == member)
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "type '{base}' has no compile-time member '{member}'"
                ))
            })?;
        let mut env = HashMap::new();
        for (decl, arg) in info.decls.iter().zip(args) {
            match (decl, arg) {
                (ParamDecl::Type { name, .. }, TyArg::Ty(ty)) => {
                    env.insert(name.clone(), CtValue::Type(Box::new(ty.clone())));
                }
                (ParamDecl::Value { name, .. }, TyArg::Val(value)) => {
                    env.insert(name.clone(), value.clone());
                }
                _ => {}
            }
        }
        self.eval(&assoc.value, &env)
    }
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

fn runtime_pack_call_argument_indices(
    template: &Stmt,
    display_name: &str,
    positional_count: usize,
    kwargs: &[crate::ast::KwArg],
) -> Result<Vec<usize>, ComptimeError> {
    let StmtKind::Def {
        params,
        positional_only,
        keyword_only,
        ..
    } = &template.kind
    else {
        return Err(ComptimeError::NotComptime(format!(
            "specialization registry entry '{display_name}' is not a function"
        )));
    };
    let regular: Vec<_> = params
        .iter()
        .filter(|parameter| {
            parameter.kind == crate::ast::ParamKind::Regular
                && !matches!(parameter.convention, Some(crate::ast::ArgConvention::Out))
        })
        .collect();
    let variadic = params
        .iter()
        .position(|parameter| parameter.kind == crate::ast::ParamKind::Variadic);
    let kw_variadic = params
        .iter()
        .any(|parameter| parameter.kind == crate::ast::ParamKind::KwVariadic);
    let marker = |source: Option<usize>| {
        source.map(|index| {
            params[..index]
                .iter()
                .filter(|parameter| {
                    parameter.kind == crate::ast::ParamKind::Regular
                        && !matches!(parameter.convention, Some(crate::ast::ArgConvention::Out))
                })
                .count()
        })
    };
    let keyword_only = [marker(*keyword_only), marker(variadic)]
        .into_iter()
        .flatten()
        .min()
        .or_else(|| effective_keyword_only_index(params, *keyword_only, variadic));
    let keyword_names: Vec<_> = kwargs
        .iter()
        .map(|argument| argument.name.as_str())
        .collect();
    let matched = match_call_slots(
        &regular
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>(),
        &regular
            .iter()
            .map(|parameter| parameter.default.is_none())
            .collect::<Vec<_>>(),
        marker(*positional_only),
        keyword_only,
        positional_count,
        &keyword_names,
        CallVariadics {
            positional: variadic.is_some(),
            keyword: kw_variadic,
        },
    )
    .map_err(|error| {
        ComptimeError::Arity(format!(
            "call to '{display_name}' cannot bind its heterogeneous pack: {error:?}"
        ))
    })?;
    Ok(matched.positional_overflow)
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
    /// abstract path (an unresolvable call or a function-value use). The
    /// program rebuild keeps these templates alongside their specializations.
    retained: HashSet<String>,
    /// Checker-discovered inferred bound-generic applications: call occurrence
    /// (without its syntax id) → the concrete clone that call selects.
    def_call_targets: HashMap<SourceSpan, DefCallTarget>,
}

impl Mono {
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
        // A SIMD width parameter is a compile-time Int value parameter;
        // `SIMDSize` is the deprecated transitional spelling.
        "SIMDLength" => Some(Ty::Int),
        "SIMDSize" => Some(Ty::Int),
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

fn tuple_specialization_values(elements: &[Ty]) -> Vec<CtValue> {
    vec![CtValue::Tuple(
        elements
            .iter()
            .cloned()
            .map(Box::new)
            .map(CtValue::Type)
            .collect(),
    )]
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
                    associated,
                    fields,
                    fieldwise: *fieldwise_init || mirrored_init,
                },
            );
        }
    }
    structs
}

fn collect_vm_ctfe_stmt_calls(statement: &Stmt, calls: &mut HashSet<String>) {
    let decorators = |decorators: &[crate::ast::Decorator], calls: &mut HashSet<String>| {
        for decorator in decorators {
            for argument in &decorator.args {
                collect_vm_ctfe_expr_calls(argument, calls);
            }
            for argument in &decorator.kwargs {
                collect_vm_ctfe_expr_calls(&argument.value, calls);
            }
        }
    };
    let parameters = |parameters: &[FnParam], calls: &mut HashSet<String>| {
        for parameter in parameters {
            if let Some(default) = &parameter.default {
                collect_vm_ctfe_expr_calls(default, calls);
            }
        }
    };

    match &statement.kind {
        StmtKind::VarDecl { value, .. }
        | StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Comptime { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value))
        | StmtKind::Expr(value) => collect_vm_ctfe_expr_calls(value, calls),
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            collect_vm_ctfe_expr_calls(place, calls);
            collect_vm_ctfe_expr_calls(value, calls);
        }
        StmtKind::Unpack { targets, value, .. } => {
            for target in targets {
                collect_vm_ctfe_expr_calls(target, calls);
            }
            collect_vm_ctfe_expr_calls(value, calls);
        }
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (condition, body) in branches {
                collect_vm_ctfe_expr_calls(condition, calls);
                collect_vm_ctfe_block_calls(body, calls);
            }
            if let Some(body) = orelse {
                collect_vm_ctfe_block_calls(body, calls);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            collect_vm_ctfe_expr_calls(cond, calls);
            collect_vm_ctfe_block_calls(body, calls);
            if let Some(body) = orelse {
                collect_vm_ctfe_block_calls(body, calls);
            }
        }
        StmtKind::For {
            iter, body, orelse, ..
        } => {
            collect_vm_ctfe_expr_calls(iter, calls);
            collect_vm_ctfe_block_calls(body, calls);
            if let Some(body) = orelse {
                collect_vm_ctfe_block_calls(body, calls);
            }
        }
        StmtKind::ComptimeFor { iter, body, .. } => {
            collect_vm_ctfe_expr_calls(iter, calls);
            collect_vm_ctfe_block_calls(body, calls);
        }
        StmtKind::With { items, body } => {
            for item in items {
                collect_vm_ctfe_expr_calls(&item.context, calls);
            }
            collect_vm_ctfe_block_calls(body, calls);
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            collect_vm_ctfe_block_calls(body, calls);
            if let Some((_, body)) = except {
                collect_vm_ctfe_block_calls(body, calls);
            }
            if let Some(body) = orelse {
                collect_vm_ctfe_block_calls(body, calls);
            }
            if let Some(body) = finalbody {
                collect_vm_ctfe_block_calls(body, calls);
            }
        }
        StmtKind::Def {
            decorators: declaration_decorators,
            params,
            where_clause,
            body,
            ..
        } => {
            decorators(declaration_decorators, calls);
            parameters(params, calls);
            if let Some(condition) = where_clause {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            collect_vm_ctfe_block_calls(body, calls);
        }
        StmtKind::Struct {
            decorators: declaration_decorators,
            conformance_conditions,
            associated,
            methods,
            ..
        } => {
            decorators(declaration_decorators, calls);
            for (_, condition) in conformance_conditions {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            for member in associated {
                collect_vm_ctfe_expr_calls(&member.value, calls);
            }
            for method in methods {
                decorators(&method.decorators, calls);
                parameters(&method.params, calls);
                if let Some(condition) = &method.where_clause {
                    collect_vm_ctfe_expr_calls(condition, calls);
                }
                collect_vm_ctfe_block_calls(&method.body, calls);
            }
        }
        StmtKind::Trait { methods, .. } => {
            for method in methods {
                parameters(&method.params, calls);
                if let Some(condition) = &method.where_clause {
                    collect_vm_ctfe_expr_calls(condition, calls);
                }
                if let Some(body) = &method.default_body {
                    collect_vm_ctfe_block_calls(body, calls);
                }
            }
        }
        StmtKind::Return(None)
        | StmtKind::Import { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue => {}
    }
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

/// Whether a source parameter is semantic metadata/runtime callable input rather
/// than a value the compile-time evaluator may inspect.  These parameters stay
/// on every generated specialization, and their call arguments stay at the
/// rewritten call site.
///
/// An unqualified `F: def(...)` is a callable *type constraint* and therefore is
/// still an ordinary type parameter.  Mojo's explicit `thin`/`capturing[...]`
/// forms declare a compile-time callable value; evaluating that value as a
/// [`CtValue`] would incorrectly require the compile-time universe to own VM
/// closures and captured storage.
fn retained_specialization_param(tp: &TypeParam, siblings: &[TypeParam]) -> bool {
    if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
        return true;
    }
    if tp.is_origin_mutability_binder(siblings) {
        return true;
    }
    matches!(
        tp.callable_bound.as_ref(),
        Some(Type::Func { thin: true, .. })
            | Some(Type::Func {
                capturing: Some(_),
                ..
            })
    )
}

/// Classify one source parameter that participates in compile-time evaluation.
/// `None` means the parameter is retained symbolically by specialization.
/// The concrete source type to substitute for a specialization type parameter
/// that is dropped from the clone's signature and calls, or `None` when the
/// parameter must remain symbolic: type packs, callable-value bindings,
/// constrained parameters, and types that do not round-trip to source syntax
/// (such as origin-carrying references). The resolver (`resolve_spec_args_for`)
/// and the clone generator (`generate_def_spec`) must agree on this decision,
/// so both consult this one predicate.
fn spec_type_param_substitution(decl: &ParamDecl, value: &CtValue) -> Option<Type> {
    let ParamDecl::Type {
        variadic: false,
        callable_bound: None,
        constraints,
        ..
    } = decl
    else {
        return None;
    };
    if !constraints.is_empty() {
        return None;
    }
    let CtValue::Type(ty) = value else {
        return None;
    };
    source_type_from_ty(ty)
}

/// The registry-aware form of [`classify_ct_param`]: a single bound naming a
/// struct classifies as a struct-typed **value** parameter.
fn classify_ct_param_with(
    tp: &TypeParam,
    siblings: &[TypeParam],
    is_value_struct: &dyn Fn(&str) -> bool,
) -> Option<ParamDecl> {
    if let [only] = tp.bounds.as_slice()
        && !retained_specialization_param(tp, siblings)
        && tp.value_type.is_none()
        && ct_value_param_type(only).is_none()
        && is_value_struct(only)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(Ty::Struct(only.clone(), Vec::new())),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    classify_ct_param(tp, siblings)
}

fn classify_ct_param(tp: &TypeParam, siblings: &[TypeParam]) -> Option<ParamDecl> {
    if retained_specialization_param(tp, siblings) {
        return None;
    }
    if let Some(source_type) = &tp.value_type
        && let Some(ty) = ct_param_source_type(source_type)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(ty),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    if let [only] = tp.bounds.as_slice()
        && let Some(ty) = ct_value_param_type(only)
    {
        return Some(ParamDecl::Value {
            name: tp.name.clone(),
            ty: Box::new(ty),
            default: tp.default.as_ref().and_then(ct_expr_from_ast),
            callable_default: None,
            infer_only: tp.infer_only,
            variadic: tp.name.starts_with('*'),
            constraints: Vec::new(),
        });
    }
    Some(ParamDecl::Type {
        name: tp.name.clone(),
        bounds: tp.bounds.clone(),
        callable_bound: None,
        default: tp.default.as_ref().and_then(|value| match &value.kind {
            ExprKind::Identifier(name) => scalar_type_name(name).map(Box::new),
            ExprKind::TypeValue(ty) => ct_param_source_type(ty).map(Box::new),
            _ => None,
        }),
        infer_only: tp.infer_only,
        variadic: tp.name.starts_with('*'),
        constraints: Vec::new(),
    })
}

fn decode_ct_origin_marker(value: &CtValue) -> Option<crate::origin::RefTy> {
    let CtValue::Param(marker) = value else {
        return None;
    };
    let marker = marker.strip_prefix("$tuple-origin:")?;
    let (index, permission) = marker.split_once(':')?;
    let id = crate::origin::OriginParamId(index.parse().ok()?);
    let mutability = match permission {
        "imm" => crate::origin::Mutability::Immutable,
        "mut" => crate::origin::Mutability::Mutable,
        "param" => crate::origin::Mutability::Param(id),
        _ => return None,
    };
    Some(crate::origin::RefTy {
        // Filled by `type_from_anno` after the marker establishes provenance.
        referent: Box::new(Ty::None),
        origin: crate::origin::Origin::Param(id),
        mutability,
    })
}

fn ct_value_param_type(name: &str) -> Option<Ty> {
    Some(match name {
        "Int" => Ty::Int,
        // A `[dtype: DType]` value parameter; compile-time-only.
        "DType" => Ty::Dtype,
        // A SIMD width parameter is a compile-time Int value parameter;
        // `SIMDSize` is the deprecated transitional spelling.
        "SIMDLength" => Ty::Int,
        "SIMDSize" => Ty::Int,
        "Bool" => Ty::Bool,
        "String" => Ty::StringLiteral,
        "StringLiteral" => Ty::StringLiteral,
        "UInt" => Ty::UInt,
        "Float64" => Ty::Float64,
        // The prelude rewrite qualifies `String` bounds like any other name;
        // a `[text: String]` value parameter keeps the compile-time string
        // type regardless of the nominal stdlib struct.
        _ if crate::symbol::is_stdlib_string_struct(name) => Ty::StringLiteral,
        _ => return None,
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

/// The specialized name for `orig` at value arguments `vals` — e.g. `f$0`, `f$1`.
/// `$` cannot appear in a source identifier, so a specialization never collides
/// with a user-written name.
fn mangle(orig: &str, vals: &[CtValue]) -> String {
    let mut s = orig.to_string();
    for v in vals {
        s.push('$');
        encode_specialization_value(v, &mut s);
    }
    s
}

/// CTFE does not evaluate an Origin as a runtime value, but nested type
/// annotations still need its stable declaration-order identity while the
/// monomorphizer resolves a variadic Tuple element pack. Encode that semantic
/// fact in the existing non-materializable `Param` carrier for the duration of
/// the enclosing struct walk.
fn ct_origin_marker(index: usize, mutability: crate::origin::Mutability) -> CtValue {
    let permission = match mutability {
        crate::origin::Mutability::Immutable => "imm",
        crate::origin::Mutability::Mutable => "mut",
        crate::origin::Mutability::Param(_) => "param",
    };
    CtValue::Param(format!("$tuple-origin:{index}:{permission}"))
}

fn ct_value_has_type(value: &CtValue, ty: &Ty) -> bool {
    materialize_ct_value(value.clone(), ty).is_some()
}

fn top_level_whole_pack_forwarding_call(
    template: &Stmt,
    arguments: &[Expr],
) -> Result<bool, ComptimeError> {
    let spreads = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, argument)| runtime_pack_spread_source(argument).map(|_| index))
        .collect::<Vec<_>>();
    if spreads.is_empty() {
        return Ok(false);
    }
    if spreads.len() != 1 {
        return Err(ComptimeError::NotComptime(
            "concatenating unpacked positional arguments is not supported; a call may contain at most one runtime-pack spread"
                .to_string(),
        ));
    }
    let StmtKind::Def { params, .. } = &template.kind else {
        return Err(ComptimeError::NotComptime(
            "runtime-pack forwarding requires a function target".to_string(),
        ));
    };
    let Some(pack_index) = params
        .iter()
        .position(|parameter| parameter.kind == ParamKind::Variadic)
    else {
        return Err(ComptimeError::NotComptime(
            "a runtime-pack spread requires a variadic target".to_string(),
        ));
    };
    let parameter = &params[pack_index];
    if !matches!(&parameter.ty, Type::Named(name, arguments)
        if name.starts_with('*') && arguments.is_empty())
    {
        return Err(ComptimeError::NotComptime(
            "a heterogeneous runtime-pack spread requires a type-pack variadic target".to_string(),
        ));
    }
    let positional_prefix = params[..pack_index]
        .iter()
        .filter(|parameter| {
            parameter.kind == ParamKind::Regular
                && !matches!(parameter.convention, Some(crate::ast::ArgConvention::Out))
        })
        .count();
    if spreads[0] != positional_prefix || arguments.len() != positional_prefix + 1 {
        return Err(ComptimeError::NotComptime(
            "a runtime-pack spread must follow the fully supplied fixed positional prefix and cannot be mixed with explicit overflow arguments"
                .to_string(),
        ));
    }
    Ok(true)
}

fn top_level_forwarded_pack_types(
    template: &Stmt,
    display_name: &str,
    arguments: &[Expr],
    kwargs: &[crate::ast::KwArg],
    mono: &Mono,
) -> Result<Option<Vec<Ty>>, ComptimeError> {
    if !arguments
        .iter()
        .any(|argument| runtime_pack_spread_source(argument).is_some())
    {
        return Ok(None);
    }
    let mut logical_types = Vec::new();
    for argument in arguments {
        if let Some(name) = runtime_pack_spread_source(argument) {
            let pack = mono.resolve_runtime_pack(name).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "cannot forward '{name}' because it is not a specialized runtime pack"
                ))
            })?;
            for ty in pack {
                logical_types.push(forwarded_runtime_pack_type(ty).ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "cannot recover the checked type of forwarded pack element '{ty:?}'"
                    ))
                })?);
            }
        } else {
            logical_types.push(infer_pack_argument_type(argument)?);
        }
    }
    let indices =
        runtime_pack_call_argument_indices(template, display_name, logical_types.len(), kwargs)?;
    Ok(Some(
        indices
            .into_iter()
            .map(|index| logical_types[index].clone())
            .collect(),
    ))
}

fn unwrap_runtime_pack_arguments(arguments: Vec<Expr>) -> Vec<Expr> {
    arguments
        .into_iter()
        .map(|argument| match argument.kind {
            ExprKind::Spread(value) => *value,
            _ => argument,
        })
        .collect()
}

/// Select one concrete element from a specialized Tuple's private runtime
/// storage. Tuple transforms are synthesized only after the element pack is
/// concrete, so this ordinary index expression reaches checking/MIR with a
/// statically known index and element type.
fn tuple_storage_element(owner: &str, index: usize, transfer: bool, span: Span) -> Expr {
    let owner = Expr::new(ExprKind::Identifier(owner.to_string()), span);
    let storage = Expr::new(
        ExprKind::Member {
            object: Box::new(owner),
            field: "storage".to_string(),
        },
        span,
    );
    let element = Expr::new(
        ExprKind::Index {
            object: Box::new(storage),
            index: Box::new(Expr::new(ExprKind::Int((index as i64).into()), span)),
        },
        span,
    );
    if transfer {
        Expr::new(ExprKind::Transfer(Box::new(element)), span)
    } else {
        element
    }
}

/// Build an ordinary concrete Tuple transform. Keeping these as normal source
/// AST methods means the checker, HIR, MIR, and VM use their existing method and
/// constructor paths; Tuple does not acquire an execution-only VM intrinsic.
fn tuple_transform_method(
    name: &str,
    self_convention: Option<ArgConvention>,
    params: Vec<FnParam>,
    target: String,
    args: Vec<Expr>,
    span: Span,
) -> crate::ast::Method {
    let result = Expr::new(
        ExprKind::Call {
            name: target.clone(),
            param_args: Vec::new(),
            args,
            kwargs: Vec::new(),
        },
        span,
    );
    crate::ast::Method {
        name: name.to_string(),
        type_params: Vec::new(),
        has_self: true,
        self_convention,
        self_origin: None,
        decorators: Vec::new(),
        params,
        positional_only: None,
        keyword_only: None,
        raises: false,
        raises_type: None,
        ret: Some(Type::Named(target, Vec::new())),
        where_clause: None,
        body: vec![mk(StmtKind::Return(Some(result)), span)],
    }
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
    kwargs: &'a [crate::ast::KwArg],
    consts: &'a HashMap<String, CtValue>,
    request_site: &'a str,
    forwarded_pack_types: Option<&'a [Ty]>,
}

fn runtime_pack_call_arguments<'a>(
    template: &Stmt,
    display_name: &str,
    args: &'a [Expr],
    kwargs: &[crate::ast::KwArg],
) -> Result<Vec<&'a Expr>, ComptimeError> {
    let indices = runtime_pack_call_argument_indices(template, display_name, args.len(), kwargs)?;
    Ok(indices.into_iter().map(|index| &args[index]).collect())
}

/// Give a top-level forwarded specialization the same ownership-safe ABI used
/// by nested whole-pack forwarding: the caller passes its concrete private
/// runtime-pack collector as one regular value and the body binds it directly.
fn select_top_level_whole_pack_abi(specialization: &mut Stmt) -> Result<(), ComptimeError> {
    let StmtKind::Def { params, .. } = &mut specialization.kind else {
        unreachable!("whole-pack specializations are functions")
    };
    let Some(parameter) = params
        .iter_mut()
        .find(|parameter| parameter.kind == ParamKind::Variadic)
    else {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding requires a variadic target".to_string(),
        ));
    };
    let Type::Named(name, _) = &mut parameter.ty else {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding lost its concrete collector type".to_string(),
        ));
    };
    if name != "$pack" {
        return Err(ComptimeError::NotComptime(
            "whole-pack forwarding requires a specialized runtime pack".to_string(),
        ));
    }
    parameter.kind = ParamKind::Regular;
    *name = "__RuntimeTuple".to_string();
    Ok(())
}

fn compare_numeric_values(
    op: InfixOp,
    left: &CtValue,
    right: &CtValue,
) -> Result<bool, ComptimeError> {
    let exact = |value: &CtValue| match value {
        CtValue::Int(value) => Some(crate::literal::FloatLiteral::from_int(
            &crate::literal::IntLiteral::from(*value),
        )),
        CtValue::UInt(value) => Some(crate::literal::FloatLiteral::from_int(
            &crate::literal::IntLiteral::from(*value),
        )),
        CtValue::Float(bits) => crate::literal::FloatLiteral::from_f64(f64::from_bits(*bits)),
        CtValue::IntLiteral(value) => Some(crate::literal::FloatLiteral::from_int(value)),
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
    use crate::{CtValue, Value};

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
    use crate::ast::{ExprKind, StmtKind};
    use crate::types::tuple_type;
    use crate::{Ty, parse};

    const TEMPLATE: &str = "struct Tuple[*Ts: AnyType]:\n    var storage: __RuntimeTuple[*Ts]\n\n";

    fn bare_call(program: &[crate::Stmt]) -> &crate::Expr {
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

    fn struct_names(program: &[crate::Stmt]) -> Vec<&str> {
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
        )
        .expect("materialize checked Tuple specialization");

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
        )
        .expect("materialize contextual Tuple declaration");

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
        )
        .expect("materialize nested Tuple specializations");
        let names = struct_names(&elaborated);

        assert!(names.contains(&inner_symbol.as_str()), "{names:?}");
        assert!(names.contains(&outer_symbol.as_str()), "{names:?}");
    }
}

#[cfg(test)]
mod def_request_tests {
    use super::{DefSpecializationRequest, elaborate_with_requests};
    use crate::ast::{ExprKind, StmtKind};
    use crate::ct::CtValue;
    use crate::types::TyArg;
    use crate::{Ty, parse};

    const TEMPLATE: &str = "def ident[T: Copyable & Movable](x: T) -> T:\n    return x\n\n";

    /// The span of the one inferred (argument-less `[...]`) call to `callee`
    /// inside `main`.
    fn inferred_call_span(program: &[crate::Stmt], callee: &str) -> crate::token::SourceSpan {
        fn find(expr: &crate::Expr, callee: &str) -> Option<crate::token::SourceSpan> {
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

    fn def_names(program: &[crate::Stmt]) -> Vec<&str> {
        program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    fn main_call_names(program: &[crate::Stmt]) -> Vec<String> {
        fn collect(expr: &crate::Expr, out: &mut Vec<String>) {
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

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request])
            .expect("materialize the requested specialization");

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

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request])
            .expect("a skipped request must not fail elaboration");

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

        let elaborated = elaborate_with_requests(parsed, &[], &[], &[request])
            .expect("materialize the requested specialization");

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
