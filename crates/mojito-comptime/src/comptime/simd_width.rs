//! Layout-dependent parameter detection: SIMD-width usage scans over
//! types, statements, and expressions.

use super::*;

/// Whether a generic `def` uses one of its parameters where checking or
/// execution requires a concrete target layout: as a `SIMD`/`Scalar` width or
/// as the operand of `size_of`. Such a declaration must specialize per call.
pub(super) fn def_uses_layout_dependent_param(statement: &Stmt) -> bool {
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

pub(super) fn type_uses_param_simd_width(ty: &Type, names: &[&str]) -> bool {
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

pub(super) fn param_arg_uses_simd_width(
    width_position: bool,
    argument: &ParamArg,
    names: &[&str],
) -> bool {
    match argument {
        ParamArg::Type(inner) => type_uses_param_simd_width(inner, names),
        ParamArg::Value(value) => {
            width_position
                && matches!(&value.kind, ExprKind::Identifier(name) if names.contains(&name.as_str()))
        }
        ParamArg::Named { value, .. } => param_arg_uses_simd_width(width_position, value, names),
    }
}

pub(super) fn stmt_uses_param_simd_width(statement: &Stmt, names: &[&str]) -> bool {
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

pub(super) fn expr_uses_param_simd_width(e: &Expr, names: &[&str]) -> bool {
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
                || (name == "size_of"
                    && param_args.iter().any(|argument| {
                        matches!(argument,
                            ParamArg::Type(Type::Named(parameter, arguments))
                                if arguments.is_empty() && names.contains(&parameter.as_str()))
                    }))
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
