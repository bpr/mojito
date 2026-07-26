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
    StructComptime, Type, TypeParam, WithItem,
};
use crate::backend::VmBackend;
use crate::call::{CallVariadics, effective_keyword_only_index, match_call_slots};
use crate::ct::{CtExpr, CtValue};
use crate::runtime::Value;
use crate::token::{SourceSpan, Span};
use crate::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

/// The maximum number of compile-time "steps" (loop iterations, statements
/// executed, function calls) across a whole program — a hard bound so compile-time
/// execution can't hang the compiler (cf. Zig's quota).
const FUEL: usize = 100_000;

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

/// Canonical concrete symbol selected for public `Tuple[*Ts]` element types.
pub(crate) fn tuple_specialization_symbol(elements: &[Ty]) -> String {
    mangle("Tuple", &tuple_specialization_values(elements))
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
            ComptimeError::QuotaExceeded => {
                write!(f, "compile-time execution exceeded the step quota ({FUEL})")
            }
        }
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
}

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

/// Elaborate all compile-time constructs in a program, returning an ordinary AST.
pub fn elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    elaborate_with_tuple_requests(program, &[])
}

/// Elaborate a program while materializing checker-discovered public `Tuple`
/// specializations.  This is a crate-internal staging seam: ordinary callers use
/// [`elaborate`], and the compiler's discovery loop supplies requests here.
pub(crate) fn elaborate_with_tuple_requests(
    program: Vec<Stmt>,
    tuple_requests: &[TupleSpecializationRequest],
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
    let elab = Elab {
        program: &program,
        fns: collect_fns(&program),
        structs: collect_structs(&program),
        specializable: collect_specializable(&program),
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
    let mut result = elab.monomorphize(materialized, tuple_requests)?;
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
            ..
        } = &s.kind
        {
            structs.insert(
                name.clone(),
                CtStruct {
                    decls: classify_ct_params(type_params),
                    associated,
                    fields,
                },
            );
        }
    }
    structs
}

/// Whether a declaration must remain a template until a concrete call selects
/// its compile-time arguments. This predicate is intentionally independent of
/// the top-level registry: nested generic pack functions need the same delayed
/// elaboration even though their lexical specialization happens later.
fn is_specializable_declaration(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Def {
            type_params, body, ..
        } => {
            !type_params.is_empty()
                && (block_has_comptime(body)
                    || type_params
                        .iter()
                        .any(|parameter| parameter.name.starts_with('*')))
        }
        StmtKind::Struct { type_params, .. } => type_params
            .iter()
            .any(|parameter| parameter.name.starts_with('*')),
        _ => false,
    }
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
                    crate::ast::SubscriptArg::Index(value) => {
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
        StmtKind::Unpack { targets, value } => {
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
fn collect_specializable(program: &[Stmt]) -> HashMap<String, &Stmt> {
    let mut m = HashMap::new();
    for s in program {
        if is_specializable_declaration(s)
            && let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &s.kind
        {
            m.insert(name.clone(), s);
        }
    }
    m
}

/// Whether a block directly contains a `comptime if`/`comptime for` (not descending
/// into nested `def`/`struct`, which have their own compile-time scope).
fn block_has_comptime(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_comptime)
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
fn retained_specialization_param(tp: &TypeParam) -> bool {
    if matches!(tp.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet") {
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
fn classify_ct_param(tp: &TypeParam) -> Option<ParamDecl> {
    if retained_specialization_param(tp) {
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

fn classify_ct_params(tps: &[TypeParam]) -> Vec<ParamDecl> {
    tps.iter().filter_map(classify_ct_param).collect()
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

fn ct_value_param_type(name: &str) -> Option<Ty> {
    Some(match name {
        "Int" => Ty::Int,
        "Bool" => Ty::Bool,
        "String" => Ty::String,
        "UInt" => Ty::UInt,
        "Float64" => Ty::Float64,
        _ => return None,
    })
}

fn ct_param_source_type(source: &Type) -> Option<Ty> {
    match source {
        Type::Int => Some(Ty::Int),
        Type::UInt => Some(Ty::UInt),
        Type::Bool => Some(Ty::Bool),
        Type::String => Some(Ty::String),
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
                reference,
                owned,
                iter,
                body,
                orelse,
            } => {
                let body = self.block(body, env, in_fn)?;
                let orelse = self.opt_block(orelse, env, in_fn)?;
                out.push(mk(
                    StmtKind::For {
                        var: var.clone(),
                        reference: *reference,
                        owned: *owned,
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
                if is_specializable_declaration(stmt) {
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

    // --- Compile-time evaluation --------------------------------------------

    /// Evaluate a compile-time expression to a `CtValue`. `scope` is the current
    /// variable environment (module constants, or a CTFE call frame's locals).
    fn eval(&self, e: &Expr, scope: &HashMap<String, CtValue>) -> Result<CtValue, ComptimeError> {
        match &e.kind {
            ExprKind::Int(n) => Ok(CtValue::IntLiteral(n.clone())),
            ExprKind::Float(value) => Ok(CtValue::FloatLiteral(value.clone())),
            ExprKind::Bool(b) => Ok(CtValue::Bool(*b)),
            ExprKind::Str(s) => Ok(CtValue::Str(s.clone())),
            ExprKind::Identifier(name) => {
                if let Some(value) = scope.get(name) {
                    return Ok(value.clone());
                }
                self.type_value(name, &[], scope)
            }
            ExprKind::TypeApply { name, args } if name == "reflect" => {
                if args.len() != 1 {
                    return Err(ComptimeError::Arity(
                        "reflect[T] takes exactly one type parameter".to_string(),
                    ));
                }
                Ok(CtValue::Reflected(Box::new(
                    self.param_arg_type(&args[0], scope)?,
                )))
            }
            ExprKind::TypeApply { name, args } => self.type_value(name, args, scope),
            ExprKind::TupleLit(elems) => Ok(CtValue::Tuple(self.eval_all(elems, scope)?)),
            ExprKind::ListLit(elems) => Ok(CtValue::List(self.eval_all(elems, scope)?)),
            ExprKind::Member { object, field } => {
                if let ExprKind::Identifier(name) = &object.kind
                    && name == "Self"
                    && let Some(value) = scope.get(field)
                {
                    return Ok(value.clone());
                }
                match self.eval(object, scope)? {
                    CtValue::Type(ty) => self.associated_value(&ty, field),
                    CtValue::Reflected(ty) if field == "T" => Ok(CtValue::Type(ty)),
                    _ => Err(ComptimeError::NotComptime(format!(
                        "compile-time member access '.{field}' needs a type value"
                    ))),
                }
            }
            ExprKind::Index { object, index } => {
                if let ExprKind::Member {
                    object: reflected,
                    field,
                } = &object.kind
                    && matches!(field.as_str(), "field" | "field_at" | "field_type")
                {
                    if field == "field_type" {
                        return Err(ComptimeError::NotComptime(
                            "Reflected.field_type was removed; use Reflected.field[name]"
                                .to_string(),
                        ));
                    }
                    let CtValue::Reflected(ty) = self.eval(reflected, scope)? else {
                        return Err(ComptimeError::NotComptime(format!(
                            "compile-time reflection selector '{field}' needs a reflect[T] handle"
                        )));
                    };
                    return self.eval_reflected_field_handle(&ty, field, index, scope);
                }
                let seq = self
                    .eval(object, scope)?
                    .as_sequence("indexing a comptime collection")?;
                let i = self.eval(index, scope)?.as_int("comptime index")?;
                seq.get(i as usize).cloned().ok_or_else(|| {
                    ComptimeError::BadArithmetic(format!("comptime index {i} out of range"))
                })
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "__len__" && args.is_empty() && kwargs.is_empty() => {
                let sequence = self
                    .eval(object, scope)?
                    .as_sequence("__len__() of a compile-time collection")?;
                Ok(CtValue::Int(sequence.len() as i64))
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let CtValue::Reflected(ty) = self.eval(object, scope)? else {
                    return Err(ComptimeError::NotComptime(format!(
                        "compile-time reflection method '{method}' needs a reflect[T] handle"
                    )));
                };
                self.eval_reflection_method(&ty, method, scope)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } if args.is_empty() && kwargs.is_empty() => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return Err(ComptimeError::NotComptime(
                        "unsupported parameterized compile-time callable".to_string(),
                    ));
                };
                let CtValue::Reflected(ty) = self.eval(object, scope)? else {
                    return Err(ComptimeError::NotComptime(format!(
                        "compile-time reflection method '{field}' needs a reflect[T] handle"
                    )));
                };
                self.eval_parameterized_reflection_method(&ty, field, param_args, scope)
            }
            ExprKind::Prefix(PrefixOp::Neg, inner) => match self.eval(inner, scope)? {
                CtValue::Int(value) => value.checked_neg().map(CtValue::Int).ok_or_else(|| {
                    ComptimeError::BadArithmetic("compile-time integer overflow".to_string())
                }),
                CtValue::IntLiteral(value) => Ok(CtValue::IntLiteral(value.neg())),
                CtValue::Float(value) => Ok(CtValue::Float((-f64::from_bits(value)).to_bits())),
                CtValue::FloatLiteral(value) => Ok(CtValue::FloatLiteral(value.neg())),
                _ => Err(ComptimeError::NotComptime(
                    "unary '-' expects a compile-time numeric value".to_string(),
                )),
            },
            ExprKind::Prefix(PrefixOp::Not, inner) => {
                Ok(CtValue::Bool(!self.eval(inner, scope)?.as_bool("'not'")?))
            }
            ExprKind::Infix(op, l, r) => self.eval_infix(*op, l, r, scope),
            ExprKind::Compare { first, rest } => {
                let mut left = self.eval(first, scope)?;
                for (op, right) in rest {
                    let r = self.eval(right, scope)?;
                    if !compare_numeric_values(*op, &left, &r)? {
                        return Ok(CtValue::Bool(false));
                    }
                    left = r;
                }
                Ok(CtValue::Bool(true))
            }
            // A built-in compile-time **type predicate** (roadmap milestone 7): `is_same_type[T,
            // U]()` is `Bool` type equality, usable in a `comptime if`.
            ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } if name == "is_same_type" => self.eval_is_same_type(param_args, args, scope),
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "reflect" && args.is_empty() && kwargs.is_empty() => {
                if param_args.len() != 1 {
                    return Err(ComptimeError::Arity(
                        "reflect[T]() takes exactly one type parameter".to_string(),
                    ));
                }
                Ok(CtValue::Reflected(Box::new(
                    self.param_arg_type(&param_args[0], scope)?,
                )))
            }
            ExprKind::Call { name, args, .. } if name == "len" && args.len() == 1 => {
                let sequence = self
                    .eval(&args[0], scope)?
                    .as_sequence("len() of a compile-time collection")?;
                Ok(CtValue::Int(sequence.len() as i64))
            }
            // A call into a pure top-level function → CTFE.
            ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } => {
                let argv = self.eval_all(args, scope)?;
                self.ctfe_call(name, param_args, argv, scope)
            }
            _ => Err(ComptimeError::NotComptime(
                "unsupported compile-time expression".to_string(),
            )),
        }
    }

    fn eval_all(
        &self,
        exprs: &[Expr],
        scope: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        exprs.iter().map(|e| self.eval(e, scope)).collect()
    }

    fn eval_reflection_method(
        &self,
        ty: &Ty,
        method: &str,
        outer_scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if method == "is_struct" {
            return Ok(CtValue::Bool(matches!(ty, Ty::Struct(_, _))));
        }
        let Ty::Struct(name, arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{method}() requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        match method {
            "field_count" => Ok(CtValue::Int(info.fields.len() as i64)),
            "field_names" => Ok(CtValue::Tuple(
                info.fields
                    .iter()
                    .map(|field| CtValue::Str(field.name.clone()))
                    .collect(),
            )),
            "field_types" => {
                let mut scope = outer_scope.clone();
                for (decl, argument) in info.decls.iter().zip(arguments) {
                    let value = match argument {
                        TyArg::Ty(ty) => CtValue::Type(Box::new(ty.clone())),
                        TyArg::Val(value) => value.clone(),
                    };
                    scope.insert(decl.name().trim_start_matches('*').to_string(), value);
                }
                info.fields
                    .iter()
                    .map(|field| {
                        self.type_from_anno(&field.ty, &scope)
                            .map(|ty| CtValue::Type(Box::new(ty)))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(CtValue::Tuple)
            }
            _ => Err(ComptimeError::NotComptime(format!(
                "unsupported reflect[T] method '{method}'"
            ))),
        }
    }

    fn eval_parameterized_reflection_method(
        &self,
        ty: &Ty,
        method: &str,
        parameters: &[ParamArg],
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        if method == "field_type" {
            return Err(ComptimeError::NotComptime(
                "Reflected.field_type was removed; use Reflected.field[name]".to_string(),
            ));
        }
        let Ty::Struct(name, _arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{method} requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        let field_name = match parameters {
            [ParamArg::Value(expr)] => match self.eval(expr, scope)? {
                CtValue::Str(name) => name,
                other => {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field name must be String, got {other}"
                    )));
                }
            },
            [
                ParamArg::Named {
                    name: parameter,
                    value,
                },
            ] if parameter == "name" => match self.resolve_ct_arg(
                &ParamDecl::Value {
                    name: "name".to_string(),
                    ty: Box::new(Ty::String),
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    constraints: Vec::new(),
                },
                value,
                scope,
            )? {
                CtValue::Str(name) => name,
                _ => unreachable!(),
            },
            _ => {
                return Err(ComptimeError::Arity(format!(
                    "reflect[T].{method}[name]() takes one String parameter"
                )));
            }
        };
        let index = info
            .fields
            .iter()
            .position(|field| field.name == field_name)
            .ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "struct '{name}' has no field named '{field_name}'"
                ))
            })?;
        match method {
            "field_index" => Ok(CtValue::Int(index as i64)),
            _ => Err(ComptimeError::NotComptime(format!(
                "unsupported parameterized reflect[T] method '{method}'"
            ))),
        }
    }

    /// Resolve the current type-valued reflected-field aliases.  Both selectors
    /// return another `Reflected` value, rather than the bare type, which makes
    /// nested selection (`reflect[Outer].field["inner"].field_at[0]`) and the
    /// terminal `.T` member use the same representation as the root handle.
    fn eval_reflected_field_handle(
        &self,
        ty: &Ty,
        selector: &str,
        argument: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let Ty::Struct(name, arguments) = ty else {
            return Err(ComptimeError::NotComptime(format!(
                "reflect[{ty}].{selector}[...] requires a struct type"
            )));
        };
        let info = self.structs.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("cannot reflect unknown struct '{name}'"))
        })?;
        let selected = self.eval(argument, scope)?;
        let index = match (selector, &selected) {
            ("field", CtValue::Str(field_name)) => info
                .fields
                .iter()
                .position(|field| field.name == *field_name)
                .ok_or_else(|| {
                    ComptimeError::NotComptime(format!(
                        "struct '{name}' has no field named '{field_name}'"
                    ))
                })?,
            ("field", other) => {
                return Err(ComptimeError::NotComptime(format!(
                    "Reflected.field expects a String field name, got {other}"
                )));
            }
            ("field_at", CtValue::Int(_) | CtValue::IntLiteral(_)) => {
                let raw_index = selected.as_int("reflection field index")?;
                if raw_index < 0 {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field index {raw_index} is out of range for struct '{name}'"
                    )));
                }
                let index = usize::try_from(raw_index).map_err(|_| {
                    ComptimeError::NotComptime(format!(
                        "reflection field index {raw_index} is out of range for struct '{name}'"
                    ))
                })?;
                if index >= info.fields.len() {
                    return Err(ComptimeError::NotComptime(format!(
                        "reflection field index {index} is out of range for struct '{name}' with {} field(s)",
                        info.fields.len()
                    )));
                }
                index
            }
            ("field_at", other) => {
                return Err(ComptimeError::NotComptime(format!(
                    "Reflected.field_at expects an Int field index, got {other}"
                )));
            }
            _ => unreachable!("reflection selector filtered by the caller"),
        };

        let mut type_scope = scope.clone();
        for (decl, argument) in info.decls.iter().zip(arguments) {
            type_scope.insert(
                decl.name().trim_start_matches('*').to_string(),
                match argument {
                    TyArg::Ty(ty) => CtValue::Type(Box::new(ty.clone())),
                    TyArg::Val(value) => value.clone(),
                },
            );
        }
        let field_ty = self.type_from_anno(&info.fields[index].ty, &type_scope)?;
        Ok(CtValue::Reflected(Box::new(field_ty)))
    }

    /// Replace a reflected handle's terminal `.T` with an ordinary source type
    /// before the handle-only comptime binding is erased. This is the handoff
    /// that makes the nightly pattern `comptime f = reflect[S].field["x"]`
    /// followed by `var value: f.T` visible to the regular checker.
    fn resolve_reflected_type(
        &self,
        source: &Type,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Type, ComptimeError> {
        if let Type::Assoc { base, name } = source
            && name == "T"
            && let Type::Named(binding, arguments) = &**base
            && arguments.is_empty()
            && let Some(CtValue::Reflected(ty)) = scope.get(binding)
        {
            return source_type_from_ty(ty).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "reflected type '{ty}' cannot be represented in a source annotation"
                ))
            });
        }

        Ok(match source {
            Type::Named(name, arguments) => Type::Named(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| self.resolve_reflected_param_arg(argument, scope))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Type::Assoc { base, name } => Type::Assoc {
                base: Box::new(self.resolve_reflected_type(base, scope)?),
                name: name.clone(),
            },
            Type::Func {
                type_params,
                params,
                ret,
                thin,
                capturing,
                raises,
                raises_type,
            } => Type::Func {
                type_params: type_params
                    .iter()
                    .map(|parameter| {
                        let mut parameter = parameter.clone();
                        if let Some(value_type) = &mut parameter.value_type {
                            *value_type = self.resolve_reflected_type(value_type, scope)?;
                        }
                        if let Some(callable) = &mut parameter.callable_bound {
                            *callable = self.resolve_reflected_type(callable, scope)?;
                        }
                        Ok(parameter)
                    })
                    .collect::<Result<Vec<_>, ComptimeError>>()?,
                params: params
                    .iter()
                    .map(|param| {
                        let mut param = param.clone();
                        param.ty = self.resolve_reflected_type(&param.ty, scope)?;
                        Ok(param)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                ret: Box::new(self.resolve_reflected_type(ret, scope)?),
                thin: *thin,
                capturing: capturing.clone(),
                raises: *raises,
                raises_type: raises_type
                    .as_deref()
                    .map(|ty| self.resolve_reflected_type(ty, scope).map(Box::new))
                    .transpose()?,
            },
            Type::Ref { referent, origin } => Type::Ref {
                referent: Box::new(self.resolve_reflected_type(referent, scope)?),
                origin: origin.clone(),
            },
            scalar_or_symbolic => scalar_or_symbolic.clone(),
        })
    }

    fn resolve_reflected_param_arg(
        &self,
        argument: &ParamArg,
        scope: &HashMap<String, CtValue>,
    ) -> Result<ParamArg, ComptimeError> {
        Ok(match argument {
            ParamArg::Type(ty) => ParamArg::Type(self.resolve_reflected_type(ty, scope)?),
            ParamArg::Value(value) => ParamArg::Value(value.clone()),
            ParamArg::Named { name, value } => ParamArg::Named {
                name: name.clone(),
                value: Box::new(self.resolve_reflected_param_arg(value, scope)?),
            },
        })
    }

    fn eval_infix(
        &self,
        op: InfixOp,
        l: &Expr,
        r: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        match op {
            InfixOp::And => {
                return Ok(CtValue::Bool(
                    self.eval(l, scope)?.as_bool("'and'")?
                        && self.eval(r, scope)?.as_bool("'and'")?,
                ));
            }
            InfixOp::Or => {
                return Ok(CtValue::Bool(
                    self.eval(l, scope)?.as_bool("'or'")?
                        || self.eval(r, scope)?.as_bool("'or'")?,
                ));
            }
            _ => {}
        }
        // String concatenation (`+`) and equality (`==`/`!=`) at compile time.
        if let (CtValue::Str(a), CtValue::Str(b)) = (self.eval(l, scope)?, self.eval(r, scope)?) {
            return match op {
                InfixOp::Add => Ok(CtValue::Str(a + &b)),
                InfixOp::Eq => Ok(CtValue::Bool(a == b)),
                InfixOp::Ne => Ok(CtValue::Bool(a != b)),
                _ => Err(ComptimeError::NotComptime(
                    "unsupported compile-time String operator".to_string(),
                )),
            };
        }
        let left = self.eval(l, scope)?;
        let right = self.eval(r, scope)?;
        use InfixOp::*;
        let bad = |m: &str| ComptimeError::BadArithmetic(m.to_string());
        if matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
            return Ok(CtValue::Bool(compare_numeric_values(op, &left, &right)?));
        }
        match (left, right) {
            (CtValue::Int(a), CtValue::Int(b)) => match op {
                Add => a
                    .checked_add(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Sub => a
                    .checked_sub(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Mul => a
                    .checked_mul(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                FloorDiv if b != 0 => a
                    .checked_div_euclid(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Mod if b != 0 => a
                    .checked_rem_euclid(b)
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                FloorDiv | Mod => Err(bad("division by zero")),
                Pow if b >= 0 => u32::try_from(b)
                    .ok()
                    .and_then(|exponent| a.checked_pow(exponent))
                    .map(CtValue::Int)
                    .ok_or_else(|| bad("compile-time integer overflow")),
                Pow => Err(bad("negative exponent")),
                _ => Err(ComptimeError::NotComptime(
                    "unsupported compile-time operator".to_string(),
                )),
            },
            (CtValue::IntLiteral(a), CtValue::IntLiteral(b)) => {
                let value = match op {
                    Add => Some(CtValue::IntLiteral(a.add(&b))),
                    Sub => Some(CtValue::IntLiteral(a.sub(&b))),
                    Mul => Some(CtValue::IntLiteral(a.mul(&b))),
                    Div => crate::literal::FloatLiteral::from_int(&a)
                        .div(&crate::literal::FloatLiteral::from_int(&b))
                        .map(CtValue::FloatLiteral),
                    FloorDiv => a.floor_div(&b).map(CtValue::IntLiteral),
                    Mod => a.floor_mod(&b).map(CtValue::IntLiteral),
                    Pow => a.pow(&b).map(CtValue::IntLiteral),
                    Shl => a.shl(&b).map(CtValue::IntLiteral),
                    Shr => a.shr(&b).map(CtValue::IntLiteral),
                    BitAnd => Some(CtValue::IntLiteral(a.bitand(&b))),
                    BitOr => Some(CtValue::IntLiteral(a.bitor(&b))),
                    BitXor => Some(CtValue::IntLiteral(a.bitxor(&b))),
                    _ => {
                        return Err(ComptimeError::NotComptime(
                            "unsupported exact compile-time operator".to_string(),
                        ));
                    }
                };
                value.ok_or_else(|| bad("invalid exact compile-time arithmetic"))
            }
            (CtValue::FloatLiteral(a), CtValue::FloatLiteral(b)) => {
                let value = match op {
                    Add => Some(a.add(&b)),
                    Sub => Some(a.sub(&b)),
                    Mul => Some(a.mul(&b)),
                    Div => a.div(&b),
                    FloorDiv => a.floor_div(&b),
                    Mod => a.floor_mod(&b),
                    Pow => b.to_int_if_whole().and_then(|b| a.pow_int(&b)),
                    _ => {
                        return Err(ComptimeError::NotComptime(
                            "unsupported exact compile-time float operator".to_string(),
                        ));
                    }
                };
                value
                    .map(CtValue::FloatLiteral)
                    .ok_or_else(|| bad("invalid exact compile-time arithmetic"))
            }
            (CtValue::Int(a), CtValue::IntLiteral(b)) => {
                self.eval_infix_values(op, CtValue::IntLiteral(a.into()), CtValue::IntLiteral(b))
            }
            (CtValue::IntLiteral(a), CtValue::Int(b)) => {
                self.eval_infix_values(op, CtValue::IntLiteral(a), CtValue::IntLiteral(b.into()))
            }
            (CtValue::IntLiteral(a), CtValue::FloatLiteral(b)) => self.eval_infix_values(
                op,
                CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&a)),
                CtValue::FloatLiteral(b),
            ),
            (CtValue::FloatLiteral(a), CtValue::IntLiteral(b)) => self.eval_infix_values(
                op,
                CtValue::FloatLiteral(a),
                CtValue::FloatLiteral(crate::literal::FloatLiteral::from_int(&b)),
            ),
            _ => Err(ComptimeError::NotComptime(
                "unsupported compile-time operands".to_string(),
            )),
        }
    }

    fn eval_infix_values(
        &self,
        op: InfixOp,
        left: CtValue,
        right: CtValue,
    ) -> Result<CtValue, ComptimeError> {
        let scope = HashMap::from([("__left".to_string(), left), ("__right".to_string(), right)]);
        let expression = |name: &str| Expr {
            kind: ExprKind::Identifier(name.to_string()),
            span: Span::default(),
            source: None,
            syntax_id: crate::token::SyntaxId::fresh(),
        };
        self.eval_infix(op, &expression("__left"), &expression("__right"), &scope)
    }

    /// Evaluate a `comptime for` / CTFE `for` iterable to the sequence of loop
    /// values: a `range(...)` of `Int`s, or any compile-time tuple/list.
    fn eval_iter(
        &self,
        iter: &Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        if let ExprKind::Call { name, args, .. } = &iter.kind
            && name == "range"
        {
            let vals: Vec<i64> = args
                .iter()
                .map(|a| self.eval(a, scope)?.as_int("range argument"))
                .collect::<Result<_, _>>()?;
            let (start, stop, step) = match vals.as_slice() {
                [stop] => (0, *stop, 1),
                [start, stop] => (*start, *stop, 1),
                [start, stop, step] => (*start, *stop, *step),
                _ => {
                    return Err(ComptimeError::BadRange(
                        "range takes 1-3 arguments".to_string(),
                    ));
                }
            };
            if step == 0 {
                return Err(ComptimeError::BadRange(
                    "range step cannot be zero".to_string(),
                ));
            }
            let mut out = Vec::new();
            let mut i = start;
            while (step > 0 && i < stop) || (step < 0 && i > stop) {
                out.push(CtValue::Int(i));
                i += step;
            }
            return Ok(out);
        }
        self.eval(iter, scope)?
            .as_sequence("a range(...), tuple, or list")
    }

    // --- CTFE: run a pure function at compile time --------------------------

    fn ctfe_call(
        &self,
        name: &str,
        param_args: &[ParamArg],
        args: Vec<CtValue>,
        scope: &HashMap<String, CtValue>,
    ) -> Result<CtValue, ComptimeError> {
        let f = self.fns.get(name).ok_or_else(|| {
            ComptimeError::NotComptime(format!("'{name}' is not a compile-time-callable function"))
        })?;
        if f.ct_params.len() != param_args.len() {
            return Err(ComptimeError::Arity(format!(
                "'{name}' expects {} compile-time argument(s), got {}",
                f.ct_params.len(),
                param_args.len()
            )));
        }
        if f.params.len() != args.len() {
            return Err(ComptimeError::Arity(format!(
                "'{name}' expects {} argument(s), got {}",
                f.params.len(),
                args.len()
            )));
        }
        self.burn()?;
        let mut locals: HashMap<String, CtValue> = HashMap::new();
        let mut value_params = Vec::new();
        for (decl, arg) in f.ct_params.iter().zip(param_args) {
            let value = self.resolve_ct_arg(decl, arg, scope)?;
            if let ParamDecl::Value { name, .. } = decl
                && !matches!(value, CtValue::Type(_))
            {
                value_params.push((name.clone(), ct_to_vm(&value)?));
            }
            locals.insert(decl.name().to_string(), value);
        }
        locals.extend(f.params.iter().cloned().zip(args));
        let mut visiting = HashSet::new();
        let mut needed = HashSet::new();
        let safe = self.vm_ctfe_safe_fn(name, &mut visiting, &mut needed);
        if safe && let Some(value) = self.vm_ctfe_call(name, &locals, &value_params, &needed)? {
            return Ok(value);
        }
        Err(ComptimeError::NotComptime(format!(
            "'{name}' is not safe for VM-backed compile-time execution"
        )))
    }

    /// Build the bounded free-declaration graph required by a VM-CTFE entry.
    /// The purity walk seeds the executed root and its direct free helpers. The
    /// ordinary nominal declarations retained for type checking contribute
    /// their own bare free-function callees (for example Range's length helper),
    /// and a work queue follows those callees transitively. Unrelated top-level
    /// functions never enter the set, so bindings that have not yet been
    /// materialized cannot leak into the checked CTFE subprogram.
    fn vm_ctfe_declaration_closure(&self, needed: &HashSet<String>) -> HashSet<String> {
        let available: HashSet<String> = self
            .program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Def { name, .. } if !is_specializable_declaration(statement) => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        let mut pending: VecDeque<String> = needed.iter().cloned().collect();

        // The ordinary checker validates retained nominal method/default bodies
        // even when the CTFE entry does not invoke them. Seed their actual free
        // callees rather than retaining every `$`-qualified linked symbol.
        for statement in self.program {
            if matches!(&statement.kind, StmtKind::Trait { .. })
                || matches!(&statement.kind, StmtKind::Struct { .. })
                    && !is_specializable_declaration(statement)
            {
                let mut calls = HashSet::new();
                collect_vm_ctfe_stmt_calls(statement, &mut calls);
                pending.extend(calls);
            }
        }

        let mut closure = HashSet::new();
        while let Some(name) = pending.pop_front() {
            if !available.contains(&name) || !closure.insert(name.clone()) {
                continue;
            }
            let mut calls = HashSet::new();
            for statement in self.program {
                if matches!(&statement.kind, StmtKind::Def { name: candidate, .. } if candidate == &name)
                    && !is_specializable_declaration(statement)
                {
                    collect_vm_ctfe_stmt_calls(statement, &mut calls);
                }
            }
            pending.extend(calls);
        }
        closure
    }

    fn vm_ctfe_call(
        &self,
        name: &str,
        locals: &HashMap<String, CtValue>,
        value_params: &[(String, Value)],
        needed: &HashSet<String>,
    ) -> Result<Option<CtValue>, ComptimeError> {
        let Some(f) = self.fns.get(name) else {
            return Ok(None);
        };
        let mut args = Vec::with_capacity(f.params.len());
        for pname in &f.params {
            let value = locals.get(pname).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "missing compile-time argument '{pname}' for VM CTFE call"
                ))
            })?;
            args.push(ct_to_vm(value)?);
        }
        let mut vm = VmBackend::new();
        let declarations = self.vm_ctfe_declaration_closure(needed);
        let mut program = self
            .program
            .iter()
            // The checked boundary needs the declaration environment, not just
            // the transitively executed bodies. Retain all declarations so trait
            // bounds, struct types, overloads, and helper calls resolve exactly;
            // the VM still executes only the requested function.
            .filter(|stmt| match &stmt.kind {
                // Only the semantic free-callee graph crosses this checked
                // boundary. Linked `$` spelling is neither a dependency nor a
                // specialization test.
                StmtKind::Def { name, .. } => declarations.contains(name),
                // A variadic struct template is a monomorphizer input and cannot
                // cross the ordinary checked boundary. Concrete CTFE uses have
                // already been specialized; an unused public `Tuple[*Ts]`
                // template must not invalidate an otherwise scalar subprogram.
                StmtKind::Struct { .. } => !is_specializable_declaration(stmt),
                StmtKind::Trait { .. } => true,
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        self.rewrite_vm_ctfe_program(&mut program, name, locals)?;
        if program.is_empty() {
            return Err(ComptimeError::NotComptime(format!(
                "missing compile-time function '{name}' for VM CTFE"
            )));
        }
        // Preserve declaration order: the ordinary checker intentionally uses
        // source-order visibility for traits and sibling helpers. Execution
        // selects `name` explicitly and does not require it to be first.
        let (value, remaining_fuel) = vm
            .run_function_value(&program, name, args, value_params, self.fuel.get())
            .map_err(|e| ComptimeError::NotComptime(format!("VM CTFE failed for '{name}': {e}")))?;
        self.fuel.set(remaining_fuel);
        Ok(Some(vm_to_ct(value)?))
    }

    fn rewrite_vm_ctfe_program(
        &self,
        program: &mut [Stmt],
        root: &str,
        root_scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        for stmt in program {
            let scope = match &stmt.kind {
                StmtKind::Def { name, .. } if name == root => root_scope,
                _ => {
                    // Non-root helpers with only runtime-value parameters need no
                    // type-fact substitution; recursive value-parameter calls are
                    // handled by the VM's normal value-param reification.
                    continue;
                }
            };
            self.rewrite_vm_ctfe_stmt(stmt, scope)?;
        }
        Ok(())
    }

    fn rewrite_vm_ctfe_block(
        &self,
        stmts: &mut [Stmt],
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        for stmt in stmts {
            self.rewrite_vm_ctfe_stmt(stmt, scope)?;
        }
        Ok(())
    }

    fn rewrite_vm_ctfe_stmt(
        &self,
        stmt: &mut Stmt,
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        // Rewrite only type/comptime facts that the VM cannot evaluate from
        // runtime values; preserve ordinary executable structure.
        match &mut stmt.kind {
            StmtKind::Def { body, .. } => self.rewrite_vm_ctfe_block(body, scope),
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. } => self.rewrite_vm_ctfe_expr(value, scope),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.rewrite_vm_ctfe_expr(place, scope)?;
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            StmtKind::Return(Some(value)) | StmtKind::Expr(value) => {
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            StmtKind::If { branches, orelse } => {
                for (cond, body) in branches {
                    self.rewrite_vm_ctfe_expr(cond, scope)?;
                    self.rewrite_vm_ctfe_block(body, scope)?;
                }
                if let Some(body) = orelse {
                    self.rewrite_vm_ctfe_block(body, scope)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body, .. } => {
                self.rewrite_vm_ctfe_expr(cond, scope)?;
                self.rewrite_vm_ctfe_block(body, scope)
            }
            StmtKind::For { iter, body, .. } => {
                self.rewrite_vm_ctfe_expr(iter, scope)?;
                self.rewrite_vm_ctfe_block(body, scope)
            }
            StmtKind::Return(None) | StmtKind::Pass => Ok(()),
            _ => Ok(()),
        }
    }

    fn rewrite_vm_ctfe_expr(
        &self,
        expr: &mut Expr,
        scope: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        match &mut expr.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "is_same_type" => {
                for arg in param_args.iter_mut() {
                    if let ParamArg::Value(e) = arg {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                let value = self.eval_is_same_type(param_args, args, scope)?;
                *expr = lit_result(&value, expr.span)?;
                Ok(())
            }
            ExprKind::Member { object, .. } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                if let Ok(value) = self.eval(expr, scope)
                    && let Some(materialized) = value.materialize(expr.span)
                {
                    *expr = materialized;
                }
                Ok(())
            }
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.rewrite_vm_ctfe_expr(inner, scope)
            }
            ExprKind::Infix(_, left, right) => {
                self.rewrite_vm_ctfe_expr(left, scope)?;
                self.rewrite_vm_ctfe_expr(right, scope)
            }
            ExprKind::Call {
                param_args,
                args,
                kwargs,
                ..
            } => {
                for arg in param_args.iter_mut() {
                    if let ParamArg::Value(e) = arg {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                Ok(())
            }
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for arg in args.iter_mut() {
                    self.rewrite_vm_ctfe_expr(arg, scope)?;
                }
                for kw in kwargs.iter_mut() {
                    self.rewrite_vm_ctfe_expr(&mut kw.value, scope)?;
                }
                Ok(())
            }
            ExprKind::Index { object, index } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                self.rewrite_vm_ctfe_expr(index, scope)
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.rewrite_vm_ctfe_expr(bound, scope)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.rewrite_vm_ctfe_expr(object, scope)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            self.rewrite_vm_ctfe_expr(value, scope)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.rewrite_vm_ctfe_expr(value, scope)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::ListLit(items) | ExprKind::TupleLit(items) => {
                for item in items {
                    self.rewrite_vm_ctfe_expr(item, scope)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.rewrite_vm_ctfe_expr(key, scope)?;
                    if let Some(value) = value {
                        self.rewrite_vm_ctfe_expr(value, scope)?;
                    }
                }
                Ok(())
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { iter, .. } => {
                            self.rewrite_vm_ctfe_expr(iter, scope)?
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.rewrite_vm_ctfe_expr(condition, scope)?
                        }
                    }
                }
                if let Some(key) = key {
                    self.rewrite_vm_ctfe_expr(key, scope)?;
                }
                self.rewrite_vm_ctfe_expr(value, scope)
            }
            ExprKind::Named { value, .. } => self.rewrite_vm_ctfe_expr(value, scope),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rewrite_vm_ctfe_expr(cond, scope)?;
                self.rewrite_vm_ctfe_expr(then_branch, scope)?;
                self.rewrite_vm_ctfe_expr(else_branch, scope)
            }
            ExprKind::Compare { first, rest } => {
                self.rewrite_vm_ctfe_expr(first, scope)?;
                for (_, e) in rest {
                    self.rewrite_vm_ctfe_expr(e, scope)?;
                }
                Ok(())
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let crate::ast::TStringPart::Expr(e) = part {
                        self.rewrite_vm_ctfe_expr(e, scope)?;
                    }
                }
                Ok(())
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized
            | ExprKind::Identifier(_)
            | ExprKind::TypeValue(_)
            | ExprKind::Invoke { .. }
            | ExprKind::TypeApply { .. } => Ok(()),
        }
    }

    fn vm_ctfe_safe_fn(
        &self,
        name: &str,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        if needed.contains(name) {
            return true;
        }
        if !visiting.insert(name.to_string()) {
            needed.insert(name.to_string());
            return true;
        }
        let Some(f) = self.fns.get(name) else {
            visiting.remove(name);
            return false;
        };
        let safe = self.vm_ctfe_safe_block(f.body, visiting, needed);
        visiting.remove(name);
        if safe {
            needed.insert(name.to_string());
        }
        safe
    }

    fn vm_ctfe_safe_block(
        &self,
        stmts: &[Stmt],
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        stmts
            .iter()
            .all(|s| self.vm_ctfe_safe_stmt(s, visiting, needed))
    }

    fn vm_ctfe_safe_stmt(
        &self,
        stmt: &Stmt,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        // This parallel walk is a purity/effect classifier: it discovers the
        // transitive helper set but never mutates or specializes the AST.
        match &stmt.kind {
            StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. } => self.vm_ctfe_safe_expr(value, visiting, needed),
            StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
                self.vm_ctfe_safe_expr(place, visiting, needed)
                    && self.vm_ctfe_safe_expr(value, visiting, needed)
            }
            StmtKind::Return(Some(value)) | StmtKind::Expr(value) => {
                self.vm_ctfe_safe_expr(value, visiting, needed)
            }
            StmtKind::Return(None) | StmtKind::Pass => true,
            StmtKind::If { branches, orelse } => {
                branches.iter().all(|(cond, body)| {
                    self.vm_ctfe_safe_expr(cond, visiting, needed)
                        && self.vm_ctfe_safe_block(body, visiting, needed)
                }) && orelse
                    .as_ref()
                    .is_none_or(|body| self.vm_ctfe_safe_block(body, visiting, needed))
            }
            StmtKind::While { cond, body, .. } => {
                self.vm_ctfe_safe_expr(cond, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::For { iter, body, .. } => {
                self.vm_ctfe_safe_expr(iter, visiting, needed)
                    && self.vm_ctfe_safe_block(body, visiting, needed)
            }
            StmtKind::ComptimeIf { .. }
            | StmtKind::ComptimeFor { .. }
            | StmtKind::Raise(_)
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Def { .. }
            | StmtKind::Struct { .. }
            | StmtKind::Trait { .. }
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::With { .. }
            | StmtKind::Try { .. }
            | StmtKind::Unpack { .. }
            | StmtKind::Comptime { .. } => false,
        }
    }

    fn vm_ctfe_safe_expr(
        &self,
        expr: &Expr,
        visiting: &mut HashSet<String>,
        needed: &mut HashSet<String>,
    ) -> bool {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Identifier(_) => true,
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.vm_ctfe_safe_expr(inner, visiting, needed)
            }
            ExprKind::Infix(_, left, right) => {
                self.vm_ctfe_safe_expr(left, visiting, needed)
                    && self.vm_ctfe_safe_expr(right, visiting, needed)
            }
            ExprKind::TupleLit(items) | ExprKind::ListLit(items) => items
                .iter()
                .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed)),
            ExprKind::Index { object, index } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && self.vm_ctfe_safe_expr(index, visiting, needed)
            }
            ExprKind::Member { object, .. } => self.vm_ctfe_safe_expr(object, visiting, needed),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.vm_ctfe_safe_expr(cond, visiting, needed)
                    && self.vm_ctfe_safe_expr(then_branch, visiting, needed)
                    && self.vm_ctfe_safe_expr(else_branch, visiting, needed)
            }
            ExprKind::Compare { first, rest } => {
                self.vm_ctfe_safe_expr(first, visiting, needed)
                    && rest
                        .iter()
                        .all(|(_, e)| self.vm_ctfe_safe_expr(e, visiting, needed))
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && lower
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && upper
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && step
                        .as_ref()
                        .is_none_or(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
            }
            ExprKind::MultiIndex { object, args } => {
                self.vm_ctfe_safe_expr(object, visiting, needed)
                    && args.iter().all(|argument| match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            self.vm_ctfe_safe_expr(value, visiting, needed)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => [lower, upper, step]
                            .into_iter()
                            .flatten()
                            .all(|value| self.vm_ctfe_safe_expr(value, visiting, needed)),
                    })
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                kwargs.is_empty()
                    && param_args.iter().all(|arg| match arg {
                        ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
                        ParamArg::Type(_) => true,
                        ParamArg::Named { value, .. } => match &**value {
                            ParamArg::Value(e) => self.vm_ctfe_safe_expr(e, visiting, needed),
                            ParamArg::Type(_) => true,
                            ParamArg::Named { .. } => false,
                        },
                    })
                    && args
                        .iter()
                        .all(|e| self.vm_ctfe_safe_expr(e, visiting, needed))
                    && (name == "is_same_type"
                        || vm_ctfe_safe_builtin(name)
                        || self.vm_ctfe_safe_fn(name, visiting, needed))
            }
            ExprKind::MethodCall { .. }
            | ExprKind::BraceLit(_)
            | ExprKind::Comprehension { .. }
            | ExprKind::Invoke { .. }
            | ExprKind::TypeValue(_)
            | ExprKind::TypeApply { .. }
            | ExprKind::Named { .. }
            | ExprKind::TString { .. }
            | ExprKind::Uninitialized => false,
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
                ParamArg::Type(_) => {
                    Err(ComptimeError::NotInt(format!("value parameter '{name}'")))
                }
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
            Type::String => Ok(Ty::String),
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
            Type::Assoc { base, name } => {
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

    // --- Monomorphization of comptime-dependent generics (roadmap milestone 6)

    /// Specialize every comptime-dependent generic template against the value
    /// arguments at its call sites, replacing each template with its concrete
    /// specializations (which have their `comptime if`/`for` resolved).
    fn monomorphize(
        &self,
        program: Vec<Stmt>,
        tuple_requests: &[TupleSpecializationRequest],
    ) -> Result<Vec<Stmt>, ComptimeError> {
        if self.specializable.is_empty() && tuple_requests.is_empty() {
            return Ok(program);
        }
        if !tuple_requests.is_empty() && !self.struct_template("Tuple") {
            return Err(ComptimeError::NotComptime(
                "checked Tuple specialization requests require a public variadic `Tuple[*Ts]` template"
                    .to_string(),
            ));
        }
        let consts = self.top_consts.borrow().clone();
        let mut mono = Mono::default();
        let mut program = program;
        let mut module_bindings = HashMap::new();
        for statement in &program {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &statement.kind {
                module_bindings.insert(name.clone(), self.specializable.contains_key(name));
            }
        }
        mono.runtime_pack_scopes.push(
            module_bindings
                .keys()
                .map(|name| (name.clone(), None))
                .collect(),
        );
        mono.value_scopes.push(module_bindings);
        for request in tuple_requests {
            let vals = tuple_specialization_values(request.elements());
            let output_name = tuple_specialization_symbol(request.elements());
            if let Some(occurrence) = request.occurrence()
                && let Some(existing) = mono
                    .tuple_call_targets
                    .insert(occurrence.clone().without_syntax(), output_name.clone())
                && existing != output_name
            {
                return Err(ComptimeError::NotComptime(format!(
                    "one bare Tuple call was assigned incompatible specializations '{existing}' and '{output_name}'"
                )));
            }
            if mono.done.insert(output_name.clone()) {
                mono.queue.push_back(Job {
                    orig: "Tuple".to_string(),
                    vals,
                    site: request
                        .occurrence()
                        .map(|span| match &span.source {
                            Some(source) => {
                                format!("{source}:{}..{}", span.span.0, span.span.1)
                            }
                            None => format!("bytes {}..{}", span.span.0, span.span.1),
                        })
                        .unwrap_or_else(|| "a checked Tuple type".to_string()),
                    output_name,
                    whole_pack_abi: false,
                });
            }
        }
        // Rewrite call sites in every non-template statement, seeding the worklist.
        for stmt in program.iter_mut() {
            if let StmtKind::Def { name, .. } | StmtKind::Struct { name, .. } = &stmt.kind
                && self.specializable.contains_key(name)
            {
                continue; // a template — replaced wholesale below
            }
            self.mono_stmt(stmt, &consts, &mut mono)?;
        }
        // Drain the worklist, generating each requested specialization and scanning
        // its body for further (e.g. recursive) instantiations.
        while let Some(job) = mono.queue.pop_front() {
            self.burn().map_err(|_| {
                ComptimeError::NotComptime(format!(
                    "specialization quota exceeded while instantiating '{}' requested at {}; possible unbounded generic recursion",
                    mangle(&job.orig, &job.vals), job.site
                ))
            })?;
            let mut spec = match &self.specializable[&job.orig].kind {
                StmtKind::Struct { .. } => self.generate_struct_spec(&job.orig, &job.vals)?,
                _ => self.generate_def_spec(
                    self.specializable[&job.orig],
                    &job.orig,
                    job.output_name.clone(),
                    &job.vals,
                )?,
            };
            match &mut spec.kind {
                StmtKind::Def { params, body, .. } => {
                    self.mono_function_body(body, params, &consts, &mut mono)?
                }
                // A struct specialization is fully concrete; walk its members for
                // further template uses (nested instantiations, recursive packs).
                StmtKind::Struct { .. } => self.mono_stmt(&mut spec, &consts, &mut mono)?,
                _ => {}
            }
            // Scan while the parameter still carries its `$pack[T0, ...]`
            // identity: a whole-pack specialization may forward the collector
            // through another generic call. Select the regular Tuple ABI only
            // after all such calls have been rewritten.
            if job.whole_pack_abi {
                select_top_level_whole_pack_abi(&mut spec)?;
            }
            mono.generated.entry(job.orig).or_default().push(spec);
        }
        // Rebuild the program, replacing each template with its specializations at
        // the template's original position. Specializations are emitted in reverse
        // generation order so a callee is defined before its caller (the checker
        // binds names sequentially, without forward references).
        let mut out = Vec::with_capacity(program.len());
        for stmt in program {
            match &stmt.kind {
                StmtKind::Def { name, .. } | StmtKind::Struct { name, .. }
                    if self.specializable.contains_key(name) =>
                {
                    if let Some(mut specs) = mono.generated.remove(name) {
                        specs.reverse();
                        if name == "Tuple" {
                            specs = self.order_tuple_specializations(specs)?;
                        }
                        out.extend(specs);
                    }
                    // No call sites ⇒ dead generic template, dropped.
                }
                _ => out.push(stmt),
            }
        }
        Ok(out)
    }

    /// Order concrete Tuple declarations by the ordinary method-signature and
    /// constructor dependencies introduced for the transforms actually used by
    /// the checked program. The generic worklist's blanket reversal handles a
    /// newly discovered callee, but all checked Tuple result types are seeded up
    /// front, so that incidental queue order is not a dependency relation.
    fn order_tuple_specializations(&self, specs: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
        let baseline = specs
            .iter()
            .map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Ok(name.clone()),
                _ => Err(ComptimeError::NotComptime(
                    "Tuple specialization produced a non-struct declaration".to_string(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let declared = baseline.iter().cloned().collect::<HashSet<_>>();
        let mut dependencies = HashMap::<String, Vec<String>>::new();
        let mut add_dependency = |receiver: &str, dependency: String| {
            if dependency != receiver && declared.contains(&dependency) {
                let entries = dependencies.entry(receiver.to_string()).or_default();
                if !entries.contains(&dependency) {
                    entries.push(dependency);
                }
            }
        };
        for (left, transforms) in &self.tuple_transforms {
            let receiver = tuple_specialization_symbol(left);
            for transform in transforms {
                match transform {
                    TupleTransformRequest::Reverse => {
                        // Generated Tuple identities are predeclared before any
                        // specialization members are checked.  A reverse method's
                        // result annotation and constructor can therefore name the
                        // reverse specialization before its full declaration.  Do
                        // not manufacture a hard ordering edge here: requesting
                        // reverse in both directions is a valid two-node cycle.
                    }
                    TupleTransformRequest::Concat(right) => {
                        add_dependency(&receiver, tuple_specialization_symbol(right));
                        let mut result = left.clone();
                        result.extend(right.iter().cloned());
                        add_dependency(&receiver, tuple_specialization_symbol(&result));
                    }
                }
            }
        }

        fn visit(
            name: &str,
            dependencies: &HashMap<String, Vec<String>>,
            visiting: &mut HashSet<String>,
            emitted: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) -> Result<(), ComptimeError> {
            if emitted.contains(name) {
                return Ok(());
            }
            if !visiting.insert(name.to_string()) {
                return Err(ComptimeError::NotComptime(format!(
                    "checked Tuple transforms create a cyclic declaration dependency involving '{name}'"
                )));
            }
            if let Some(required) = dependencies.get(name) {
                for dependency in required {
                    visit(dependency, dependencies, visiting, emitted, order)?;
                }
            }
            visiting.remove(name);
            emitted.insert(name.to_string());
            order.push(name.to_string());
            Ok(())
        }

        let mut order = Vec::with_capacity(baseline.len());
        let mut visiting = HashSet::new();
        let mut emitted = HashSet::new();
        for name in &baseline {
            visit(name, &dependencies, &mut visiting, &mut emitted, &mut order)?;
        }
        let mut by_name = specs
            .into_iter()
            .map(|statement| {
                let StmtKind::Struct { name, .. } = &statement.kind else {
                    unreachable!("validated Tuple specialization shape")
                };
                (name.clone(), statement)
            })
            .collect::<HashMap<_, _>>();
        Ok(order
            .into_iter()
            .map(|name| {
                by_name
                    .remove(&name)
                    .expect("topological Tuple name came from generated declarations")
            })
            .collect())
    }

    /// Declaration-based specialization core shared by top-level and lexical
    /// nested templates. `display_name` remains source-facing for diagnostics;
    /// `output_name` is the canonical, scope-qualified symbol selected by the
    /// caller.
    fn generate_def_spec(
        &self,
        template: &Stmt,
        display_name: &str,
        output_name: String,
        vals: &[CtValue],
    ) -> Result<Stmt, ComptimeError> {
        let StmtKind::Def {
            decorators,
            type_params,
            params,
            positional_only,
            keyword_only,
            raises,
            raises_type,
            ret,
            body,
            ..
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{display_name}' is not a function"
            )));
        };
        let evaluated_count = type_params
            .iter()
            .filter(|parameter| classify_ct_param(parameter).is_some())
            .count();
        if evaluated_count != vals.len() {
            return Err(ComptimeError::Arity(format!(
                "'{display_name}' expects {} compile-time argument(s), got {}",
                evaluated_count,
                vals.len()
            )));
        }
        // Bind every parameter for comptime resolution; fold value parameters into
        // runtime literals (except where a regular parameter shadows the name); keep
        // type parameters on the specialized signature.
        let mut env = self.top_consts.borrow().clone();
        let mut subs = self.top_consts.borrow().clone();
        for p in params {
            subs.remove(&p.name);
        }
        let mut kept_type_params = Vec::new();
        let mut specialized_params = params.clone();
        let mut type_pack_expansions: HashMap<String, Vec<Type>> = HashMap::new();
        let mut type_pack_values: HashMap<String, Vec<CtValue>> = HashMap::new();
        let mut values = vals.iter();
        for tp in type_params {
            let Some(decl) = classify_ct_param(tp) else {
                // Origin/OriginSet binders and explicit callable-value
                // parameters remain symbolic. Their arguments are retained at
                // each rewritten call and therefore never enter `CtValue`.
                kept_type_params.push(tp.clone());
                continue;
            };
            let v = values
                .next()
                .expect("evaluated parameter count checked above");
            let binding = decl.name().trim_start_matches('*').to_string();
            env.insert(binding.clone(), v.clone());
            match &decl {
                ParamDecl::Value { name, .. } => {
                    subs.insert(name.trim_start_matches('*').to_string(), v.clone());
                }
                ParamDecl::Type { variadic: true, .. } => {
                    let CtValue::Tuple(types) = v else {
                        return Err(ComptimeError::NotComptime(
                            "a type pack specialization requires a tuple of types".to_string(),
                        ));
                    };
                    let source_types = types
                        .iter()
                        .map(|value| match value {
                            CtValue::Type(ty) => source_type_from_ty(ty),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            ComptimeError::NotComptime(
                                "type pack contains a non-type value".to_string(),
                            )
                        })?;
                    type_pack_expansions.insert(binding.clone(), source_types.clone());
                    type_pack_values.insert(binding.clone(), types.clone());
                    for parameter in &mut specialized_params {
                        if matches!(&parameter.ty, Type::Named(name, _) if name.trim_start_matches('*') == decl.name().trim_start_matches('*'))
                        {
                            parameter.ty = Type::Named(
                                "$pack".to_string(),
                                source_types.iter().cloned().map(ParamArg::Type).collect(),
                            );
                        }
                    }
                }
                ParamDecl::Type { .. } => kept_type_params.push(tp.clone()),
            }
        }
        debug_assert!(values.next().is_none());
        // A variadic type-pack specialization also exposes its sequence of
        // element types through the runtime `*args` parameter during compile-time
        // elaboration. This makes `len(args)` and `args[i]` evaluable while a
        // `comptime for` body is being unrolled.
        for pack_param in params {
            let Type::Named(pack_name, _) = &pack_param.ty else {
                continue;
            };
            let Some(types) = type_pack_values.get(pack_name.trim_start_matches('*')) else {
                continue;
            };
            env.insert(pack_param.name.clone(), CtValue::Tuple(types.clone()));
        }
        // Elaborate the body with the parameters bound, so its comptime constructs
        // select/unroll against the concrete arguments.
        let elaborated = self.block(body, &mut env, true)?;
        let mut final_body = materialize_block(elaborated, &subs);
        for parameter in &mut specialized_params {
            if let Some(default) = &mut parameter.default {
                *default = materialize_expression(default, &subs);
            }
        }
        // Retained origin mutability and callable defaults may depend on an
        // earlier scalar value parameter that has just been baked out of the
        // signature. Keep their source declarations self-contained.
        for parameter in &mut kept_type_params {
            if let Some(mutability) = &mut parameter.origin_mutability {
                *mutability = materialize_expression(mutability, &subs);
            }
            if let Some(default) = &mut parameter.default {
                *default = materialize_expression(default, &subs);
            }
        }
        let mut specialized_decorators = decorators.clone();
        for decorator in &mut specialized_decorators {
            for argument in &mut decorator.args {
                *argument = materialize_expression(argument, &subs);
            }
            for argument in &mut decorator.kwargs {
                argument.value = materialize_expression(&argument.value, &subs);
            }
        }
        let specialized_where = match &template.kind {
            StmtKind::Def { where_clause, .. } => where_clause
                .as_ref()
                .map(|predicate| materialize_expression(predicate, &subs)),
            _ => None,
        };
        expand_pack_spreads_in_function_body(
            &mut final_body,
            &specialized_params,
            &type_pack_expansions,
        );
        let mut specialized_ret = ret.clone();
        if let Some(ret) = &mut specialized_ret {
            expand_type_packs(ret, &type_pack_expansions);
        }
        for parameter in &mut specialized_params {
            expand_type_packs(&mut parameter.ty, &type_pack_expansions);
        }
        let mut specialization = mk(
            StmtKind::Def {
                name: output_name.clone(),
                decorators: specialized_decorators,
                type_params: kept_type_params,
                params: specialized_params,
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                captures: match &template.kind {
                    StmtKind::Def { captures, .. } => captures.clone(),
                    _ => None,
                },
                raises: *raises,
                raises_type: raises_type.clone(),
                ret: specialized_ret,
                where_clause: specialized_where,
                body: final_body,
            },
            template.span,
        );
        // Declaration facts are keyed by source identity plus span. Cloned
        // specializations share the template span, so give each concrete
        // function its own synthetic source before checking/HIR lowering.
        let tag = match &template.module {
            Some(module) => format!("{module}${output_name}"),
            None => output_name,
        };
        crate::ast::stamp_source(std::slice::from_mut(&mut specialization), &tag);
        Ok(specialization)
    }

    /// Generate one specialization of variadic-struct template `orig` for the
    /// compile-time arguments `vals`: bind the type pack in the comptime env so
    /// member bodies' `comptime if`/`for` resolve against the concrete element
    /// types, expand pack-typed member annotations (`Tuple[*Ts]`) to the concrete
    /// list, and emit a fully concrete (parameter-free) struct under the mangled
    /// name. Unlike a def specialization, nothing stays symbolic.
    fn generate_struct_spec(&self, orig: &str, vals: &[CtValue]) -> Result<Stmt, ComptimeError> {
        let template = self.specializable[orig];
        let StmtKind::Struct {
            decorators,
            type_params,
            conforms,
            callable_conformance,
            conformance_conditions,
            fields,
            associated,
            methods,
            fieldwise_init,
            ..
        } = &template.kind
        else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{orig}' is not a struct"
            )));
        };
        let decls = classify_ct_params(type_params);
        let (
            [
                ParamDecl::Type {
                    name: pack,
                    variadic: true,
                    ..
                },
            ],
            [CtValue::Tuple(types)],
        ) = (decls.as_slice(), vals)
        else {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{orig}' supports exactly one type-parameter pack and no other compile-time parameters"
            )));
        };
        let binding = pack.trim_start_matches('*').to_string();
        let semantic_types = types
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Some((**ty).clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ComptimeError::NotComptime("type pack contains a non-type value".to_string())
            })?;
        let mut reference_origins = HashMap::new();
        for ty in &semantic_types {
            collect_reference_origin_parameters(ty, &mut reference_origins).ok_or_else(|| {
                ComptimeError::NotComptime(format!(
                    "Tuple element type '{ty}' has an origin that cannot be retained by a nominal specialization"
                ))
            })?;
        }
        // OriginParamId is declaration-order based. Preserve that identity even
        // when an earlier ordinary type/value parameter did not itself occur in
        // this pack by emitting semantic-only padding origins up to the highest
        // retained id.
        let origin_count = reference_origins
            .keys()
            .map(|id| id.0 as usize + 1)
            .max()
            .unwrap_or(0);
        let origin_names = (0..origin_count)
            .map(|index| {
                (
                    crate::origin::OriginParamId(index as u32),
                    format!("__tuple_origin_{index}"),
                )
            })
            .collect::<HashMap<_, _>>();
        let retained_origin_parameters = (0..origin_count)
            .map(|index| {
                let id = crate::origin::OriginParamId(index as u32);
                let mutability = reference_origins
                    .get(&id)
                    .copied()
                    .unwrap_or(crate::origin::Mutability::Param(id));
                TypeParam {
                    name: origin_names[&id].clone(),
                    bounds: vec!["Origin".to_string()],
                    value_type: None,
                    callable_bound: None,
                    origin_mutability: match mutability {
                        crate::origin::Mutability::Immutable => {
                            Some(Expr::new(ExprKind::Bool(false), template.span))
                        }
                        crate::origin::Mutability::Mutable => {
                            Some(Expr::new(ExprKind::Bool(true), template.span))
                        }
                        crate::origin::Mutability::Param(_) => None,
                    },
                    infer_only: true,
                    default: None,
                    constraints: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        let source_types = semantic_types
            .iter()
            .map(|ty| {
                source_type_from_ty_with_origins(ty, &origin_names, &self.materialized_callables)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ComptimeError::NotComptime(
                    "type pack contains a type which cannot be materialized in source".to_string(),
                )
            })?;
        let mut type_pack_expansions = HashMap::new();
        type_pack_expansions.insert(binding.clone(), source_types.clone());
        let mut specialized_associated = associated.clone();
        for member in &mut specialized_associated {
            if matches!(&member.value.kind, ExprKind::Identifier(name) if name == &binding) {
                member.value.kind = ExprKind::TupleLit(
                    source_types
                        .iter()
                        .cloned()
                        .map(|ty| Expr::new(ExprKind::TypeValue(ty), member.value.span))
                        .collect(),
                );
            }
        }
        // Conditional conformances on the source pack become unconditional
        // facts (or disappear) on the concrete implementation struct. Leaving
        // `Ts.values` attached after erasing the pack declaration would make the
        // checker reconstruct a dependency that no longer exists.
        let mut specialized_conforms = Vec::with_capacity(conforms.len());
        for conformance in conforms {
            let Some((_, condition)) = conformance_conditions
                .iter()
                .find(|(candidate, _)| candidate == conformance)
            else {
                specialized_conforms.push(conformance.clone());
                continue;
            };
            let folded = self.fold_pack_conformance_predicate(condition, &binding, types)?;
            match folded.kind {
                ExprKind::Bool(true) => specialized_conforms.push(conformance.clone()),
                ExprKind::Bool(false) => {}
                _ => {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': conditional conformance '{conformance}' did not become concrete after specializing '*{binding}'"
                    )));
                }
            }
        }
        // Elaborate each method body with the pack bound, so comptime constructs
        // select/unroll against the concrete element types.
        let mut elaborated_methods = Vec::with_capacity(methods.len());
        for method in methods {
            let mut method = method.clone();
            let dependent_index_accessor =
                matches!(method.name.as_str(), "__getitem__" | "__getitem_param__")
                    && !method.type_params.is_empty();
            let mut env = self.top_consts.borrow().clone();
            env.insert(binding.clone(), CtValue::Tuple(types.clone()));
            let mut subs = self.top_consts.borrow().clone();
            subs.remove("self");
            for parameter in &method.params {
                subs.remove(&parameter.name);
            }
            // The source pack declaration is erased from a concrete variadic
            // struct. Rewrite every pack-index annotation before checking:
            // concrete indices select their element immediately, while a
            // method/callable binder such as `index` becomes the structural
            // `Self.element_types[index]` projection retained by checked HIR.
            for parameter in &mut method.type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    self.fold_pack_index_annotation(value_type, &binding, &source_types, &env)?;
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    self.fold_pack_index_annotation(callable, &binding, &source_types, &env)?;
                }
            }
            for parameter in &mut method.params {
                self.fold_pack_index_annotation(&mut parameter.ty, &binding, &source_types, &env)?;
            }
            if let Some(error) = &mut method.raises_type {
                self.fold_pack_index_annotation(error, &binding, &source_types, &env)?;
            }
            // Keep `Ts[i]` intact until the dependent-index accessor is
            // unrolled below. At this point `i` is not bound yet; eagerly
            // rewriting it to `Self.element_types[i]` would require every
            // user-defined variadic struct to manufacture Tuple's private
            // `element_types` associated member. Each unrolled accessor has
            // an `env_k` in which `i` is concrete, so the original annotation
            // can be folded directly to the selected element type there.
            if !dependent_index_accessor && let Some(ret) = &mut method.ret {
                self.fold_pack_index_annotation(ret, &binding, &source_types, &env)?;
            }
            // Availability clauses over the struct pack are just as dependent
            // as its conditional conformances. Fold their pack atoms now. A
            // false concrete clause removes the unavailable method; a true one
            // is erased. Any residual method-generic proposition remains for
            // ordinary checker specialization.
            if let Some(condition) = method.where_clause.take() {
                let folded = self.fold_pack_conformance_predicate(&condition, &binding, types)?;
                match &folded.kind {
                    ExprKind::Bool(false) => continue,
                    ExprKind::Bool(true) => {}
                    _ => method.where_clause = Some(folded),
                }
            }
            // A pack-typed runtime parameter (`var *args: *Ts`) becomes the
            // concrete `$pack[T0, ...]`; its element sequence is exposed in the
            // comptime env so `len(args)`/`args[i]`/`comptime for` evaluate
            // while the body is elaborated (mirrors the def-pack path).
            for parameter in &mut method.params {
                if matches!(&parameter.ty, Type::Named(name, _) if name.trim_start_matches('*') == binding)
                {
                    parameter.ty = Type::Named(
                        "$pack".to_string(),
                        source_types.iter().cloned().map(ParamArg::Type).collect(),
                    );
                    env.insert(parameter.name.clone(), CtValue::Tuple(types.clone()));
                }
            }
            // Tuple membership is source-generic, but each comparison is legal
            // only for elements whose concrete type equals the searched type.
            // Once `*Ts` is known, emit one ordinary overload per distinct
            // element type and resolve the `is_same_type` branches now. This
            // leaves no dependent/generic reconstruction for the checker or VM.
            if orig == "Tuple" && method.name == "__contains__" && !source_types.is_empty() {
                let [type_parameter] = method.type_params.as_slice() else {
                    return Err(ComptimeError::NotComptime(
                        "Tuple.__contains__ must have exactly one type parameter".to_string(),
                    ));
                };
                let parameter_name = type_parameter.name.trim_start_matches('*').to_string();
                let mut distinct = Vec::<(Type, CtValue)>::new();
                for ((source_type, semantic_type), value) in source_types
                    .iter()
                    .cloned()
                    .zip(semantic_types.iter())
                    .zip(types.iter())
                {
                    // Specialization erases the method type parameter, so its
                    // declaration bounds must be discharged now. Emitting a
                    // List-valued overload for `T: Equatable`, for example,
                    // would type-check a comparison the source method was never
                    // available to perform.
                    if type_parameter
                        .bounds
                        .iter()
                        .any(|bound| self.conformance.require(semantic_type, bound).is_err())
                    {
                        continue;
                    }
                    if !distinct
                        .iter()
                        .any(|(existing, _)| existing == &source_type)
                    {
                        distinct.push((source_type, value.clone()));
                    }
                }
                for (source_type, value) in distinct {
                    let mut overload = method.clone();
                    overload.type_params.clear();
                    for parameter in &mut overload.params {
                        substitute_source_type_binding(
                            &mut parameter.ty,
                            &parameter_name,
                            &source_type,
                        );
                    }
                    if let Some(ret) = &mut overload.ret {
                        substitute_source_type_binding(ret, &parameter_name, &source_type);
                    }
                    let mut overload_env = env.clone();
                    overload_env.insert(parameter_name.clone(), value.clone());
                    let elaborated = self
                        .block(&overload.body, &mut overload_env, true)
                        .map_err(|error| {
                            ComptimeError::NotComptime(format!(
                                "while specializing {orig}.{}: {error}",
                                overload.name
                            ))
                        })?;
                    overload.body = materialize_block(elaborated, &subs);
                    elaborated_methods.push(overload);
                }
                continue;
            }
            // The dependent-index accessor `def __getitem__[i: Int](self) ->
            // Ts[i]` cannot survive as one checked method (its return type
            // depends on the compile-time index), so it unrolls into one
            // concrete accessor per element — `__getitem__$k` with `i`
            // substituted and the `Ts[i]` annotation folded to that element.
            if dependent_index_accessor {
                let accessor_name = method.name.clone();
                let index_decls = classify_ct_params(&method.type_params);
                let (
                    [
                        ParamDecl::Value {
                            name: index_name,
                            ty: index_ty,
                            ..
                        },
                    ],
                    true,
                    true,
                ) = (
                    index_decls.as_slice(),
                    method.has_self,
                    method.params.is_empty(),
                )
                else {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': a compile-time-parameterized {accessor_name} must take exactly one Int index parameter and only self"
                    )));
                };
                if **index_ty != Ty::Int {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{orig}': the {accessor_name} index parameter must be Int, got {index_ty}"
                    )));
                }
                for k in 0..source_types.len() {
                    let mut unrolled = method.clone();
                    unrolled.name = format!("{accessor_name}${k}");
                    unrolled.type_params = Vec::new();
                    let mut env_k = env.clone();
                    env_k.insert(index_name.clone(), CtValue::Int(k as i64));
                    let mut subs_k = subs.clone();
                    subs_k.insert(index_name.clone(), CtValue::Int(k as i64));
                    let elaborated =
                        self.block(&unrolled.body, &mut env_k, true)
                            .map_err(|error| {
                                ComptimeError::NotComptime(format!(
                                    "while specializing {orig}.{}: {error}",
                                    unrolled.name
                                ))
                            })?;
                    unrolled.body = materialize_block(elaborated, &subs_k);
                    if let Some(ret) = &mut unrolled.ret {
                        self.fold_pack_index_annotation(ret, &binding, &source_types, &env_k)?;
                    }
                    // Indexing private storage whose element is itself a
                    // reference reads through that stored handle. Its public
                    // result therefore carries the element's original origin,
                    // not a newly nested `ref[origin_of(self)] ref[...] T`.
                    if matches!(semantic_types[k], Ty::Ref(_)) {
                        unrolled.ret = Some(source_types[k].clone());
                    }
                    // A reference-returning accessor needs a stable receiver
                    // place. Rvalue Tuple subscripts and destructuring instead
                    // use a value-returning twin when the selected element is
                    // implicitly copyable. Keeping this as an ordinary method
                    // preserves nominal dispatch without manufacturing an
                    // origin for a temporary expression.
                    let value_accessor = if matches!(accessor_name.as_str(), "__getitem__" | "__getitem_param__")
                            && matches!(&unrolled.ret, Some(Type::Ref { .. }))
                            // A callable may be reached through a checked
                            // reference to live Tuple storage, but copying it
                            // out of an rvalue aggregate would turn the
                            // compiler-generated accessor into an escaping
                            // callable return.
                            && !matches!(
                                semantic_types[k],
                                Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                            )
                            && self
                                .conformance
                                .require(&semantic_types[k], "ImplicitlyCopyable")
                                .is_ok()
                    {
                        let mut value_accessor = unrolled.clone();
                        let value_name = if accessor_name == "__getitem_param__" {
                            "__getitem_param_value__"
                        } else {
                            "__getitem_value__"
                        };
                        value_accessor.name = format!("{value_name}${k}");
                        value_accessor.self_convention = None;
                        value_accessor.ret = match value_accessor.ret.take() {
                            Some(Type::Ref { referent, .. }) => Some(*referent),
                            _ => {
                                unreachable!("value-accessor gate requires a reference return")
                            }
                        };
                        Some(value_accessor)
                    } else {
                        None
                    };
                    elaborated_methods.push(unrolled);
                    if let Some(value_accessor) = value_accessor {
                        elaborated_methods.push(value_accessor);
                    }
                }
                continue;
            }
            let elaborated = self.block(&method.body, &mut env, true).map_err(|error| {
                ComptimeError::NotComptime(format!(
                    "while specializing {orig}.{}: {error}",
                    method.name
                ))
            })?;
            method.body = materialize_block(elaborated, &subs);
            elaborated_methods.push(method);
        }
        if orig == "Tuple" {
            self.append_tuple_transform_methods(
                &mut elaborated_methods,
                &semantic_types,
                template.span,
            );
        }
        let mangled = mangle(orig, vals);
        let mut spec = mk(
            StmtKind::Struct {
                name: mangled.clone(),
                decorators: decorators.clone(),
                type_params: retained_origin_parameters,
                conforms: specialized_conforms,
                callable_conformance: callable_conformance.clone(),
                conformance_conditions: Vec::new(),
                fields: fields.clone(),
                associated: specialized_associated,
                methods: elaborated_methods,
                fieldwise_init: *fieldwise_init,
            },
            template.span,
        );
        expand_pack_spreads_in_stmt(&mut spec, &type_pack_expansions);
        // Every specialization reuses the template's spans (correct provenance),
        // so checked facts keyed by source location would collide across
        // specializations of one template. Stamp each subtree with a unique
        // source tag — the mangled name layered on the template's module — and
        // give each unrolled dependent accessor (a clone of one source method)
        // its own tag on top, so their checked facts stay separate too.
        let tag = match &template.module {
            Some(module) => format!("{module}${mangled}"),
            None => mangled,
        };
        crate::ast::stamp_source(std::slice::from_mut(&mut spec), &tag);
        if let StmtKind::Struct { methods, .. } = &mut spec.kind {
            for method in methods {
                if method.name.starts_with("__getitem__$")
                    || method.name.starts_with("__getitem_param__$")
                    || method.name.starts_with("__getitem_value__$")
                    || method.name.starts_with("__getitem_param_value__$")
                {
                    crate::ast::stamp_source(&mut method.body, &format!("{tag}.{}", method.name));
                }
            }
        }
        // The subtree is stamped; disarm `elaborate`'s uniform module re-stamp
        // (it would collapse the per-accessor tags back into one).
        spec.module = None;
        Ok(spec)
    }

    /// Emit closed-world, fully concrete Tuple transforms as ordinary methods.
    /// The discovery checker has already recorded every result Tuple type. No
    /// dependent pack transform survives into checking or MIR, and execution is
    /// normal constructor/method dispatch rather than a VM tuple intrinsic.
    fn append_tuple_transform_methods(
        &self,
        methods: &mut Vec<crate::ast::Method>,
        left: &[Ty],
        span: Span,
    ) {
        let Some((_, transforms)) = self
            .tuple_transforms
            .iter()
            .find(|(elements, _)| elements == left)
        else {
            return;
        };
        for transform in transforms {
            match transform {
                TupleTransformRequest::Reverse => {
                    let reversed = left.iter().rev().cloned().collect::<Vec<_>>();
                    if !self
                        .tuple_universe
                        .iter()
                        .any(|elements| elements == &reversed)
                    {
                        continue;
                    }
                    let target = tuple_specialization_symbol(&reversed);
                    let arguments = (0..left.len())
                        .rev()
                        .map(|index| tuple_storage_element("self", index, true, span))
                        .collect();
                    methods.push(tuple_transform_method(
                        "reverse",
                        Some(ArgConvention::Deinit),
                        Vec::new(),
                        target,
                        arguments,
                        span,
                    ));
                }
                TupleTransformRequest::Concat(right) => {
                    let mut result = left.to_vec();
                    result.extend(right.iter().cloned());
                    if !self
                        .tuple_universe
                        .iter()
                        .any(|elements| elements == &result)
                    {
                        continue;
                    }
                    let right_symbol = tuple_specialization_symbol(right);
                    let target = tuple_specialization_symbol(&result);
                    let mut arguments = (0..left.len())
                        .map(|index| tuple_storage_element("self", index, true, span))
                        .collect::<Vec<_>>();
                    arguments.extend(
                        (0..right.len())
                            .map(|index| tuple_storage_element("other", index, true, span)),
                    );
                    methods.push(tuple_transform_method(
                        "concat",
                        Some(ArgConvention::Deinit),
                        vec![FnParam {
                            name: "other".to_string(),
                            ty: Type::Named(right_symbol, Vec::new()),
                            default: None,
                            kind: ParamKind::Regular,
                            convention: Some(ArgConvention::Deinit),
                            origin: None,
                        }],
                        target,
                        arguments,
                        span,
                    ));
                }
            }
        }
    }

    /// Fold the pack-valued `conforms_to(Ts.values, Trait)` atoms used by
    /// conditional conformances and method availability. Boolean structure is
    /// simplified while unrelated method-generic propositions are retained.
    fn fold_pack_conformance_predicate(
        &self,
        expression: &Expr,
        binding: &str,
        elements: &[CtValue],
    ) -> Result<Expr, ComptimeError> {
        let with_kind = |kind| {
            let mut folded = expression.clone();
            folded.kind = kind;
            folded
        };
        match &expression.kind {
            ExprKind::Call {
                name, args, kwargs, ..
            } if name == "conforms_to" && kwargs.is_empty() && args.len() == 2 => {
                let pack_matches = matches!(
                    &args[0].kind,
                    ExprKind::Member { object, field }
                        if field == "values"
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == binding)
                );
                if !pack_matches {
                    return Ok(expression.clone());
                }
                let ExprKind::Identifier(trait_name) = &args[1].kind else {
                    return Err(ComptimeError::NotComptime(
                        "conforms_to on a type pack requires a trait name".to_string(),
                    ));
                };
                let satisfied = elements.iter().all(|element| match element {
                    CtValue::Type(ty) => self.conformance.require(ty, trait_name).is_ok(),
                    _ => false,
                });
                Ok(with_kind(ExprKind::Bool(satisfied)))
            }
            ExprKind::Prefix(PrefixOp::Not, operand) => {
                let operand = self.fold_pack_conformance_predicate(operand, binding, elements)?;
                match operand.kind {
                    ExprKind::Bool(value) => Ok(with_kind(ExprKind::Bool(!value))),
                    _ => Ok(with_kind(ExprKind::Prefix(
                        PrefixOp::Not,
                        Box::new(operand),
                    ))),
                }
            }
            ExprKind::Infix(op @ (InfixOp::And | InfixOp::Or), left, right) => {
                let left = self.fold_pack_conformance_predicate(left, binding, elements)?;
                let right = self.fold_pack_conformance_predicate(right, binding, elements)?;
                match (op, &left.kind, &right.kind) {
                    (InfixOp::And, ExprKind::Bool(false), _)
                    | (InfixOp::And, _, ExprKind::Bool(false)) => {
                        Ok(with_kind(ExprKind::Bool(false)))
                    }
                    (InfixOp::And, ExprKind::Bool(true), _) => Ok(right),
                    (InfixOp::And, _, ExprKind::Bool(true)) => Ok(left),
                    (InfixOp::Or, ExprKind::Bool(true), _)
                    | (InfixOp::Or, _, ExprKind::Bool(true)) => Ok(with_kind(ExprKind::Bool(true))),
                    (InfixOp::Or, ExprKind::Bool(false), _) => Ok(right),
                    (InfixOp::Or, _, ExprKind::Bool(false)) => Ok(left),
                    _ => Ok(with_kind(ExprKind::Infix(
                        *op,
                        Box::new(left),
                        Box::new(right),
                    ))),
                }
            }
            _ => Ok(expression.clone()),
        }
    }

    /// Fold a dependent pack-element annotation `Ts[expr]` (with `expr`
    /// evaluable in `env`, e.g. the unrolled accessor's index) to the concrete
    /// element type it selects.
    fn fold_pack_index_annotation(
        &self,
        ty: &mut Type,
        binding: &str,
        elements: &[Type],
        env: &HashMap<String, CtValue>,
    ) -> Result<(), ComptimeError> {
        match ty {
            Type::Named(name, arguments) => {
                if name.trim_start_matches('*') == binding
                    && let [ParamArg::Value(index)] = arguments.as_slice()
                {
                    if let Ok(index_value) = self.eval(index, env) {
                        let index_value = index_value.as_int("pack index")?;
                        let element = elements.get(index_value as usize).ok_or_else(|| {
                            ComptimeError::BadArithmetic(format!(
                                "pack index {index_value} out of range for '{binding}' of length {}",
                                elements.len()
                            ))
                        })?;
                        *ty = element.clone();
                    } else {
                        *ty = Type::IndexedProjection {
                            base: Box::new(Type::Assoc {
                                base: Box::new(Type::SelfType),
                                name: "element_types".to_string(),
                            }),
                            index: Box::new(materialize_expression(index, env)),
                        };
                    }
                    return Ok(());
                }
                for argument in arguments {
                    if let ParamArg::Type(inner) = argument {
                        self.fold_pack_index_annotation(inner, binding, elements, env)?;
                    }
                }
                Ok(())
            }
            Type::Assoc { base, .. } => {
                self.fold_pack_index_annotation(base, binding, elements, env)
            }
            Type::IndexedProjection { base, index } => {
                self.fold_pack_index_annotation(base, binding, elements, env)?;
                **index = materialize_expression(index, env);
                Ok(())
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
                        self.fold_pack_index_annotation(value_type, binding, elements, env)?;
                    }
                    if let Some(callable) = &mut parameter.callable_bound {
                        self.fold_pack_index_annotation(callable, binding, elements, env)?;
                    }
                }
                for param in params {
                    self.fold_pack_index_annotation(&mut param.ty, binding, elements, env)?;
                }
                self.fold_pack_index_annotation(ret, binding, elements, env)?;
                if let Some(error) = raises_type {
                    self.fold_pack_index_annotation(error, binding, elements, env)?;
                }
                Ok(())
            }
            Type::Ref { referent, .. } => {
                self.fold_pack_index_annotation(referent, binding, elements, env)
            }
            Type::Int
            | Type::UInt
            | Type::Bool
            | Type::String
            | Type::Float64
            | Type::None
            | Type::SelfParam(_)
            | Type::SelfType
            | Type::MaterializedCallable(_) => Ok(()),
        }
    }

    fn mono_block(
        &self,
        stmts: &mut [Stmt],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        mono.push_value_scope();
        let result = self.mono_block_contents(stmts, consts, mono);
        mono.pop_value_scope();
        result
    }

    fn mono_function_body(
        &self,
        stmts: &mut [Stmt],
        parameters: &[FnParam],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        mono.push_function_scope();
        for parameter in parameters {
            mono.bind_parameter(parameter);
        }
        let result = self.mono_block_contents(stmts, consts, mono);
        mono.pop_function_scope();
        result
    }

    fn mono_block_contents(
        &self,
        stmts: &mut [Stmt],
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        for s in stmts {
            // Declarations bind before their body is visited, preserving
            // recursion while shadowing an outer top-level template.
            if let StmtKind::Def { name, .. }
            | StmtKind::Struct { name, .. }
            | StmtKind::Trait { name, .. } = &s.kind
            {
                mono.bind_value(name, false);
            }
            self.mono_stmt(s, consts, mono)?;
            match &s.kind {
                StmtKind::VarDecl { name, .. }
                | StmtKind::RefDecl { name, .. }
                | StmtKind::Comptime { name, .. } => mono.bind_value(name, false),
                StmtKind::Assign { name, .. } => mono.bind_named_value(name),
                StmtKind::Import { path, alias } => {
                    if let Some(name) = alias.as_ref().or_else(|| path.first()) {
                        mono.bind_value(name, false);
                    }
                }
                StmtKind::FromImport {
                    names: crate::ast::ImportNames::Names(names),
                    ..
                } => {
                    for import in names {
                        mono.bind_value(import.alias.as_deref().unwrap_or(&import.name), false);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn mono_stmt(
        &self,
        s: &mut Stmt,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        // Monomorphization substitutes one concrete parameter environment and
        // rewrites nested calls to their specialized symbols.
        match &mut s.kind {
            StmtKind::VarDecl { ty, value, .. } => {
                if let Some(ty) = ty {
                    self.mono_type(ty, consts, mono)?;
                }
                self.mono_expr(value, consts, mono)
            }
            StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. }
            | StmtKind::Comptime { value, .. }
            | StmtKind::Raise(value)
            | StmtKind::Return(Some(value)) => self.mono_expr(value, consts, mono),
            StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Import { .. }
            | StmtKind::FromImport { .. }
            | StmtKind::Trait { .. } => Ok(()),
            StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
                self.mono_expr(place, consts, mono)?;
                self.mono_expr(value, consts, mono)
            }
            StmtKind::Unpack { targets, value } => {
                for t in targets.iter_mut() {
                    self.mono_expr(t, consts, mono)?;
                }
                self.mono_expr(value, consts, mono)
            }
            StmtKind::Expr(e) => self.mono_expr(e, consts, mono),
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (c, b) in branches.iter_mut() {
                    self.mono_expr(c, consts, mono)?;
                    self.mono_block(b, consts, mono)?;
                }
                if let Some(b) = orelse {
                    self.mono_block(b, consts, mono)?;
                }
                Ok(())
            }
            StmtKind::While { cond, body, .. } => {
                self.mono_expr(cond, consts, mono)?;
                self.mono_block(body, consts, mono)
            }
            StmtKind::For {
                var, iter, body, ..
            }
            | StmtKind::ComptimeFor { var, iter, body } => {
                self.mono_expr(iter, consts, mono)?;
                mono.push_value_scope();
                mono.bind_value(var, false);
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_value_scope();
                result
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.mono_block(body, consts, mono)?;
                if let Some((name, b)) = except {
                    mono.push_value_scope();
                    if let Some(name) = name {
                        mono.bind_value(name, false);
                    }
                    let result = self.mono_block_contents(b, consts, mono);
                    mono.pop_value_scope();
                    result?;
                }
                if let Some(b) = orelse {
                    self.mono_block(b, consts, mono)?;
                }
                if let Some(b) = finalbody {
                    self.mono_block(b, consts, mono)?;
                }
                Ok(())
            }
            StmtKind::With { items, body } => {
                for WithItem { context, .. } in items.iter_mut() {
                    self.mono_expr(context, consts, mono)?;
                }
                mono.push_value_scope();
                for item in items {
                    if let Some(name) = &item.var {
                        mono.bind_value(name, false);
                    }
                }
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_value_scope();
                result
            }
            StmtKind::Def {
                params,
                raises_type,
                ret,
                body,
                ..
            } => {
                for parameter in params.iter_mut() {
                    self.mono_type(&mut parameter.ty, consts, mono)?;
                    if let Some(default) = &mut parameter.default {
                        self.mono_expr(default, consts, mono)?;
                    }
                }
                if let Some(error) = raises_type {
                    self.mono_type(error, consts, mono)?;
                }
                if let Some(ret) = ret {
                    self.mono_type(ret, consts, mono)?;
                }
                mono.push_function_scope();
                for parameter in params {
                    mono.bind_parameter(parameter);
                }
                let result = self.mono_block_contents(body, consts, mono);
                mono.pop_function_scope();
                result
            }
            StmtKind::Struct {
                type_params,
                fields,
                associated,
                methods,
                ..
            } => {
                let mut struct_consts = consts.clone();
                for (index, parameter) in type_params.iter().enumerate() {
                    if parameter.bounds.as_slice() != ["Origin"] {
                        continue;
                    }
                    let id = crate::origin::OriginParamId(index as u32);
                    let mutability = match parameter.origin_mutability.as_ref().map(|e| &e.kind) {
                        Some(ExprKind::Bool(true)) => crate::origin::Mutability::Mutable,
                        Some(ExprKind::Bool(false)) => crate::origin::Mutability::Immutable,
                        _ => crate::origin::Mutability::Param(id),
                    };
                    struct_consts
                        .insert(parameter.name.clone(), ct_origin_marker(index, mutability));
                }
                for field in fields.iter_mut() {
                    self.mono_type(&mut field.ty, &struct_consts, mono)?;
                }
                // Associated facts may themselves be type-valued.  A variadic
                // struct mentioned only here still needs a concrete request
                // before its template is removed (for example an Iterable's
                // associated iterator family).
                for member in associated.iter_mut() {
                    self.mono_expr(&mut member.value, &struct_consts, mono)?;
                }
                for m in methods.iter_mut() {
                    for parameter in m.params.iter_mut() {
                        self.mono_type(&mut parameter.ty, &struct_consts, mono)?;
                        if let Some(default) = &mut parameter.default {
                            self.mono_expr(default, &struct_consts, mono)?;
                        }
                    }
                    if let Some(error) = &mut m.raises_type {
                        self.mono_type(error, &struct_consts, mono)?;
                    }
                    if let Some(ret) = &mut m.ret {
                        self.mono_type(ret, &struct_consts, mono)?;
                    }
                    mono.push_function_scope();
                    if m.has_self {
                        mono.bind_value("self", false);
                    }
                    for parameter in &m.params {
                        mono.bind_parameter(parameter);
                    }
                    let result = self.mono_block_contents(&mut m.body, &struct_consts, mono);
                    mono.pop_function_scope();
                    result?;
                }
                Ok(())
            }
        }
    }

    /// Rewrite variadic-struct template names inside a type annotation to their
    /// specialized (mangled) names, enqueueing the needed instantiations.
    fn mono_type(
        &self,
        ty: &mut Type,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        match ty {
            Type::Named(name, arguments) => {
                for argument in arguments.iter_mut() {
                    self.mono_param_arg(argument, consts, mono)?;
                }
                if self.specializable.contains_key(name.as_str())
                    && matches!(
                        self.specializable[name.as_str()].kind,
                        StmtKind::Struct { .. }
                    )
                {
                    let Some(vals) =
                        self.resolve_struct_spec_args_if_ready(name, arguments, consts)?
                    else {
                        // Public Tuple applications in an ordinary generic
                        // declaration remain symbolic until the checker has
                        // substituted the declaration's type parameters at a
                        // concrete use.  The discovery pass then requests the
                        // resulting closed nominal specialization.
                        return Ok(());
                    };
                    let mangled = mangle(name, &vals);
                    if mono.done.insert(mangled.clone()) {
                        mono.queue.push_back(Job {
                            orig: name.clone(),
                            vals,
                            site: "a type annotation".to_string(),
                            output_name: mangled.clone(),
                            whole_pack_abi: false,
                        });
                    }
                    *name = mangled;
                    arguments.clear();
                }
                Ok(())
            }
            Type::Assoc { base, .. } => self.mono_type(base, consts, mono),
            Type::IndexedProjection { base, index } => {
                self.mono_type(base, consts, mono)?;
                self.mono_expr(index, consts, mono)
            }
            Type::Func {
                type_params,
                params,
                ret,
                capturing,
                raises_type,
                ..
            } => {
                for parameter in type_params {
                    if let Some(value_type) = &mut parameter.value_type {
                        self.mono_type(value_type, consts, mono)?;
                    }
                    if let Some(callable) = &mut parameter.callable_bound {
                        self.mono_type(callable, consts, mono)?;
                    }
                    if let Some(mutability) = &mut parameter.origin_mutability {
                        self.mono_expr(mutability, consts, mono)?;
                    }
                    if let Some(default) = &mut parameter.default {
                        self.mono_expr(default, consts, mono)?;
                    }
                    for constraint in &mut parameter.constraints {
                        self.mono_expr(constraint, consts, mono)?;
                    }
                }
                for param in params {
                    self.mono_type(&mut param.ty, consts, mono)?;
                }
                self.mono_type(ret, consts, mono)?;
                for origin in capturing.iter_mut().flatten() {
                    self.mono_expr(origin, consts, mono)?;
                }
                if let Some(error) = raises_type {
                    self.mono_type(error, consts, mono)?;
                }
                Ok(())
            }
            Type::Ref { referent, .. } => self.mono_type(referent, consts, mono),
            Type::Int
            | Type::UInt
            | Type::Bool
            | Type::String
            | Type::Float64
            | Type::None
            | Type::SelfParam(_)
            | Type::SelfType
            | Type::MaterializedCallable(_) => Ok(()),
        }
    }

    fn mono_param_arg(
        &self,
        argument: &mut ParamArg,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        match argument {
            ParamArg::Type(ty) => self.mono_type(ty, consts, mono),
            ParamArg::Named { value, .. } => self.mono_param_arg(value, consts, mono),
            ParamArg::Value(value) => self.mono_expr(value, consts, mono),
        }
    }

    fn mono_expr(
        &self,
        e: &mut Expr,
        consts: &HashMap<String, CtValue>,
        mono: &mut Mono,
    ) -> Result<(), ComptimeError> {
        let source_span = e.source_span();
        let request_site = match &source_span.source {
            Some(source) => format!("{source}:{}..{}", source_span.span.0, source_span.span.1),
            None => format!("bytes {}..{}", source_span.span.0, source_span.span.1),
        };
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::TString { .. } => Ok(()),
            ExprKind::Identifier(name) => {
                // The template is dropped after monomorphization, so a bare
                // (argument-less) use of a variadic struct can never resolve.
                if mono.resolves_top_template(name) && self.struct_template(name) {
                    return Err(ComptimeError::NotComptime(format!(
                        "variadic struct '{name}' requires explicit compile-time type arguments, e.g. `{name}[Int, Bool](...)`"
                    )));
                }
                Ok(())
            }
            ExprKind::TypeApply { name, args } => {
                if mono.resolves_top_template(name) && self.struct_template(name) {
                    let Some(vals) = self.resolve_struct_spec_args_if_ready(name, args, consts)?
                    else {
                        return Ok(());
                    };
                    let mangled = mangle(name, &vals);
                    if mono.done.insert(mangled.clone()) {
                        mono.queue.push_back(Job {
                            orig: name.clone(),
                            vals,
                            site: request_site,
                            output_name: mangled.clone(),
                            whole_pack_abi: false,
                        });
                    }
                    *name = mangled;
                    args.clear();
                }
                Ok(())
            }
            ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) | ExprKind::Spread(inner) => {
                self.mono_expr(inner, consts, mono)
            }
            ExprKind::Infix(_, l, r) => {
                self.mono_expr(l, consts, mono)?;
                self.mono_expr(r, consts, mono)
            }
            ExprKind::Compare { first, rest } => {
                self.mono_expr(first, consts, mono)?;
                for (_, r) in rest.iter_mut() {
                    self.mono_expr(r, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                for a in args.iter_mut() {
                    self.mono_expr(a, consts, mono)?;
                }
                for k in kwargs.iter_mut() {
                    self.mono_expr(&mut k.value, consts, mono)?;
                }
                // A bare public `Tuple(...)` has no source type arguments from
                // which pre-check elaboration could soundly choose `*Ts`.  Only
                // rewrite an occurrence the checker explicitly identified; an
                // unhinted occurrence deliberately survives for the discovery
                // check. Other variadic struct templates retain their existing
                // explicit-argument requirement.
                if name == "Tuple"
                    && param_args.is_empty()
                    && mono.resolves_top_template(name)
                    && self.struct_template(name)
                {
                    if let Some(target) = mono
                        .tuple_call_targets
                        .get(&source_span.clone().without_syntax())
                    {
                        *name = target.clone();
                    }
                    return Ok(());
                }
                if mono.resolves_top_template(name) && self.specializable.contains_key(name) {
                    let (vals, kept_type_args, whole_pack_abi) = if self.struct_template(name) {
                        // A struct specialization is fully concrete: every
                        // compile-time argument is baked into the mangled name.
                        let Some(values) =
                            self.resolve_struct_spec_args_if_ready(name, param_args, consts)?
                        else {
                            return Ok(());
                        };
                        (values, Vec::new(), false)
                    } else {
                        let template = self.specializable[name.as_str()];
                        let whole_pack_abi = top_level_whole_pack_forwarding_call(template, args)?;
                        let forwarded =
                            top_level_forwarded_pack_types(template, name, args, kwargs, mono)?;
                        let (values, kept) = self.resolve_spec_args_for(
                            template,
                            name,
                            SpecRequest {
                                param_args,
                                call_args: args,
                                kwargs,
                                consts,
                                request_site: &request_site,
                                forwarded_pack_types: forwarded.as_deref(),
                            },
                        )?;
                        (values, kept, whole_pack_abi)
                    };
                    let original = name.clone();
                    let mut output_name = mangle(name, &vals);
                    if whole_pack_abi {
                        output_name.push_str("$whole_pack");
                    }
                    if mono.done.insert(output_name.clone()) {
                        mono.queue.push_back(Job {
                            orig: original,
                            vals,
                            site: request_site,
                            output_name: output_name.clone(),
                            whole_pack_abi,
                        });
                    }
                    *name = output_name;
                    if whole_pack_abi {
                        *args = unwrap_runtime_pack_arguments(std::mem::take(args));
                    }
                    // Value arguments are baked into the specialization; type
                    // arguments stay on the (still type-generic) specialized def.
                    *param_args = kept_type_args;
                }
                Ok(())
            }
            ExprKind::Member { object, .. } => self.mono_expr(object, consts, mono),
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.mono_expr(object, consts, mono)?;
                for a in args.iter_mut() {
                    self.mono_expr(a, consts, mono)?;
                }
                for k in kwargs.iter_mut() {
                    self.mono_expr(&mut k.value, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Index { object, index } => {
                self.mono_expr(object, consts, mono)?;
                self.mono_expr(index, consts, mono)
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.mono_expr(object, consts, mono)?;
                for b in [lower, upper, step].into_iter().flatten() {
                    self.mono_expr(b, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::MultiIndex { object, args } => {
                self.mono_expr(object, consts, mono)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value) => {
                            self.mono_expr(value, consts, mono)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.mono_expr(value, consts, mono)?;
                            }
                        }
                    }
                }
                Ok(())
            }
            ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => {
                for el in elems.iter_mut() {
                    self.mono_expr(el, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.mono_expr(key, consts, mono)?;
                    if let Some(value) = value {
                        self.mono_expr(value, consts, mono)?;
                    }
                }
                Ok(())
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                mono.push_value_scope();
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.mono_expr(iter, consts, mono)?;
                            mono.bind_value(var, false);
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.mono_expr(condition, consts, mono)?
                        }
                    }
                }
                if let Some(key) = key {
                    self.mono_expr(key, consts, mono)?;
                }
                let result = self.mono_expr(value, consts, mono);
                mono.pop_value_scope();
                result
            }
            ExprKind::Named { name, value } => {
                self.mono_expr(value, consts, mono)?;
                mono.bind_named_value(name);
                Ok(())
            }
            ExprKind::TypeValue(_) => Ok(()),
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.mono_expr(callee, consts, mono)?;
                for argument in param_args {
                    self.mono_param_arg(argument, consts, mono)?;
                }
                for argument in args {
                    self.mono_expr(argument, consts, mono)?;
                }
                for argument in kwargs {
                    self.mono_expr(&mut argument.value, consts, mono)?;
                }
                Ok(())
            }
            ExprKind::Uninitialized => Ok(()),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.mono_expr(cond, consts, mono)?;
                self.mono_expr(then_branch, consts, mono)?;
                self.mono_expr(else_branch, consts, mono)
            }
        }
    }

    /// Whether `name` is a specializable variadic-struct template.
    fn struct_template(&self, name: &str) -> bool {
        self.specializable
            .get(name)
            .is_some_and(|template| matches!(template.kind, StmtKind::Struct { .. }))
    }

    /// Resolve a variadic-struct instantiation's `[...]` arguments into the
    /// specialization key: every argument is a type, collected into the pack
    /// tuple. Instantiation requires explicit arguments (the elaborator does
    /// not infer types), and a template supports exactly one trailing pack.
    fn resolve_struct_spec_args(
        &self,
        name: &str,
        param_args: &[ParamArg],
        consts: &HashMap<String, CtValue>,
    ) -> Result<Vec<CtValue>, ComptimeError> {
        let StmtKind::Struct { type_params, .. } = &self.specializable[name].kind else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{name}' is not a struct"
            )));
        };
        let decls = classify_ct_params(type_params);
        let [ParamDecl::Type { variadic: true, .. }] = decls.as_slice() else {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{name}' supports exactly one type-parameter pack and no other compile-time parameters"
            )));
        };
        if param_args.is_empty() {
            return Err(ComptimeError::NotComptime(format!(
                "variadic struct '{name}' requires explicit compile-time type arguments, e.g. `{name}[Int, Bool](...)`"
            )));
        }
        let types = param_args
            .iter()
            .map(|argument| {
                self.param_arg_type(argument, consts)
                    .map(|ty| CtValue::Type(Box::new(ty)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(vec![CtValue::Tuple(types)])
    }

    /// Resolve a variadic-struct application when it is ready for concrete
    /// monomorphization.  Public `Tuple[T, ...]` may appear in the signature or
    /// body of an ordinary generic declaration: pre-check elaboration has no
    /// binding for `T`, and manufacturing a `Tuple$T` implementation would be
    /// unsound.  Leave only that compiler-known public template canonical so
    /// the checker can retain the symbolic type and the later discovery pass can
    /// request its closed call-site instantiations.  User variadic structs keep
    /// their existing eager, explicit-specialization diagnostics.
    fn resolve_struct_spec_args_if_ready(
        &self,
        name: &str,
        param_args: &[ParamArg],
        consts: &HashMap<String, CtValue>,
    ) -> Result<Option<Vec<CtValue>>, ComptimeError> {
        match self.resolve_struct_spec_args(name, param_args, consts) {
            Ok(values) => Ok(Some(values)),
            Err(_) if name == "Tuple" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Resolve arguments for one concrete declaration. `forwarded_pack_types`
    /// supplies the element sequence when a specialized runtime pack is being
    /// forwarded into another heterogeneous collector; ordinary calls infer the
    /// sequence from their source expressions as before.
    fn resolve_spec_args_for(
        &self,
        template: &Stmt,
        display_name: &str,
        request: SpecRequest<'_>,
    ) -> Result<(Vec<CtValue>, Vec<ParamArg>), ComptimeError> {
        let SpecRequest {
            param_args,
            call_args,
            kwargs,
            consts,
            request_site,
            forwarded_pack_types,
        } = request;
        let StmtKind::Def { type_params, .. } = &template.kind else {
            return Err(ComptimeError::NotComptime(format!(
                "specialization registry entry '{display_name}' is not a function"
            )));
        };

        // Bind the source argument list before classifying anything away.  In
        // particular, an infer-only Origin consumes no positional slot, and a
        // pack consumes only the overflow left after required suffix binders.
        // This is the source-layout invariant used again by
        // `generate_def_spec`.
        let mut bound: Vec<Vec<&ParamArg>> = vec![Vec::new(); type_params.len()];
        let mut positional = Vec::new();
        for argument in param_args {
            if let ParamArg::Named { name, .. } = argument {
                let Some(index) = type_params
                    .iter()
                    .position(|parameter| parameter.name.trim_start_matches('*') == name)
                else {
                    return Err(ComptimeError::Arity(format!(
                        "generic '{display_name}' has no compile-time parameter named '{name}'"
                    )));
                };
                if !bound[index].is_empty() {
                    return Err(ComptimeError::Arity(format!(
                        "generic '{display_name}' received compile-time parameter '{name}' more than once"
                    )));
                }
                bound[index].push(argument);
            } else {
                positional.push(argument);
            }
        }

        let required_suffix = |start: usize, bound: &[Vec<&ParamArg>]| {
            type_params[start..]
                .iter()
                .zip(&bound[start..])
                .filter(|(parameter, arguments)| {
                    arguments.is_empty()
                        && !parameter.infer_only
                        && !parameter.name.starts_with('*')
                        && parameter.default.is_none()
                })
                .count()
        };
        let mut next_positional = 0;
        for index in 0..type_params.len() {
            let parameter = &type_params[index];
            if !bound[index].is_empty() || parameter.infer_only {
                continue;
            }
            let remaining = positional.len() - next_positional;
            let suffix = required_suffix(index + 1, &bound);
            if parameter.name.starts_with('*') {
                let take = remaining.saturating_sub(suffix);
                bound[index].extend_from_slice(
                    &positional[next_positional..next_positional.saturating_add(take)],
                );
                next_positional += take;
            } else if remaining > suffix {
                bound[index].push(positional[next_positional]);
                next_positional += 1;
            }
        }
        if next_positional != positional.len() {
            return Err(ComptimeError::Arity(format!(
                "generic '{display_name}' received {} unmatched compile-time argument(s)",
                positional.len() - next_positional
            )));
        }

        let mut vals = Vec::new();
        let mut kept_type_args = Vec::new();
        let mut environment = consts.clone();
        for (parameter, arguments) in type_params.iter().zip(bound) {
            if retained_specialization_param(parameter) {
                if arguments.is_empty() && !parameter.infer_only && parameter.default.is_none() {
                    return Err(ComptimeError::Arity(format!(
                        "generic '{display_name}' requires compile-time parameter '{}'",
                        parameter.name.trim_start_matches('*')
                    )));
                }
                kept_type_args.extend(arguments.into_iter().cloned());
                continue;
            }

            let decl = classify_ct_param(parameter)
                .expect("non-retained source parameter must have a comptime classification");
            let binding = decl.name().trim_start_matches('*').to_string();
            if parameter.name.starts_with('*') {
                let value = match &decl {
                    ParamDecl::Value { name: pack, ty, .. } => {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            let value = self.resolve_ct_arg(&decl, argument, &environment)?;
                            if !ct_value_has_type(&value, ty) {
                                return Err(ComptimeError::NotComptime(format!(
                                    "value pack '{}' expects {ty}, got {value}",
                                    pack.trim_start_matches('*')
                                )));
                            }
                            values.push(value);
                        }
                        CtValue::Tuple(values)
                    }
                    ParamDecl::Type {
                        name: pack, bounds, ..
                    } => {
                        let types = if arguments.is_empty() {
                            match forwarded_pack_types {
                                Some(types) => types.to_vec(),
                                None => runtime_pack_call_arguments(
                                    template,
                                    display_name,
                                    call_args,
                                    kwargs,
                                )?
                                .into_iter()
                                .map(infer_pack_argument_type)
                                .collect::<Result<Vec<_>, _>>()?,
                            }
                        } else {
                            arguments
                                .into_iter()
                                .map(|argument| self.param_arg_type(argument, &environment))
                                .collect::<Result<Vec<_>, _>>()?
                        };
                        for (index, ty) in types.iter().enumerate() {
                            for trait_name in bounds {
                                if let Err(failure) = self.conformance.require(ty, trait_name) {
                                    return Err(ComptimeError::PackBound(Box::new(
                                        PackBoundError {
                                            function: display_name.to_string(),
                                            pack: pack.trim_start_matches('*').to_string(),
                                            index,
                                            ty: ty.to_string(),
                                            trait_name: trait_name.clone(),
                                            site: request_site.to_string(),
                                            reason: failure.reason,
                                        },
                                    )));
                                }
                            }
                        }
                        CtValue::Tuple(
                            types
                                .into_iter()
                                .map(|ty| CtValue::Type(Box::new(ty)))
                                .collect(),
                        )
                    }
                };
                environment.insert(binding, value.clone());
                vals.push(value);
                continue;
            }

            let value = if let Some(argument) = arguments.first() {
                self.resolve_ct_arg(&decl, argument, &environment)?
            } else {
                match &decl {
                    ParamDecl::Value {
                        default: Some(default),
                        ty,
                        ..
                    } => {
                        let evaluated = default.evaluate(&environment).ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "cannot evaluate default for parameter '{}'",
                                decl.name()
                            ))
                        })?;
                        materialize_ct_value(evaluated.clone(), ty).ok_or_else(|| {
                            ComptimeError::NotComptime(format!(
                                "default for parameter '{}' expects {ty}, got {evaluated}",
                                decl.name()
                            ))
                        })?
                    }
                    ParamDecl::Type {
                        default: Some(default),
                        ..
                    } => CtValue::Type(default.clone()),
                    _ => {
                        return Err(ComptimeError::Arity(format!(
                            "generic '{display_name}' requires compile-time parameter '{}'",
                            decl.name().trim_start_matches('*')
                        )));
                    }
                }
            };
            if matches!(decl, ParamDecl::Type { .. }) {
                kept_type_args.extend(arguments.into_iter().cloned());
            }
            environment.insert(binding, value.clone());
            vals.push(value);
        }
        Ok((vals, kept_type_args))
    }
}

fn materialize_ct_value(value: CtValue, ty: &Ty) -> Option<CtValue> {
    value.materialize_as(ty)
}

fn ct_value_has_type(value: &CtValue, ty: &Ty) -> bool {
    materialize_ct_value(value.clone(), ty).is_some()
}

fn infer_pack_argument_type(expr: &Expr) -> Result<Ty, ComptimeError> {
    match &expr.kind {
        ExprKind::Int(_) => Ok(Ty::Int),
        ExprKind::Float(_) => Ok(Ty::Float64),
        ExprKind::Bool(_) => Ok(Ty::Bool),
        ExprKind::Str(_) => Ok(Ty::String),
        ExprKind::None => Ok(Ty::None),
        ExprKind::Call { name, .. } => Ok(match name.as_str() {
            "Int" => Ty::Int,
            "UInt" => Ty::UInt,
            "Float64" => Ty::Float64,
            "Bool" => Ty::Bool,
            "String" => Ty::String,
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
            TyArg::Val(_) => Some(()),
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

fn source_type_from_ty_with_origins(
    ty: &Ty,
    origin_names: &HashMap<crate::origin::OriginParamId, String>,
    materialized_callables: &[(Ty, String)],
) -> Option<Type> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => Type::Int,
        Ty::UInt => Type::UInt,
        Ty::Bool => Type::Bool,
        Ty::String => Type::String,
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

fn source_type_from_ty(ty: &Ty) -> Option<Type> {
    source_type_from_ty_with_origins(ty, &HashMap::new(), &[])
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
        | Type::String
        | Type::Float64
        | Type::None
        | Type::SelfParam(_)
        | Type::SelfType
        | Type::MaterializedCallable(_) => {}
    }
}

fn substitute_source_param_arg_binding(argument: &mut ParamArg, binding: &str, replacement: &Type) {
    match argument {
        ParamArg::Type(ty) => substitute_source_type_binding(ty, binding, replacement),
        ParamArg::Named { value, .. } => {
            substitute_source_param_arg_binding(value, binding, replacement);
        }
        ParamArg::Value(_) => {}
    }
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

/// A pending specialization request: template `orig`, specialized for `vals`.
struct Job {
    orig: String,
    vals: Vec<CtValue>,
    site: String,
    output_name: String,
    whole_pack_abi: bool,
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

fn encode_specialization_value(value: &CtValue, out: &mut String) {
    match value {
        CtValue::Int(value) => out.push_str(&format!("i{value};")),
        CtValue::UInt(value) => out.push_str(&format!("u{value};")),
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

fn mk(kind: StmtKind, span: Span) -> Stmt {
    Stmt {
        kind,
        span,
        module: None,
        syntax_id: crate::token::SyntaxId::fresh(),
    }
}

fn lit_result(val: &CtValue, span: Span) -> Result<Expr, ComptimeError> {
    val.materialize(span).ok_or_else(|| {
        ComptimeError::NotComptime(
            "type-valued or symbolic comptime values cannot materialize at runtime".to_string(),
        )
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
        CtValue::Type(_) | CtValue::Reflected(_) | CtValue::Param(_) => {
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

fn vm_ctfe_safe_builtin(name: &str) -> bool {
    matches!(
        name,
        "range" | "abs" | "min" | "max" | "round" | "Int" | "UInt" | "Float64"
    )
}

fn scalar_type_name(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::Int),
        "UInt" => Some(Ty::UInt),
        "Bool" => Some(Ty::Bool),
        "String" => Some(Ty::String),
        "Float64" => Some(Ty::Float64),
        "None" => Some(Ty::None),
        _ => None,
    }
}

mod nested;
mod rewrite;
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
    use super::{
        TupleSpecializationRequest, elaborate_with_tuple_requests, tuple_specialization_symbol,
    };
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
        let elements = vec![Ty::Int, Ty::String];
        let expected = tuple_specialization_symbol(&elements);

        let elaborated = elaborate_with_tuple_requests(
            parsed,
            &[TupleSpecializationRequest::bare_call(elements, occurrence)],
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

        let elaborated = elaborate_with_tuple_requests(
            parsed,
            &[TupleSpecializationRequest::declaration(elements)],
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
        let outer_elements = vec![inner, Ty::String];
        let inner_symbol = tuple_specialization_symbol(&[Ty::Int]);
        let outer_symbol = tuple_specialization_symbol(&outer_elements);

        let elaborated = elaborate_with_tuple_requests(
            parsed,
            &[TupleSpecializationRequest::declaration(outer_elements)],
        )
        .expect("materialize nested Tuple specializations");
        let names = struct_names(&elaborated);

        assert!(names.contains(&inner_symbol.as_str()), "{names:?}");
        assert!(names.contains(&outer_symbol.as_str()), "{names:?}");
    }
}
