//! Stage 2: compile-time elaboration above the check.
//!
//! A pass between linking and the check that normalizes declarations, per
//! `docs/notes/comptime.md`. It evaluates nothing: the check binds every
//! parameter expression, and `native::mono` evaluates each one below MIR,
//! running a compile-time application once on the register VM
//! (`docs/notes/ctfe-request-path.md`), as upstream evaluates parameter
//! expressions only in its elaborator, after sema.
//!
//! - **Preparation** ([`prepare`]) — qualifies struct packs, synthesizes the
//!   derived `copy` and `__hash__`, parameterizes a declaration over the
//!   parameters its arguments' struct types leave unbound, and folds origin
//!   and SIMD alias bounds and `!kgen.type` parameters. It selects no arm
//!   and unrolls no loop.
//! - **Elaboration** ([`elaborate_prepared`]) — every `comptime`
//!   declaration, `comptime if`, `comptime for`, and `comptime match` is
//!   kept as written. A body's alias of an element of its own type pack
//!   (`comptime T = Ts[i]`) is spelled as that element in the statements
//!   after it, and a leading-dot `comptime` initializer is spelled at its
//!   annotated type. At module scope the parser rejects a `comptime if`
//!   or `comptime match`, and `compiler::validate_module_scope` a `comptime
//!   for`.

use mojito_ast::ast::{
    Expr, ExprKind, InfixOp, ParamArg, PrefixOp, Stmt, StmtKind, TypeParam, WithItem,
};
pub use mojito_symbol::symbol::mangle;
use mojito_types::types::TyKind;

use mojito_checker::checker::InstalledInterfaces;
use mojito_common::token::Span;
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{ParamContext, ParamExpr};
use mojito_types::types::{ParamDecl, Ty, list_type, tuple_type};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// An error from compile-time elaboration.
#[derive(Debug)]
pub enum ComptimeError {
    /// A checker diagnostic, reported verbatim: an undeclared trait a
    /// `!kgen.type` parameter is bounded by.
    Type(mojito_common::error::TypeError),
    /// A variadic struct member spelled the struct's own pack bare, where
    /// upstream requires `Self.Ts`; the message is upstream's diagnostic, the
    /// same text the checker reports for a non-pack parameter.
    UnqualifiedStructParam(String),
}

impl std::fmt::Display for ComptimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnqualifiedStructParam(name) => write!(
                f,
                "unqualified access to struct parameter '{name}'; use 'Self.{name}' instead"
            ),
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
/// Qualify struct packs, synthesize the derived `copy`, `__hash__`,
/// `__eq__`, `write_to`, and `write_repr_to` methods,
/// parameterize each declaration over the parameters its arguments' struct
/// types leave unbound, fold origin and SIMD alias bounds, and fold a
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
    synthesize_copyable_copy(&mut own, installed);
    synthesize_hashable_hash(&mut own);
    synthesize_equatable_eq(&mut own);
    auto_parameterize::parameterize_unbound_arguments(&mut own, installed);
    fold_simd_alias_bounds(&mut own, installed);
    fold_kgen_type_parameters(&mut own, installed)?;
    Ok(own)
}

/// Elaborate a [`prepare`]d program.
///
/// Every `comptime` declaration, `comptime if`, `comptime for`, and
/// `comptime match` is kept as written: the check binds each parameter
/// expression, and `native::mono` evaluates it, as upstream evaluates
/// parameter expressions only in its elaborator, after sema. An importing
/// program elaborates only its own statements, as upstream's importer takes
/// a package declaration already resolved. Ordinary callers use
/// [`elaborate`]; the compiler, which prepares once, enters here.
///
/// # Errors
///
/// None today; the signature keeps the stage's seam.
pub fn elaborate_prepared(program: &[Stmt]) -> Result<Vec<Stmt>, ComptimeError> {
    let mut result = elab::block(program, None);
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    stamp_enum_storage_writes(&mut result);
    Ok(result)
}

mod auto_parameterize;
mod elab;
mod pack_qualification;
mod params;
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

/// The name an annotation applies and its bracket operands: a bare name
/// other than `Self` (no operands) or a bracket on one (`Pair[Int]`,
/// `List[]`), as the checker reads the annotation's head.
fn applied_name(annotation: &Expr) -> Option<(&str, Cow<'_, [ParamArg]>)> {
    match &annotation.kind {
        ExprKind::Identifier(name)
            if !annotation.is_self_annotation() && !annotation.is_sentinel() =>
        {
            Some((name, Cow::Borrowed(&[][..])))
        }
        _ => annotation
            .bracket_application()
            .filter(|(name, _)| *name != "Self"),
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

/// The alias a `comptime NAME = Ts[index]` statement declares of an element
/// of a pack `is_pack` accepts, beside the expression `Ts[index]` it
/// denotes: such an alias names a parameter expression, so a template
/// carries it as that dependent element rather than evaluating it with the
/// index unknown. An enclosing struct's pack is spelled, and asked of
/// `is_pack`, as `Self.Ts`.
pub(super) fn pack_element_alias(
    kind: &StmtKind,
    is_pack: &dyn Fn(&str) -> bool,
) -> Option<(String, Expr)> {
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
    let ExprKind::Index { object, .. } = &value.kind else {
        return None;
    };
    if !type_params.is_empty() || !where_clauses.is_empty() {
        return None;
    }
    let pack = match &object.kind {
        ExprKind::Identifier(base) => is_pack(base),
        _ => object
            .self_member()
            .is_some_and(|field| is_pack(&format!("Self.{field}"))),
    };
    pack.then(|| (name.clone(), value.clone()))
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
        ExprKind::Prefix(PrefixOp::Pos, value) => ct_expr_from_ast(value, siblings, owner),
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

/// The type a value parameter's annotation names: `None`, a trait-valued
/// `type_of(...)`, or an applied name ([`ct_param_applied_type`]).
fn ct_param_source_type(source: &Expr) -> Option<Ty> {
    match &source.kind {
        ExprKind::None => Some(Ty::none()),
        // A trait-valued parameter (`Tr: type_of(AnyType)`).
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } if name == "type_of" && param_args.is_empty() && kwargs.is_empty() => {
            match args.as_slice() {
                [operand] => any_trait_source_type(operand),
                _ => None,
            }
        }
        _ => ct_param_applied_type(source),
    }
}

/// The type an applied name names: a scalar alias, or a `List`, `Tuple`,
/// or `SIMD` application.
fn ct_param_applied_type(source: &Expr) -> Option<Ty> {
    let (name, args) = applied_name(source)?;
    match (name, args.as_ref()) {
        // A scalar alias (`Int`, `Float64`, `Int8`): after linking, a bare
        // alias name is std's, since a program's own declaration is renamed.
        (name, []) => scalar_type_name(name),
        ("List", [element]) => Some(list_type(ct_param_argument_type(element)?)),
        ("Tuple", args) => args
            .iter()
            .map(ct_param_argument_type)
            .collect::<Option<Vec<_>>>()
            .map(tuple_type),
        // A vector-typed value parameter (`[key: SIMD[DType.uint64, 4]]`).
        ("SIMD", args) => simd_source_dims(args)
            .map(|(dtype, width)| mojito_types::types::canonical_simd_ty(dtype, width)),
        _ => None,
    }
}

/// The type an element argument of a value binder's `List` or `Tuple`
/// annotation names: a type, or an application operand (`List[Int]`,
/// which parses as an expression) by the type it spells.
fn ct_param_argument_type(argument: &ParamArg) -> Option<Ty> {
    match argument {
        ParamArg::Value(value) => ct_param_applied_type(value),
        ParamArg::Named { .. } => None,
    }
}

fn scalar_type_name(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::int()),
        // A `DType` value, and the type of a `[dtype: DType]` value parameter.
        "DType" => Some(Ty::dtype()),
        "UInt" => Some(Ty::uint()),
        "Bool" => Some(Ty::bool()),
        "Float64" => Some(Ty::float64()),
        "None" => Some(Ty::none()),
        "NoneType" => Some(Ty::none_type()),
        // The (qualified) `String` spelling deliberately falls through to
        // ordinary struct resolution: in type-argument and type-value
        // positions it denotes the nominal stdlib struct. Value-parameter
        // classification keeps the literal type via `ct_value_param_type`.
        // The sized scalar aliases (`Int8`, `UInt64`, `Float32`, ...) are
        // width-1 SIMD types, so `comptime c_int = Int32` is a type value.
        _ => mojito_ast::ast::Dtype::from_scalar_alias(name)
            .map(|dtype| mojito_types::types::canonical_simd_ty(dtype, 1)),
    }
}

/// The source spelling of upstream's `!kgen.string` type,
/// `` __mlir_type.`!kgen.string` ``, at `span`.
fn kgen_string_source_type(span: mojito_common::token::Span) -> Expr {
    Expr::new(
        ExprKind::Member {
            object: Box::new(Expr::new(
                ExprKind::Identifier("__mlir_type".to_string()),
                span,
            )),
            field: "!kgen.string".to_string(),
        },
        span,
    )
}

mod rewrite;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rewrite::*;
