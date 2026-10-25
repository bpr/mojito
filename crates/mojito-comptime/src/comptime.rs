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
use mojito_types::types::TyKind;

use mojito_checker::checker::{ElaboratedConstant, ElaboratedConstants, InstalledInterfaces};
use mojito_common::token::Span;
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
    /// indexing, `len`, and dictionary literals. A `TypeList` value (`Sized` and iterable
    /// upstream) yields its element types.
    fn as_sequence(&self, ctx: &str) -> Result<Vec<CtValue>, ComptimeError> {
        // A dictionary iterates (and counts) its keys, as at runtime.
        self.comptime_iteration_elements()
            .or_else(|| self.typelist_elements().map(<[Self]>::to_vec))
            .ok_or_else(|| ComptimeError::NotCollection(ctx.to_string()))
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
    /// A compile-time collection read (an index, a `len`, a dictionary
    /// literal) over a value that is no collection.
    NotCollection(String),
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
            Self::NotCollection(s) => write!(f, "not a compile-time collection: {s}"),
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

/// What an importing program's front end reads of the package it imports,
/// beside its own statements. The default is a program that imports none.
#[derive(Clone, Copy, Default)]
pub struct ImportedScope<'a> {
    /// The declaration interfaces of the modules the link reached: the
    /// package's resolved declarations, and its module constants as the
    /// build's elaboration folded them, each read on first lookup.
    pub installed: Option<&'a InstalledInterfaces>,
}

/// Prepare a linked program for source validation and elaboration.
///
/// Qualify struct packs, synthesize the derived `copy`/`__hash__` methods,
/// desugar `SIMD[_, _]` parameters, fold SIMD alias bounds, and fold a
/// `!kgen.type`-typed parameter into a type binder. A trait
/// default stays in its trait; a conformer's wrapper for a default it
/// inherits is the check's to build, over the default's resolved signature.
///
/// These rewrites normalize declarations without selecting a `comptime if`
/// arm or unrolling a loop, so the result still carries every source body
/// the check must see.
pub fn prepare(program: Vec<Stmt>) -> Result<Vec<Stmt>, ComptimeError> {
    prepare_importing(program, None)
}

/// [`prepare`] a program's own statements, `own`, beside the package it
/// imports.
///
/// `installed` holds the package's module constants as its build
/// elaborated them, whose SIMD aliases bound an own parameter. No package
/// declaration is read or rewritten.
pub fn prepare_importing(
    mut own: Vec<Stmt>,
    installed: Option<&InstalledInterfaces>,
) -> Result<Vec<Stmt>, ComptimeError> {
    pack_qualification::qualify_struct_packs(&mut own)?;
    synthesize_copyable_copy(&mut own);
    synthesize_hashable_hash(&mut own);
    desugar_simd_wildcard_parameters(&mut own);
    fold_simd_alias_bounds(&mut own, installed);
    fold_kgen_type_parameters(&mut own, installed)?;
    read_materialize_self_operands(&mut own);
    Ok(own)
}

/// Elaborate a [`prepare`]d program.
///
/// Ordinary callers use [`elaborate`]; the compiler, which prepares once,
/// enters here.
pub fn elaborate_prepared(program: &[Stmt]) -> Result<Vec<Stmt>, ComptimeError> {
    elaborate_importing(program, ImportedScope::default())
}

/// Elaborate a program's own [`prepare_importing`]d statements, `own`,
/// beside the package `imported` holds.
///
/// The package's module constants are read as its build folded them, and
/// its structs and functions as its check resolved them, so only `own` is
/// elaborated and returned, as upstream's importer takes a package
/// declaration already resolved.
pub fn elaborate_importing(
    own: &[Stmt],
    imported: ImportedScope<'_>,
) -> Result<Vec<Stmt>, ComptimeError> {
    elaborate_in(own, imported).map(|(elaborated, _)| elaborated)
}

/// Elaborate a [`prepare`]d program as [`elaborate_prepared`] does, and
/// export the module constants it resolved, each beside the module whose
/// `comptime` declaration names it, for a package built from it.
pub fn elaborate_exporting(
    program: &[Stmt],
) -> Result<(Vec<Stmt>, ElaboratedConstants), ComptimeError> {
    let (elaborated, elab) = elaborate_in(program, ImportedScope::default())?;
    let values = elab.top_consts.into_inner();
    let aliases = elab.generic_aliases.into_inner();
    let mut constants = Vec::new();
    for statement in program {
        let (StmtKind::Comptime { name, .. }, Some(module)) = (&statement.kind, &statement.module)
        else {
            continue;
        };
        let constant = match (values.get(name), aliases.get(name)) {
            (Some(value), _) => ElaboratedConstant::Value(value.clone()),
            (None, Some(alias)) => ElaboratedConstant::Alias(
                alias.params.clone(),
                alias.where_clauses.clone(),
                alias.body.clone(),
            ),
            (None, None) => continue,
        };
        constants.push((module.clone(), name.clone(), constant));
    }
    Ok((elaborated, constants))
}

mod crossing;
mod elab;
mod pack_qualification;
mod params;
mod synth;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use params::*;
#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use synth::*;

/// Elaborate `program` beside `imported`, returning the elaboration and the
/// engine that ran it, whose module environment a package build exports.
fn elaborate_in<'a>(
    program: &'a [Stmt],
    imported: ImportedScope<'a>,
) -> Result<(Vec<Stmt>, Elab<'a>), ComptimeError> {
    let indexes = mojito_common::timing::span("indexes");
    let conformance =
        mojito_checker::checker::ConformanceOracle::importing(program, imported.installed)
            .map_err(|error| {
                ComptimeError::NotComptime(format!(
                    "could not build the specialization conformance oracle: {error}"
                ))
            })?;
    let elab = Elab {
        program,
        fns: collect_fns(program),
        structs: collect_structs(program, &conformance),
        conformance,
        template_binders: RefCell::new(Vec::new()),
        crossing_templates: Cell::new(0),
        top_consts: RefCell::new(HashMap::new()),
        generic_aliases: RefCell::new(HashMap::new()),
        installed: imported.installed,
    };
    drop(indexes);
    let mut env = ConstEnv::over(HashMap::new(), imported.installed);
    let aliases = elab::TypeAliases::importing(imported.installed);
    let mut elaborated = elab.block_statements(program, &mut env, false, aliases)?;
    // A module constant declared after its use crosses here.
    let consts = ConstEnv::over(elab.top_consts.borrow().clone(), imported.installed);
    elab.fold_runtime_crossings(&mut elaborated, &consts)?;
    // A body's read of a module constant stays a name: the check binds it as
    // the parameter value its declaration denotes (decision D3).
    let mut result = mark_struct_names(elaborated, &|name| elab.declares_struct(name));
    elab.freeze_struct_value_arguments(&mut result, &consts);
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    stamp_enum_storage_writes(&mut result);
    Ok((result, elab))
}

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

/// The value of the imported package's module constant `name`.
fn installed_value<'a>(
    installed: Option<&'a InstalledInterfaces>,
    name: &str,
) -> Option<&'a CtValue> {
    match installed?.elaborated(name)? {
        ElaboratedConstant::Value(value) => Some(value),
        ElaboratedConstant::Alias(..) => None,
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

/// The trait metatype `type_of(operand)` denotes when the operand is a
/// trait or a `&` composition of traits. Which spelled traits a sibling
/// implies is the checker's to drop.
pub(super) fn any_trait_source_type(operand: &Expr) -> Option<Ty> {
    mojito_ast::ast::trait_conjunction_names(operand).map(|names| {
        TyKind::AnyTrait(mojito_types::types::trait_set(
            names.into_iter().map(str::to_string),
        ))
        .into()
    })
}

/// Substitute every now-concrete type binder `subs` answers in a source
/// annotation, wherever it appears unshadowed.
fn substitute_source_type_bindings(ty: &mut Type, subs: rewrite::TypeSubs) {
    match ty {
        Type::Named(name, arguments) if arguments.is_empty() => {
            if let Some(replacement) = subs(name) {
                *ty = replacement;
            }
        }
        Type::Named(_, arguments) => {
            for argument in arguments {
                substitute_source_param_arg_bindings(argument, subs);
            }
        }
        // `Self.T` — the enclosing struct's own parameter spelled through
        // `Self`, the dominant spelling inside struct bodies.
        Type::SelfParam(name) => {
            if let Some(replacement) = subs(name) {
                *ty = replacement;
            }
        }
        Type::Assoc { base, name, args }
            if args.is_empty() && matches!(base.as_ref(), Type::SelfType) =>
        {
            if let Some(replacement) = subs(name) {
                *ty = replacement;
            }
        }
        Type::Assoc { base, args, .. } => {
            substitute_source_type_bindings(base, subs);
            for argument in args {
                substitute_source_param_arg_bindings(argument, subs);
            }
        }
        Type::IndexedProjection { base, .. } => {
            substitute_source_type_bindings(base, subs);
        }
        Type::Func {
            type_params,
            params,
            ret,
            raises_type,
            ..
        } => {
            // The contract's own binder of that spelling shadows the binding.
            let binders: Vec<String> = type_params
                .iter()
                .map(|parameter| parameter.name.trim_start_matches('*').to_string())
                .collect();
            let inner = |name: &str| {
                if binders.iter().any(|binder| binder == name) {
                    None
                } else {
                    subs(name)
                }
            };
            for parameter in type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    substitute_source_type_bindings(value_type, &inner);
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    substitute_source_type_bindings(callable, &inner);
                }
            }
            for parameter in params {
                substitute_source_type_bindings(&mut parameter.ty, &inner);
            }
            substitute_source_type_bindings(ret, &inner);
            if let Some(error) = raises_type {
                substitute_source_type_bindings(error, &inner);
            }
        }
        Type::Ref { referent, .. } => {
            substitute_source_type_bindings(referent, subs);
        }
        // The operand of `type_of` is an expression, which names no type
        // binder.
        Type::TypeOf(_) => {}
        Type::Value(value) => rewrite::substitute_type_bindings_in_expr(value, subs),
        Type::Int
        | Type::UInt
        | Type::Bool
        | Type::StringLiteral
        | Type::ClosedStringLiteral
        | Type::Float64
        | Type::None
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
        Type::Int => Some(Ty::int()),
        Type::UInt => Some(Ty::uint()),
        Type::Bool => Some(Ty::bool()),
        Type::StringLiteral | Type::ClosedStringLiteral => Some(Ty::string_literal()),
        Type::Float64 => Some(Ty::float64()),
        Type::None => Some(Ty::none()),
        // A trait-valued parameter (`Tr: type_of(AnyType)`).
        Type::TypeOf(operand) => any_trait_source_type(operand),
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
            simd_source_dims(args).map(|(dtype, width)| {
                TyKind::Simd {
                    dtype: mojito_types::types::SimdDtype::Known(dtype),
                    width: mojito_types::types::SimdWidth::Known(width),
                }
                .into()
            })
        }
        _ => None,
    }
}

fn source_type_from_ty_with_origins(
    ty: &Ty,
    origin_names: &HashMap<mojito_types::origin::OriginParamId, String>,
) -> Option<Type> {
    Some(match ty.kind() {
        TyKind::Int | TyKind::IntLiteral => Type::Int,
        TyKind::UInt => Type::UInt,
        TyKind::Bool => Type::Bool,
        TyKind::StringLiteral => Type::ClosedStringLiteral,
        TyKind::Float64 | TyKind::FloatLiteral => Type::Float64,
        TyKind::None => Type::None,
        TyKind::Dtype => Type::Named("DType".to_string(), Vec::new()),
        TyKind::KgenString => kgen_string_source_type(),
        // A callable has no source spelling a clone could carry.
        TyKind::Func(_) | TyKind::GenericFunc(_) => return None,
        TyKind::ComptimeList(element) => Type::Named(
            "List".to_string(),
            vec![ParamArg::Type(source_type_from_ty_with_origins(
                element,
                origin_names,
            )?)],
        ),
        // Upstream's `_MLIR.KGENParamListType[T]` body.
        TyKind::ParamList(element) => {
            let delimiter = |text: &str| {
                ParamArg::Value(mojito_ast::ast::Expr::from(
                    mojito_ast::ast::ExprKind::Identifier(text.to_string()),
                ))
            };
            Type::Named(
                "__mlir_type".to_string(),
                vec![
                    delimiter("!kgen.param_list<"),
                    ParamArg::Type(source_type_from_ty_with_origins(element, origin_names)?),
                    delimiter(">"),
                ],
            )
        }
        TyKind::Tuple(elements) => Type::Named(
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
        TyKind::Struct(name, _)
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
        TyKind::Struct(name, arguments) => Type::Named(
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
        TyKind::Simd { dtype, width } => Type::Named(
            "SIMD".to_string(),
            vec![
                ParamArg::Value(CtValue::Dtype(dtype.known()?).materialize((0, 0))?),
                ParamArg::Value(CtValue::Int(width.known()?).materialize((0, 0))?),
            ],
        ),
        TyKind::Ref(reference) => {
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

/// Compile-time metadata for one of the program's own top-level structs,
/// enough to read associated facts such as `T.size`.
struct CtStruct<'a> {
    decls: Vec<ParamDecl>,
    /// The source parameters `decls` classified from — the fallback for
    /// declared defaults classification cannot resolve without evaluation
    /// (`H: Hasher = default_hasher` names a module alias).
    source_params: &'a [TypeParam],
    /// Its own `comptime` members, then the defaults of the conformed
    /// traits' requirements it omits.
    associated: std::borrow::Cow<'a, [StructComptime]>,
    fields: &'a [mojito_ast::ast::Param],
}

/// A struct the elaborator reads: one of the program's own, from its
/// declaration, or one of the imported package's, as the package's check
/// resolved it.
#[derive(Clone, Copy)]
enum StructView<'e> {
    Own(&'e CtStruct<'e>),
    Package(mojito_checker::checker::PackageStruct<'e>),
}

impl<'e> StructView<'e> {
    fn decls(self) -> &'e [ParamDecl] {
        match self {
            Self::Own(info) => &info.decls,
            Self::Package(info) => info.decls(),
        }
    }

    fn source_params(self) -> &'e [TypeParam] {
        match self {
            Self::Own(info) => info.source_params,
            Self::Package(info) => info.source_params(),
        }
    }

    fn field_names(self) -> Vec<&'e str> {
        match self {
            Self::Own(info) => info
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect(),
            Self::Package(info) => info.field_names().iter().map(String::as_str).collect(),
        }
    }
}

/// The compile-time elaboration engine: the CTFE-callable functions and a shared
/// fuel budget. `top_consts` captures module-level constants for materialization.
struct Elab<'a> {
    program: &'a [Stmt],
    /// The program's own `def` names; the package's are the oracle's.
    fns: HashSet<String>,
    /// The program's own structs; the package's are the oracle's.
    structs: HashMap<String, CtStruct<'a>>,
    /// Checker-owned declaration facts used to validate inferred pack bounds
    /// before specialization consumes the source generic call, and the
    /// imported package's resolved structs and functions.
    conformance: mojito_checker::checker::ConformanceOracle,
    /// The compile-time parameter names of each generic `def` whose body is
    /// being elaborated as a template, innermost last. A `comptime if`
    /// whose condition names one is kept for the check: its arms are the
    /// template's, and the elaborator below MIR selects.
    template_binders: RefCell<Vec<HashSet<String>>>,
    /// How many function bodies the runtime-crossing pass has descended into.
    crossing_templates: Cell<usize>,
    /// The program's own module constants; an imported package's are
    /// `installed`'s ([`Self::module_constant`]).
    top_consts: RefCell<HashMap<String, CtValue>>,
    /// The program's own module-scope generic `comptime` aliases in
    /// declaration order; an imported package's are `installed`'s
    /// ([`Self::generic_alias`]). The declarations pass
    /// through elaboration for the checker's alias registry, but an
    /// application inside a `comptime if` condition must already evaluate
    /// here — the branches are pruned before checking.
    generic_aliases: RefCell<HashMap<String, GenericAlias>>,
    /// The declaration interfaces of the package the program imports, whose
    /// module constants a lookup the program's own miss reads, each decoded
    /// on first lookup.
    installed: Option<&'a InstalledInterfaces>,
}

/// A module-scope generic `comptime` alias as a compile-time application
/// evaluates it: its parameters, its `where` clauses, and its body.
#[derive(Clone)]
struct GenericAlias {
    params: Vec<TypeParam>,
    where_clauses: Vec<Expr>,
    body: Expr,
}

/// The compile-time values an elaboration reads by name: its own frame,
/// and under it the module constants of the package the program imports,
/// each decoded the first time a lookup the frame misses names it, as
/// upstream resolves a package declaration on first lookup.
#[derive(Clone, Default)]
struct ConstEnv<'a> {
    frame: HashMap<String, CtValue>,
    /// The names a local binding of the frame's scope shadows without a
    /// value, which no package constant answers.
    shadowed: HashSet<String>,
    installed: Option<&'a InstalledInterfaces>,
}

impl<'a> ConstEnv<'a> {
    /// `frame` over the module constants of the package `installed` holds.
    fn over(frame: HashMap<String, CtValue>, installed: Option<&'a InstalledInterfaces>) -> Self {
        Self {
            frame,
            shadowed: HashSet::new(),
            installed,
        }
    }

    /// The value bound to `name`: the frame's, or a package constant's.
    fn get(&self, name: &str) -> Option<&CtValue> {
        self.frame.get(name).or_else(|| {
            if self.shadowed.contains(name) {
                None
            } else {
                installed_value(self.installed, name)
            }
        })
    }

    /// Bind `name` in the frame.
    fn insert(&mut self, name: String, value: CtValue) -> Option<CtValue> {
        self.frame.insert(name, value)
    }

    /// Shadow `name` with a binding of no compile-time value.
    fn shadow(&mut self, name: &str) {
        self.frame.remove(name);
        self.shadowed.insert(name.to_string());
    }
}

impl Elab<'_> {
    /// The module constant `name`: the program's own, or the imported
    /// package's.
    fn module_constant(&self, name: &str) -> Option<CtValue> {
        self.top_consts
            .borrow()
            .get(name)
            .cloned()
            .or_else(|| installed_value(self.installed, name).cloned())
    }

    /// Whether `name` is a module-scope generic alias, the program's own or
    /// the imported package's.
    fn is_generic_alias(&self, name: &str) -> bool {
        self.generic_aliases.borrow().contains_key(name)
            || matches!(
                self.installed
                    .and_then(|installed| installed.elaborated(name)),
                Some(ElaboratedConstant::Alias(..))
            )
    }

    /// The module-scope generic alias `name`.
    fn generic_alias(&self, name: &str) -> Option<GenericAlias> {
        self.generic_aliases
            .borrow()
            .get(name)
            .cloned()
            .or_else(|| match self.installed?.elaborated(name)? {
                ElaboratedConstant::Alias(params, where_clauses, body) => Some(GenericAlias {
                    params: params.clone(),
                    where_clauses: where_clauses.clone(),
                    body: body.clone(),
                }),
                ElaboratedConstant::Value(_) => None,
            })
    }

    /// Struct `name`, the program's own or the imported package's.
    fn ct_struct(&self, name: &str) -> Option<StructView<'_>> {
        self.structs.get(name).map(StructView::Own).or_else(|| {
            self.conformance
                .package_struct(name)
                .map(StructView::Package)
        })
    }

    /// Whether `name` is a struct, the program's own or the imported
    /// package's, decoding no package declaration.
    fn declares_struct(&self, name: &str) -> bool {
        self.structs.contains_key(name) || self.conformance.names_package_struct(name)
    }

    /// Whether `name` is a `def`, the program's own or the imported
    /// package's, decoding no package declaration.
    fn declares_function(&self, name: &str) -> bool {
        self.fns.contains(name) || self.conformance.names_package_function(name)
    }
}

fn classify_ct_params(tps: &[TypeParam], owner: &str) -> Vec<ParamDecl> {
    tps.iter()
        .filter_map(|tp| classify_ct_param(tp, tps, owner))
        .collect()
}

fn materialize_ct_value(value: CtValue, ty: &Ty) -> Option<CtValue> {
    value.materialize_as(ty)
}

fn substitute_source_param_arg_bindings(argument: &mut ParamArg, subs: rewrite::TypeSubs) {
    match argument {
        ParamArg::Type(ty) => substitute_source_type_bindings(ty, subs),
        ParamArg::Named { value, .. } => substitute_source_param_arg_bindings(value, subs),
        // The parser encodes a bare identifier argument (`Tuple[T, T]`) as a
        // value expression; once the binding is concrete it is a type argument.
        ParamArg::Value(expr) => {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(replacement) = subs(name)
            {
                *argument = ParamArg::Type(replacement);
            }
        }
    }
}

fn scalar_type_name(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::int()),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Some(Ty::dtype()),
        // A SIMD width parameter is a compile-time Int value parameter (the
        // removed `SIMDSize` spelling rejects).
        "SIMDLength" => Some(Ty::int()),
        "UInt" => Some(Ty::uint()),
        "Bool" => Some(Ty::bool()),
        "StringLiteral" => Some(Ty::string_literal()),
        "Float64" => Some(Ty::float64()),
        "None" | "NoneType" => Some(Ty::none()),
        // The (qualified) `String` spelling deliberately falls through to
        // ordinary struct resolution: in type-argument and type-value
        // positions it denotes the nominal stdlib struct. Value-parameter
        // classification keeps the literal type via `ct_value_param_type`.
        // The sized scalar aliases (`Int8`, `UInt64`, `Float32`, ...) are
        // width-1 SIMD types, so `comptime c_int = Int32` is a type value.
        _ => mojito_ast::ast::Dtype::from_scalar_alias(name).map(|dtype| {
            TyKind::Simd {
                dtype: mojito_types::types::SimdDtype::Known(dtype),
                width: mojito_types::types::SimdWidth::Known(1),
            }
            .into()
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

fn collect_structs<'a>(
    program: &'a [Stmt],
    conformance: &mojito_checker::checker::ConformanceOracle,
) -> HashMap<String, CtStruct<'a>> {
    let mut structs = HashMap::new();
    for s in program {
        if let StmtKind::Struct {
            name,
            type_params,
            conforms,
            associated,
            fields,
            ..
        } = &s.kind
        {
            let mut defaults = conformance.comptime_defaults(conforms);
            defaults.retain(|default| associated.iter().all(|member| member.name != default.name));
            let associated = if defaults.is_empty() {
                std::borrow::Cow::Borrowed(associated.as_slice())
            } else {
                std::borrow::Cow::Owned([associated.as_slice(), &defaults].concat())
            };
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

/// The source spelling of upstream's `!kgen.string` type,
/// `` __mlir_type.`!kgen.string` ``.
fn kgen_string_source_type() -> Type {
    Type::Assoc {
        base: Box::new(Type::Named("__mlir_type".to_string(), Vec::new())),
        name: "!kgen.string".to_string(),
        args: Vec::new(),
    }
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
