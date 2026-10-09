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
//!
//! Compile-time values are the shared [`CtValue`](mojito_types::ct::CtValue) universe:
//! runtime-materializable `Int`/`Bool`/`String`/`Tuple`/`List`, plus
//! compile-time-only `Type` and symbolic `Param` facts.

use mojito_ast::ast::{
    Expr, ExprKind, FnParam, InfixOp, ParamArg, ParamKind, PrefixOp, Stmt, StmtKind,
    StructComptime, TStringPart, Type, TypeParam, WithItem,
};
pub use mojito_symbol::symbol::mangle;

use mojito_common::token::{Span, SyntaxId};
use mojito_types::ct::{CtMarker, CtValue};
use mojito_types::param_expr::{ParamContext, ParamError, ParamExpr};
use mojito_types::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

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
        }
    }
}

/// Elaborate all compile-time constructs in a program, returning an ordinary AST.
///
/// The composed-stage seam: prepares the program and elaborates it, the
/// same contract the compiler driver enforces.
pub fn elaborate(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    let prepared = prepare(program)?;
    elaborate_prepared(&prepared)
}

/// Prepare a linked program for source validation and elaboration.
///
/// Qualify struct packs, synthesize the derived `copy`/`__hash__` methods,
/// give each conformer the trait defaults it inherits, desugar `SIMD[_, _]`
/// parameters, and fold SIMD alias bounds.
///
/// These rewrites normalize declarations without selecting a `comptime if`
/// arm or unrolling a loop, so the result still carries every source body
/// the check must see.
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

/// Elaborate a [`prepare`]d program.
///
/// Ordinary callers use [`elaborate`]; the compiler, which prepares once,
/// enters here.
pub fn elaborate_prepared(program: &[Stmt]) -> Result<Vec<Stmt>, ComptimeError> {
    let indexes = mojito_common::timing::span("indexes");
    let conformance =
        mojito_checker::checker::ConformanceOracle::from_program(program).map_err(|error| {
            ComptimeError::NotComptime(format!(
                "could not build the specialization conformance oracle: {error}"
            ))
        })?;
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
        conformance,
        template_binders: RefCell::new(Vec::new()),
        crossing_templates: Cell::new(0),
        top_consts: RefCell::new(HashMap::new()),
        applied_displays: RefCell::new(HashMap::new()),
        generic_aliases: RefCell::new(HashMap::new()),
    };
    drop(indexes);
    let mut env = HashMap::new();
    let mut elaborated = elab.block(program, &mut env, false)?;
    elab.request_display_reads(&mut elaborated);
    // A module constant declared after its use crosses here.
    let consts = elab.top_consts.borrow().clone();
    elab.fold_runtime_crossings(&mut elaborated, &consts)?;
    // Materialize module-level comptime constants into runtime literals. A
    // scalar or an applied constant stays a name: the check binds a body's
    // read of it as the parameter value its declaration denotes (decision
    // D3).
    let materialized_consts: HashMap<String, CtValue> = consts
        .iter()
        .filter(|(_, value)| !is_scalar_constant(value))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut result = materialize_block(elaborated, &materialized_consts, &elab.struct_names);
    elab.freeze_struct_value_arguments(&mut result, &consts);
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    Ok(result)
}

mod crossing;
mod elab;
mod pack_qualification;
mod params;
mod requests;
mod synth;

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

/// Whether a module constant's value is a scalar, or an applied constant,
/// the check reads as a parameter value where a body names it, rather than
/// a literal the elaborator spells there.
const fn is_scalar_constant(value: &CtValue) -> bool {
    matches!(
        value,
        CtValue::Int(_)
            | CtValue::IntLiteral(_)
            | CtValue::UInt(_)
            | CtValue::Float(_)
            | CtValue::FloatLiteral(_)
            | CtValue::Bool(_)
            | CtValue::Str(_)
            | CtValue::Marker(CtMarker::Applied)
    )
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

/// Compile-time metadata for a top-level struct, enough to read associated
/// facts such as `T.size`.
struct CtStruct<'a> {
    decls: Vec<ParamDecl>,
    /// The source parameters `decls` classified from — the fallback for
    /// declared defaults classification cannot resolve without evaluation
    /// (`H: Hasher = default_hasher` names a module alias).
    source_params: &'a [TypeParam],
    associated: &'a [StructComptime],
    fields: &'a [mojito_ast::ast::Param],
}

/// The compile-time elaboration engine: the CTFE-callable functions and a shared
/// fuel budget. `top_consts` captures module-level constants for materialization.
struct Elab<'a> {
    program: &'a [Stmt],
    fns: HashSet<String>,
    structs: HashMap<String, CtStruct<'a>>,
    /// Every declared struct name, for materialization's projection rewrite.
    struct_names: HashSet<String>,
    /// Checker-owned declaration facts used to validate inferred pack bounds
    /// before specialization consumes the source generic call.
    conformance: mojito_checker::checker::ConformanceOracle,
    /// The compile-time parameter names of each generic `def` whose body is
    /// being elaborated as a template, innermost last. A `comptime if`
    /// whose condition names one is kept for the check: its arms are the
    /// template's, and the elaborator below MIR selects.
    template_binders: RefCell<Vec<HashSet<String>>>,
    /// How many function bodies the runtime-crossing pass has descended into.
    crossing_templates: Cell<usize>,
    top_consts: RefCell<HashMap<String, CtValue>>,
    /// The module constants whose initializer is a display that applies a
    /// callable, by name, each as the display spelled where it is read
    /// (`requests.rs`).
    applied_displays: RefCell<HashMap<String, Expr>>,
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

/// The names of the module's `def`s, which an application names.
fn collect_fns(program: &[Stmt]) -> HashSet<String> {
    program
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Def { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
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
                    decls: classify_ct_params(type_params, name),
                    source_params: type_params,
                    associated,
                    fields,
                },
            );
        }
    }
    structs
}

fn source_type_from_ty(ty: &Ty) -> Option<Type> {
    source_type_from_ty_with_origins(ty, &HashMap::new())
}

fn lit_result(val: &CtValue, span: Span) -> Result<Expr, ComptimeError> {
    val.materialize(span).ok_or_else(|| {
        ComptimeError::NotComptime(
            "type-valued or symbolic comptime values cannot materialize at runtime".to_string(),
        )
    })
}

mod eval;

mod rewrite;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rewrite::*;
