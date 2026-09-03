//! Runtime-pack plumbing: pack argument typing, forwarding-call
//! detection, tuple storage/transform helpers, and whole-pack ABI
//! selection.

use super::*;

pub(super) fn infer_pack_argument_type(expr: &Expr) -> Result<Ty, ComptimeError> {
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

pub(super) fn runtime_pack_spread_source(expression: &Expr) -> Option<&str> {
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

pub(super) fn forwarded_runtime_pack_type(source: &Type) -> Option<Ty> {
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

pub(super) fn runtime_pack_call_argument_indices(
    template: &Stmt,
    display_name: &str,
    positional_count: usize,
    kwargs: &[mojito_ast::ast::KwArg],
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
            parameter.kind == mojito_ast::ast::ParamKind::Regular
                && !matches!(
                    parameter.convention,
                    Some(mojito_ast::ast::ArgConvention::Out)
                )
        })
        .collect();
    let variadic = params
        .iter()
        .position(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Variadic);
    let kw_variadic = params
        .iter()
        .any(|parameter| parameter.kind == mojito_ast::ast::ParamKind::KwVariadic);
    let marker = |source: Option<usize>| {
        source.map(|index| {
            params[..index]
                .iter()
                .filter(|parameter| {
                    parameter.kind == mojito_ast::ast::ParamKind::Regular
                        && !matches!(
                            parameter.convention,
                            Some(mojito_ast::ast::ArgConvention::Out)
                        )
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

pub(super) fn top_level_whole_pack_forwarding_call(
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
                && !matches!(
                    parameter.convention,
                    Some(mojito_ast::ast::ArgConvention::Out)
                )
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

pub(super) fn top_level_forwarded_pack_types(
    template: &Stmt,
    display_name: &str,
    arguments: &[Expr],
    kwargs: &[mojito_ast::ast::KwArg],
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

pub(super) fn unwrap_runtime_pack_arguments(arguments: Vec<Expr>) -> Vec<Expr> {
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
pub(super) fn tuple_storage_element(owner: &str, index: usize, transfer: bool, span: Span) -> Expr {
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
pub(super) fn tuple_transform_method(
    name: &str,
    self_convention: Option<ArgConvention>,
    params: Vec<FnParam>,
    target: String,
    args: Vec<Expr>,
    span: Span,
) -> mojito_ast::ast::Method {
    let result = Expr::new(
        ExprKind::Call {
            name: target.clone(),
            param_args: Vec::new(),
            args,
            kwargs: Vec::new(),
        },
        span,
    );
    mojito_ast::ast::Method {
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
        where_clauses: Vec::new(),
        body: vec![mk(StmtKind::Return(Some(result)), span)],
    }
}

pub(super) fn runtime_pack_call_arguments<'a>(
    template: &Stmt,
    display_name: &str,
    args: &'a [Expr],
    kwargs: &[mojito_ast::ast::KwArg],
) -> Result<Vec<&'a Expr>, ComptimeError> {
    let indices = runtime_pack_call_argument_indices(template, display_name, args.len(), kwargs)?;
    Ok(indices.into_iter().map(|index| &args[index]).collect())
}

/// Give a top-level forwarded specialization the same ownership-safe ABI used
/// by nested whole-pack forwarding: the caller passes its concrete private
/// runtime-pack collector as one regular value and the body binds it directly.
pub(super) fn select_top_level_whole_pack_abi(
    specialization: &mut Stmt,
) -> Result<(), ComptimeError> {
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
