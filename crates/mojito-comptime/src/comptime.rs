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
//!   derived `copy` and `__hash__`, desugars `SIMD[_, _]` parameters, and
//!   folds SIMD alias bounds and `!kgen.type` parameters. It selects no arm
//!   and unrolls no loop.
//! - **Elaboration** ([`elaborate_importing`]) — every `comptime`
//!   declaration, `comptime if`, `comptime for`, and `comptime match` is
//!   kept as written. A body's alias of an element of its own type pack
//!   (`comptime T = Ts[i]`) is spelled as that element in the statements
//!   after it, a leading-dot `comptime` initializer is spelled at its
//!   annotated type, and declared struct names are marked for the check's
//!   projection reading. At module scope the parser rejects a `comptime if`
//!   or `comptime match`, and `compiler::validate_module_scope` a `comptime
//!   for`.

use mojito_ast::ast::{
    Expr, ExprKind, InfixOp, ParamArg, PrefixOp, Stmt, StmtKind, Type, TypeParam, WithItem,
};
pub use mojito_symbol::symbol::mangle;
use mojito_types::types::TyKind;

use mojito_checker::checker::InstalledInterfaces;
use mojito_common::token::Span;
use mojito_types::ct::{CtMarker, CtValue};
use mojito_types::param_expr::{ParamContext, ParamExpr};
use mojito_types::types::{ParamDecl, Ty, TyArg, list_type, tuple_type};
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

/// What an importing program's front end reads of the package it imports,
/// beside its own statements. The default is a program that imports none.
#[derive(Clone, Copy, Default)]
pub struct ImportedScope<'a> {
    /// The declaration interfaces of the modules the link reached, whose
    /// struct names a subscript-shaped projection is told apart by.
    pub installed: Option<&'a InstalledInterfaces>,
}

/// Prepare a linked program for source validation and elaboration.
///
/// Qualify struct packs, synthesize the derived `copy`, `__hash__`,
/// `__eq__`, `write_to`, and `write_repr_to` methods,
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
    synthesize_copyable_copy(&mut own, installed);
    synthesize_hashable_hash(&mut own);
    synthesize_equatable_eq(&mut own);
    synthesize_writable_defaults(&mut own, installed);
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
/// Every `comptime` declaration, `comptime if`, `comptime for`, and
/// `comptime match` is kept as written: the check binds each parameter
/// expression, and `native::mono` evaluates it, as upstream evaluates
/// parameter expressions only in its elaborator, after sema. Only `own` is
/// elaborated and returned, as upstream's importer takes a package
/// declaration already resolved.
///
/// # Errors
///
/// None today; the signature keeps the stage's seam.
pub fn elaborate_importing(
    own: &[Stmt],
    imported: ImportedScope<'_>,
) -> Result<Vec<Stmt>, ComptimeError> {
    let elaborated = elab::block(own, None);
    let own_structs: HashSet<&str> = own
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut result = mark_struct_names(elaborated, &|name| {
        own_structs.contains(name)
            || imported
                .installed
                .is_some_and(|installed| installed.declares_struct(name))
    });
    for statement in &mut result {
        if let Some(source) = statement.module.clone() {
            mojito_ast::ast::stamp_source(std::slice::from_mut(statement), &source);
        }
    }
    stamp_enum_storage_writes(&mut result);
    Ok(result)
}

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

fn mk(kind: StmtKind, span: Span) -> Stmt {
    Stmt {
        kind,
        span,
        module: None,
        syntax_id: mojito_common::token::SyntaxId::fresh(),
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
        Type::Generator { params, result } => {
            // The generator's own binders shadow the binding.
            let binders: Vec<String> = params
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
            for parameter in params {
                if let Some(value_type) = &mut parameter.value_type {
                    substitute_source_type_bindings(value_type, &inner);
                }
            }
            substitute_source_type_bindings(result, &inner);
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
            Some(list_type(ct_param_argument_type(&args[0])?))
        }
        Type::Named(name, args) if name == "Tuple" => args
            .iter()
            .map(ct_param_argument_type)
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

/// The type an element argument of a value binder's `List` or `Tuple`
/// annotation names: a type, or an application operand (`List[Int]`,
/// which parses as an expression) by the type it spells.
fn ct_param_argument_type(argument: &ParamArg) -> Option<Ty> {
    match argument {
        ParamArg::Type(ty) => ct_param_source_type(ty),
        ParamArg::Value(value)
            if matches!(
                value.kind,
                ExprKind::TypeApply { .. } | ExprKind::Index { .. }
            ) =>
        {
            ct_param_source_type(&value.type_spelling()?)
        }
        _ => None,
    }
}

fn source_type_from_ty_with_origins(
    ty: &Ty,
    origin_names: &HashMap<mojito_types::origin::OriginParamId, String>,
) -> Option<Type> {
    Some(match ty.kind() {
        TyKind::INT | TyKind::IntLiteral => Type::Int,
        TyKind::UINT => Type::UInt,
        TyKind::Bool => Type::Bool,
        TyKind::StringLiteral => Type::ClosedStringLiteral,
        TyKind::FLOAT64 | TyKind::FloatLiteral => Type::Float64,
        TyKind::None => Type::None,
        TyKind::Dtype => Type::Named("DType".to_string(), Vec::new()),
        TyKind::KgenString => kgen_string_source_type(),
        TyKind::KgenDeferred => mlir_source_type("!kgen.deferred"),
        TyKind::Index => mlir_source_type("index"),
        TyKind::I1 => mlir_source_type("i1"),
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
        // A vector's storage is spelled through the vector's `_mlir_type`
        // member, which names it.
        TyKind::KgenSimd { .. } => Type::Assoc {
            base: Box::new(source_type_from_ty_with_origins(
                &mojito_types::types::storage_simd(ty)?,
                origin_names,
            )?),
            name: "_mlir_type".to_string(),
            args: Vec::new(),
        },
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

/// The source spelling of upstream's `!kgen.string` type,
/// `` __mlir_type.`!kgen.string` ``.
fn kgen_string_source_type() -> Type {
    mlir_source_type("!kgen.string")
}

/// The `` __mlir_type.`name` `` spelling.
fn mlir_source_type(name: &str) -> Type {
    Type::Assoc {
        base: Box::new(Type::Named("__mlir_type".to_string(), Vec::new())),
        name: name.to_string(),
        args: Vec::new(),
    }
}

fn source_type_from_ty(ty: &Ty) -> Option<Type> {
    source_type_from_ty_with_origins(ty, &HashMap::new())
}

mod rewrite;

#[allow(clippy::wildcard_imports, reason = "pages of this split module")]
use rewrite::*;
