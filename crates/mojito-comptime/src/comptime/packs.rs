//! Runtime-pack plumbing: pack argument typing, forwarding-call
//! detection, tuple storage/transform helpers, and whole-pack ABI
//! selection.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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
            other => Ty::Struct(other.to_string(), Vec::new().into()),
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
            .map(|arguments| Ty::Struct(name.clone(), arguments.into())),
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
/// The element types a type-list expression denotes.
///
/// The expression is a spread's operand (`Ts.reverse()`,
/// `TypeList._concat[A.values, B.values]()`), and `pack` gives the elements
/// of every pack it names. `None` while a pack is unbound or the expression
/// is no type list.
pub(super) fn type_list_source_types(
    operand: &Expr,
    pack: &dyn Fn(&str) -> Option<Vec<Type>>,
) -> Option<Vec<Type>> {
    if let Some(name) = pack_name(operand) {
        return pack(name);
    }
    let operands = |arguments: &[ParamArg]| {
        arguments
            .iter()
            .map(|argument| match argument {
                ParamArg::Value(values) => type_list_source_types(values, pack),
                ParamArg::Type(_) | ParamArg::Named { .. } => None,
            })
            .collect::<Option<Vec<_>>>()
    };
    match &operand.kind {
        ExprKind::TupleLit(elements) => elements
            .iter()
            .map(|element| match &element.kind {
                ExprKind::TypeValue(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect(),
        ExprKind::Member { object, field } if field == "values" => {
            type_list_source_types(object, pack)
        }
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } if method == "reverse" && args.is_empty() && kwargs.is_empty() => {
            type_list_source_types(object, pack).map(|types| types.into_iter().rev().collect())
        }
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } if args.is_empty()
            && kwargs.is_empty()
            && matches!(&callee.kind, ExprKind::Member { object, field }
                if field == "_concat"
                    && matches!(&object.kind, ExprKind::Identifier(name) if name == "TypeList")) =>
        {
            operands(param_args).map(|lists| lists.into_iter().flatten().collect())
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } if name == "TypeList" && args.is_empty() && kwargs.is_empty() => {
            operands(param_args).map(|lists| lists.into_iter().flatten().collect())
        }
        _ => None,
    }
}

/// Write the element types of each pack `pack` binds into a type-list
/// expression, in the pack's place.
///
/// This is the list a specialization leaves open over another pack
/// (`TypeList._concat[Self.Ts.values, OtherTs.values]()` in a struct clone).
pub(super) fn bind_type_list_packs(operand: &mut Expr, pack: &dyn Fn(&str) -> Option<Vec<Type>>) {
    if let Some(types) = pack_name(operand).and_then(pack) {
        let span = operand.span;
        operand.kind = ExprKind::TupleLit(
            types
                .into_iter()
                .map(|ty| Expr::new(ExprKind::TypeValue(ty), span))
                .collect(),
        );
        return;
    }
    let arguments = |arguments: &mut [ParamArg]| {
        for argument in arguments {
            if let ParamArg::Value(values) = argument {
                bind_type_list_packs(values, pack);
            }
        }
    };
    match &mut operand.kind {
        ExprKind::Member { object, .. } | ExprKind::MethodCall { object, .. } => {
            bind_type_list_packs(object, pack);
        }
        ExprKind::Invoke { param_args, .. } | ExprKind::Call { param_args, .. } => {
            arguments(param_args);
        }
        _ => {}
    }
}

/// A spread type argument expanded over the packs `pack` binds: its element
/// types when every pack in it is bound, and otherwise the spread with the
/// bound packs written out.
pub(super) fn expand_spread_argument(
    mut operand: Expr,
    span: Span,
    pack: &dyn Fn(&str) -> Option<Vec<Type>>,
) -> Vec<ParamArg> {
    if let Some(types) = type_list_source_types(&operand, pack) {
        return types.into_iter().map(ParamArg::Type).collect();
    }
    bind_type_list_packs(&mut operand, pack);
    vec![ParamArg::Value(Expr::new(
        ExprKind::Spread(Box::new(operand)),
        span,
    ))]
}

/// The concrete default construction a bound pack element's `Ts[i]()`
/// elaborates to, its nodes identified by identities derived from the
/// construction's own (`parent`), so every copy builds the same syntax.
pub(super) fn pack_element_default_construction(
    element: &Ty,
    span: Span,
    parent: mojito_common::token::SyntaxId,
) -> Option<Expr> {
    let mut construction = default_constructor_call(&source_type_from_ty(element)?, element, span)?;
    let mut identities = mojito_ast::visit::DerivedIdentities { parent, next: 0 };
    mojito_ast::visit::walk_expr_mut(&mut identities, &mut construction);
    construction.syntax_id = parent;
    Some(construction)
}

fn default_constructor_call(ty: &Type, semantic: &Ty, span: Span) -> Option<Expr> {
    // A SIMD element default-constructs to zero lanes: the checker accepts one
    // lane to splat, not a nullary construction, so spell the zero explicitly.
    if let (Ty::Simd { dtype, .. }, Type::Named(name, arguments)) = (semantic, ty) {
        let zero = match dtype.known() {
            Some(mojito_ast::ast::Dtype::Bool) => ExprKind::Bool(false),
            Some(dtype) if dtype.is_float() => ExprKind::Float(0.0.into()),
            // An integer literal splats into any numeric lane, a symbolic one
            // included.
            _ => ExprKind::Int(0.into()),
        };
        return Some(Expr::new(
            ExprKind::Call {
                name: name.clone(),
                param_args: arguments.clone(),
                args: vec![Expr::new(zero, span)],
                kwargs: Vec::new(),
            },
            span,
        ));
    }
    // A scalar element converts its zero literal explicitly, so the storage
    // is built at exactly the element types, with no literal left for the
    // store to materialize.
    let scalar = match ty {
        Type::Int => Some(("Int", ExprKind::Int(0.into()))),
        Type::UInt => Some(("UInt", ExprKind::Int(0.into()))),
        Type::Bool => Some(("Bool", ExprKind::Bool(false))),
        Type::Float64 => Some(("Float64", ExprKind::Float(0.0.into()))),
        _ => None,
    };
    if let Some((name, zero)) = scalar {
        return Some(Expr::new(
            ExprKind::Call {
                name: name.to_string(),
                param_args: Vec::new(),
                args: vec![Expr::new(zero, span)],
                kwargs: Vec::new(),
            },
            span,
        ));
    }
    let literal = match ty {
        Type::StringLiteral | Type::ClosedStringLiteral => Some(ExprKind::Str(String::new())),
        Type::None => Some(ExprKind::None),
        _ => None,
    };
    if let Some(kind) = literal {
        return Some(Expr::new(kind, span));
    }
    let (name, param_args) = match ty {
        Type::Named(name, arguments) => (
            name.clone(),
            // A specialized (mangled) name has its arguments baked in; an
            // open application (`Tuple[Int, Bool]`, `Optional[Int]`)
            // keeps them so the element constructs through its own
            // specialization.
            if name.contains('$') {
                Vec::new()
            } else {
                arguments.clone()
            },
        ),
        _ => return None,
    };
    Some(Expr::new(
        ExprKind::Call {
            name,
            param_args,
            args: Vec::new(),
            kwargs: Vec::new(),
        },
        span,
    ))
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
