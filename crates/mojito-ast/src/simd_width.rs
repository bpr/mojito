//! Layout-dependent parameter detection: SIMD-width usage scans over
//! types, statements, and expressions. The elaborator specializes such a
//! declaration per call, and source validation checks its template.

use crate::ast::{Expr, ExprKind, Method, ParamArg, Stmt, StmtKind, Type, TypeParam};

/// Whether a generic `def` uses one of its parameters as a lane width.
///
/// That is a use where checking or execution requires a concrete target
/// layout: as a `SIMD`/`Scalar` width or as the operand of `size_of`. Such a
/// declaration must specialize per call.
pub fn def_uses_layout_dependent_param(statement: &Stmt) -> bool {
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

/// Whether a generic struct uses one of its own parameters as a lane width.
///
/// That is a `SIMD`/`Scalar` width (`SIMD[DType.int64, Self.length]`) in a
/// field, a method signature, or a method body. An erased body has no lane
/// count for it, so such a struct specializes per application.
pub fn struct_uses_layout_dependent_param(statement: &Stmt) -> bool {
    let StmtKind::Struct {
        type_params,
        fields,
        methods,
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
    fields
        .iter()
        .any(|field| type_uses_param_simd_width(&field.ty, &names))
        || methods.iter().any(|method| {
            method
                .params
                .iter()
                .any(|parameter| type_uses_param_simd_width(&parameter.ty, &names))
                || method
                    .ret
                    .as_ref()
                    .is_some_and(|ty| type_uses_param_simd_width(ty, &names))
                || method
                    .body
                    .iter()
                    .any(|inner| stmt_uses_param_simd_width(inner, &names))
        })
}

/// Whether a struct method's template body constructs a vector at a lane its
/// own parameters spell (`Scalar[dt](x)`, `SIMD[DType.int32, w](v)`).
///
/// Such a construction checks, but lowers only once the lane is bound: every call
/// retargets to a per-call clone and the template body is a trap stub, as a
/// free `def`'s template never reaches the program.
pub fn method_constructs_at_own_lane(method: &Method) -> bool {
    struct Finder<'a> {
        own: &'a [TypeParam],
        found: bool,
    }

    struct Names<'a> {
        own: &'a [TypeParam],
        found: bool,
    }

    impl crate::visit::Visitor for Names<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::Identifier(name) = &expr.kind {
                self.found |= self.own.iter().any(|parameter| &parameter.name == name);
            }
        }

        fn visit_type(&mut self, ty: &Type) {
            if let Type::Named(name, arguments) = ty
                && arguments.is_empty()
            {
                self.found |= self.own.iter().any(|parameter| &parameter.name == name);
            }
        }
    }

    impl crate::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            let ExprKind::Call {
                name, param_args, ..
            } = &expr.kind
            else {
                return;
            };
            if name != "SIMD" && name != "Scalar" {
                return;
            }
            let mut names = Names {
                own: self.own,
                found: false,
            };
            for argument in param_args {
                crate::visit::walk_param_arg(&mut names, argument);
            }
            self.found |= names.found;
        }
    }

    let mut finder = Finder {
        own: &method.type_params,
        found: false,
    };
    crate::visit::walk_block(&mut finder, &method.body);
    finder.found
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
        // A struct's own parameter in a width slot is spelled `Self.<name>`.
        ParamArg::Type(Type::SelfParam(name)) => width_position && names.contains(&name.as_str()),
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
