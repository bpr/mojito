//! Read-only traversal of the source AST.
//!
//! The `walk_*` functions visit every child of a node in source order and
//! report each expression and type to a [`Visitor`] before descending into
//! it. A construct that binds names — a `def`, method, lambda, or callable
//! type with its own parameters, a struct or generic alias with parameters, a
//! loop, comprehension, `except`, or `with` binder — asks
//! [`Visitor::enter_scope`] first, so a visitor tracking one name can stop at
//! the scope that shadows it.

use crate::ast::{
    ComprehensionClause, Decorator, Expr, ExprKind, FnParam, FunctionTypeParam, Method, Param,
    ParamArg, Stmt, StmtKind, StructComptime, SubscriptArg, TStringPart, TraitComptime,
    TraitMethod, Type, TypeParam,
};

/// Callbacks for a read-only walk. Every method has a no-op default.
pub trait Visitor {
    /// An expression, reported before its children.
    fn visit_expr(&mut self, _expr: &Expr) {}

    /// A type annotation, reported before its children.
    fn visit_type(&mut self, _ty: &Type) {}

    /// A scope that binds `names` is about to be walked (parameter names keep
    /// a pack's leading `*`). Returning `false` skips everything the scope
    /// covers.
    fn enter_scope(&mut self, _names: &[&str]) -> bool {
        true
    }
}

pub fn walk_block<V: Visitor>(visitor: &mut V, statements: &[Stmt]) {
    for statement in statements {
        walk_stmt(visitor, statement);
    }
}

pub fn walk_stmt<V: Visitor>(visitor: &mut V, statement: &Stmt) {
    match &statement.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                walk_type(visitor, ty);
            }
            walk_expr(visitor, value);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value))
        | StmtKind::Expr(value) => walk_expr(visitor, value),
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            walk_expr(visitor, place);
            walk_expr(visitor, value);
        }
        StmtKind::Unpack { targets, value, .. } => {
            walk_exprs(visitor, targets);
            walk_expr(visitor, value);
        }
        StmtKind::Def {
            decorators,
            type_params,
            params,
            raises_type,
            ret,
            where_clauses,
            body,
            ..
        } => {
            walk_decorators(visitor, decorators);
            let names = type_params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .chain(params.iter().map(|parameter| parameter.name.as_str()))
                .collect::<Vec<_>>();
            if !visitor.enter_scope(&names) {
                return;
            }
            walk_type_params(visitor, type_params);
            walk_fn_params(visitor, params);
            walk_optional_type(visitor, raises_type.as_ref());
            walk_optional_type(visitor, ret.as_ref());
            walk_exprs(visitor, where_clauses);
            walk_block(visitor, body);
        }
        StmtKind::Struct {
            decorators,
            type_params,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            ..
        } => {
            walk_decorators(visitor, decorators);
            let names = type_params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>();
            if !visitor.enter_scope(&names) {
                return;
            }
            walk_type_params(visitor, type_params);
            walk_optional_type(visitor, callable_conformance.as_ref());
            for (_, condition) in conformance_conditions {
                walk_expr(visitor, condition);
            }
            walk_exprs(visitor, where_clauses);
            for field in fields {
                walk_field(visitor, field);
            }
            for member in associated {
                walk_struct_comptime(visitor, member);
            }
            for method in methods {
                walk_method(visitor, method);
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            for method in methods {
                walk_trait_method(visitor, method);
            }
            for member in comptime_members {
                walk_trait_comptime(visitor, member);
            }
        }
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            let names = type_params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>();
            if !visitor.enter_scope(&names) {
                return;
            }
            walk_type_params(visitor, type_params);
            walk_optional_type(visitor, ty.as_ref());
            walk_exprs(visitor, where_clauses);
            walk_expr(visitor, value);
        }
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (condition, body) in branches {
                walk_expr(visitor, condition);
                walk_block(visitor, body);
            }
            walk_optional_block(visitor, orelse.as_deref());
        }
        StmtKind::While { cond, body, orelse } => {
            walk_expr(visitor, cond);
            walk_block(visitor, body);
            walk_optional_block(visitor, orelse.as_deref());
        }
        StmtKind::For {
            var,
            iter,
            body,
            orelse,
            ..
        } => {
            walk_expr(visitor, iter);
            if visitor.enter_scope(&[var.as_str()]) {
                walk_block(visitor, body);
            }
            walk_optional_block(visitor, orelse.as_deref());
        }
        StmtKind::ComptimeFor { var, iter, body } => {
            walk_expr(visitor, iter);
            if visitor.enter_scope(&[var.as_str()]) {
                walk_block(visitor, body);
            }
        }
        StmtKind::With { items, body } => {
            for item in items {
                walk_expr(visitor, &item.context);
            }
            let names = items
                .iter()
                .filter_map(|item| item.var.as_deref())
                .collect::<Vec<_>>();
            if visitor.enter_scope(&names) {
                walk_block(visitor, body);
            }
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            walk_block(visitor, body);
            if let Some((binder, body)) = except {
                let names = binder.iter().map(String::as_str).collect::<Vec<_>>();
                if visitor.enter_scope(&names) {
                    walk_block(visitor, body);
                }
            }
            walk_optional_block(visitor, orelse.as_deref());
            walk_optional_block(visitor, finalbody.as_deref());
        }
        StmtKind::Return(None)
        | StmtKind::Import { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue => {}
    }
}

pub fn walk_expr<V: Visitor>(visitor: &mut V, expression: &Expr) {
    visitor.visit_expr(expression);
    match &expression.kind {
        ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
            walk_expr(visitor, value);
        }
        ExprKind::Infix(_, left, right)
        | ExprKind::Index {
            object: left,
            index: right,
        } => {
            walk_expr(visitor, left);
            walk_expr(visitor, right);
        }
        ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } => {
            walk_param_args(visitor, param_args);
            walk_exprs(visitor, args);
            for argument in kwargs {
                walk_expr(visitor, &argument.value);
            }
        }
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            walk_expr(visitor, callee);
            walk_param_args(visitor, param_args);
            walk_exprs(visitor, args);
            for argument in kwargs {
                walk_expr(visitor, &argument.value);
            }
        }
        ExprKind::Member { object, .. } => walk_expr(visitor, object),
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            walk_expr(visitor, object);
            walk_exprs(visitor, args);
            for argument in kwargs {
                walk_expr(visitor, &argument.value);
            }
        }
        ExprKind::TypeApply { args, .. } => walk_param_args(visitor, args),
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => walk_exprs(visitor, values),
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                walk_expr(visitor, key);
                if let Some(value) = value {
                    walk_expr(visitor, value);
                }
            }
        }
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            for clause in clauses {
                match clause {
                    ComprehensionClause::For { var, iter, .. } => {
                        walk_expr(visitor, iter);
                        if !visitor.enter_scope(&[var.as_str()]) {
                            return;
                        }
                    }
                    ComprehensionClause::If(condition) => walk_expr(visitor, condition),
                }
            }
            if let Some(key) = key {
                walk_expr(visitor, key);
            }
            walk_expr(visitor, value);
        }
        ExprKind::Lambda { def } => walk_stmt(visitor, def),
        ExprKind::Named { value, .. } => walk_expr(visitor, value),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr(visitor, cond);
            walk_expr(visitor, then_branch);
            walk_expr(visitor, else_branch);
        }
        ExprKind::Compare { first, rest } => {
            walk_expr(visitor, first);
            for (_, value) in rest {
                walk_expr(visitor, value);
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            walk_expr(visitor, object);
            for value in [lower, upper, step].into_iter().flatten() {
                walk_expr(visitor, value);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            walk_expr(visitor, object);
            for argument in args {
                match argument {
                    SubscriptArg::Index(value) | SubscriptArg::Keyword { value, .. } => {
                        walk_expr(visitor, value);
                    }
                    SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            walk_expr(visitor, value);
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    walk_expr(visitor, value);
                }
            }
        }
        ExprKind::TypeValue(ty) => walk_type(visitor, ty),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Uninitialized
        | ExprKind::EmptySubscript
        | ExprKind::Identifier(_) => {}
    }
}

pub fn walk_type<V: Visitor>(visitor: &mut V, ty: &Type) {
    visitor.visit_type(ty);
    match ty {
        Type::Named(_, arguments) => walk_param_args(visitor, arguments),
        Type::Assoc { base, args, .. } => {
            walk_type(visitor, base);
            walk_param_args(visitor, args);
        }
        Type::IndexedProjection { base, index } => {
            walk_type(visitor, base);
            walk_expr(visitor, index);
        }
        Type::Func {
            type_params,
            params,
            ret,
            capturing,
            raises_type,
            where_clauses,
            ..
        } => {
            let names = type_params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .chain(
                    params
                        .iter()
                        .filter_map(|parameter| parameter.name.as_deref()),
                )
                .collect::<Vec<_>>();
            if !visitor.enter_scope(&names) {
                return;
            }
            walk_type_params(visitor, type_params);
            for parameter in params {
                walk_function_type_param(visitor, parameter);
            }
            walk_type(visitor, ret);
            walk_exprs(visitor, capturing.as_deref().unwrap_or_default());
            walk_optional_type(visitor, raises_type.as_deref());
            walk_exprs(visitor, where_clauses);
        }
        Type::Ref { referent, origin } => {
            walk_type(visitor, referent);
            walk_exprs(visitor, origin.as_deref().unwrap_or_default());
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

pub fn walk_param_arg<V: Visitor>(visitor: &mut V, argument: &ParamArg) {
    match argument {
        ParamArg::Type(ty) => walk_type(visitor, ty),
        ParamArg::Value(value) => walk_expr(visitor, value),
        ParamArg::Named { value, .. } => walk_param_arg(visitor, value),
    }
}

pub fn walk_type_param<V: Visitor>(visitor: &mut V, parameter: &TypeParam) {
    walk_optional_type(visitor, parameter.value_type.as_ref());
    walk_optional_type(visitor, parameter.callable_bound.as_ref());
    if let Some(mutability) = &parameter.origin_mutability {
        walk_expr(visitor, mutability);
    }
    if let Some(default) = &parameter.default {
        walk_expr(visitor, default);
    }
    walk_exprs(visitor, &parameter.constraints);
}

pub fn walk_fn_param<V: Visitor>(visitor: &mut V, parameter: &FnParam) {
    walk_type(visitor, &parameter.ty);
    walk_exprs(visitor, parameter.origin.as_deref().unwrap_or_default());
    if let Some(default) = &parameter.default {
        walk_expr(visitor, default);
    }
}

pub fn walk_decorators<V: Visitor>(visitor: &mut V, decorators: &[Decorator]) {
    for decorator in decorators {
        walk_exprs(visitor, &decorator.args);
        for argument in &decorator.kwargs {
            walk_expr(visitor, &argument.value);
        }
    }
}

/// A struct field declaration.
pub fn walk_field<V: Visitor>(visitor: &mut V, field: &Param) {
    walk_type(visitor, &field.ty);
}

/// A struct's `comptime` member, inside the scope of its own parameters.
pub fn walk_struct_comptime<V: Visitor>(visitor: &mut V, member: &StructComptime) {
    let names = member
        .params
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    if !visitor.enter_scope(&names) {
        return;
    }
    walk_type_params(visitor, &member.params);
    walk_optional_type(visitor, member.ty.as_ref());
    walk_exprs(visitor, &member.where_clauses);
    walk_expr(visitor, &member.value);
}

/// A struct method: its decorators, then everything inside the scope of its
/// own parameters.
pub fn walk_method<V: Visitor>(visitor: &mut V, method: &Method) {
    walk_decorators(visitor, &method.decorators);
    let names = method
        .type_params
        .iter()
        .map(|parameter| parameter.name.as_str())
        .chain(
            method
                .params
                .iter()
                .map(|parameter| parameter.name.as_str()),
        )
        .collect::<Vec<_>>();
    if !visitor.enter_scope(&names) {
        return;
    }
    walk_type_params(visitor, &method.type_params);
    walk_exprs(visitor, method.self_origin.as_deref().unwrap_or_default());
    walk_fn_params(visitor, &method.params);
    walk_optional_type(visitor, method.raises_type.as_ref());
    walk_optional_type(visitor, method.ret.as_ref());
    walk_optional_type(visitor, method.self_ty.as_ref());
    walk_exprs(visitor, &method.where_clauses);
    walk_block(visitor, &method.body);
}

pub fn walk_trait_method<V: Visitor>(visitor: &mut V, method: &TraitMethod) {
    let names = method
        .type_params
        .iter()
        .map(|parameter| parameter.name.as_str())
        .chain(
            method
                .params
                .iter()
                .map(|parameter| parameter.name.as_str()),
        )
        .collect::<Vec<_>>();
    if !visitor.enter_scope(&names) {
        return;
    }
    walk_type_params(visitor, &method.type_params);
    walk_exprs(visitor, method.self_origin.as_deref().unwrap_or_default());
    walk_fn_params(visitor, &method.params);
    walk_optional_type(visitor, method.raises_type.as_ref());
    walk_optional_type(visitor, method.ret.as_ref());
    walk_exprs(visitor, &method.where_clauses);
    walk_optional_block(visitor, method.default_body.as_deref());
}

pub fn walk_trait_comptime<V: Visitor>(visitor: &mut V, member: &TraitComptime) {
    let names = member
        .params
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect::<Vec<_>>();
    if !visitor.enter_scope(&names) {
        return;
    }
    walk_type_params(visitor, &member.params);
    walk_type(visitor, &member.ty);
    walk_exprs(visitor, &member.where_clauses);
}

fn walk_exprs<V: Visitor>(visitor: &mut V, expressions: &[Expr]) {
    for expression in expressions {
        walk_expr(visitor, expression);
    }
}

fn walk_param_args<V: Visitor>(visitor: &mut V, arguments: &[ParamArg]) {
    for argument in arguments {
        walk_param_arg(visitor, argument);
    }
}

fn walk_type_params<V: Visitor>(visitor: &mut V, parameters: &[TypeParam]) {
    for parameter in parameters {
        walk_type_param(visitor, parameter);
    }
}

fn walk_fn_params<V: Visitor>(visitor: &mut V, parameters: &[FnParam]) {
    for parameter in parameters {
        walk_fn_param(visitor, parameter);
    }
}

fn walk_function_type_param<V: Visitor>(visitor: &mut V, parameter: &FunctionTypeParam) {
    walk_type(visitor, &parameter.ty);
    walk_exprs(visitor, parameter.origin.as_deref().unwrap_or_default());
}

fn walk_optional_type<V: Visitor>(visitor: &mut V, ty: Option<&Type>) {
    if let Some(ty) = ty {
        walk_type(visitor, ty);
    }
}

fn walk_optional_block<V: Visitor>(visitor: &mut V, statements: Option<&[Stmt]>) {
    if let Some(statements) = statements {
        walk_block(visitor, statements);
    }
}

/// Callbacks for an in-place walk. Every method has a no-op default.
///
/// A statement is reported before its children; an expression after its
/// children, so a visitor may replace a node once its subtree has been
/// visited (erasing a wrapper call keeps the operand's own provenance).
pub trait MutVisitor {
    /// A statement, reported before its children.
    fn visit_stmt_mut(&mut self, _statement: &mut Stmt) {}

    /// An expression, reported after its children.
    fn visit_expr_mut(&mut self, _expr: &mut Expr) {}
}

/// Walk every statement, expression, and annotation of a block in place.
pub fn walk_block_mut<V: MutVisitor>(visitor: &mut V, statements: &mut [Stmt]) {
    for statement in statements {
        walk_stmt_mut(visitor, statement);
    }
}

/// Walk one statement in place, its nested blocks, annotations, and
/// declarations included.
pub fn walk_stmt_mut<V: MutVisitor>(visitor: &mut V, statement: &mut Stmt) {
    visitor.visit_stmt_mut(statement);
    match &mut statement.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                walk_type_mut(visitor, ty);
            }
            walk_expr_mut(visitor, value);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value))
        | StmtKind::Expr(value) => walk_expr_mut(visitor, value),
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            walk_expr_mut(visitor, place);
            walk_expr_mut(visitor, value);
        }
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            walk_type_params_mut(visitor, type_params);
            if let Some(ty) = ty {
                walk_type_mut(visitor, ty);
            }
            for condition in where_clauses {
                walk_expr_mut(visitor, condition);
            }
            walk_expr_mut(visitor, value);
        }
        StmtKind::Unpack { targets, value, .. } => {
            for target in targets {
                walk_expr_mut(visitor, target);
            }
            walk_expr_mut(visitor, value);
        }
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (condition, block) in branches {
                walk_expr_mut(visitor, condition);
                walk_block_mut(visitor, block);
            }
            if let Some(block) = orelse {
                walk_block_mut(visitor, block);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            walk_expr_mut(visitor, cond);
            walk_block_mut(visitor, body);
            if let Some(body) = orelse {
                walk_block_mut(visitor, body);
            }
        }
        StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
            walk_expr_mut(visitor, iter);
            walk_block_mut(visitor, body);
        }
        StmtKind::With { items, body } => {
            for item in items {
                walk_expr_mut(visitor, &mut item.context);
            }
            walk_block_mut(visitor, body);
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            walk_block_mut(visitor, body);
            if let Some((_, block)) = except {
                walk_block_mut(visitor, block);
            }
            if let Some(block) = orelse {
                walk_block_mut(visitor, block);
            }
            if let Some(block) = finalbody {
                walk_block_mut(visitor, block);
            }
        }
        StmtKind::Def {
            type_params,
            params,
            raises_type,
            ret,
            where_clauses,
            body,
            decorators,
            ..
        } => {
            walk_type_params_mut(visitor, type_params);
            for param in params {
                walk_fn_param_mut(visitor, param);
            }
            if let Some(error) = raises_type {
                walk_type_mut(visitor, error);
            }
            if let Some(ret) = ret {
                walk_type_mut(visitor, ret);
            }
            for condition in where_clauses {
                walk_expr_mut(visitor, condition);
            }
            walk_decorators_mut(visitor, decorators);
            walk_block_mut(visitor, body);
        }
        StmtKind::Struct {
            type_params,
            callable_conformance,
            conformance_conditions,
            fields,
            associated,
            methods,
            decorators,
            where_clauses,
            ..
        } => {
            walk_type_params_mut(visitor, type_params);
            if let Some(callable) = callable_conformance {
                walk_type_mut(visitor, callable);
            }
            for condition in where_clauses {
                walk_expr_mut(visitor, condition);
            }
            for (_, condition) in conformance_conditions {
                walk_expr_mut(visitor, condition);
            }
            for field in fields {
                walk_type_mut(visitor, &mut field.ty);
            }
            for member in associated {
                walk_type_params_mut(visitor, &mut member.params);
                if let Some(ty) = &mut member.ty {
                    walk_type_mut(visitor, ty);
                }
                for condition in &mut member.where_clauses {
                    walk_expr_mut(visitor, condition);
                }
                walk_expr_mut(visitor, &mut member.value);
            }
            walk_decorators_mut(visitor, decorators);
            for method in methods {
                walk_type_params_mut(visitor, &mut method.type_params);
                if let Some(origins) = &mut method.self_origin {
                    for origin in origins {
                        walk_expr_mut(visitor, origin);
                    }
                }
                for param in &mut method.params {
                    walk_fn_param_mut(visitor, param);
                }
                if let Some(error) = &mut method.raises_type {
                    walk_type_mut(visitor, error);
                }
                if let Some(ret) = &mut method.ret {
                    walk_type_mut(visitor, ret);
                }
                for condition in &mut method.where_clauses {
                    walk_expr_mut(visitor, condition);
                }
                walk_decorators_mut(visitor, &mut method.decorators);
                walk_block_mut(visitor, &mut method.body);
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            for method in methods {
                walk_type_params_mut(visitor, &mut method.type_params);
                if let Some(origins) = &mut method.self_origin {
                    for origin in origins {
                        walk_expr_mut(visitor, origin);
                    }
                }
                for param in &mut method.params {
                    walk_fn_param_mut(visitor, param);
                }
                if let Some(error) = &mut method.raises_type {
                    walk_type_mut(visitor, error);
                }
                if let Some(ret) = &mut method.ret {
                    walk_type_mut(visitor, ret);
                }
                for condition in &mut method.where_clauses {
                    walk_expr_mut(visitor, condition);
                }
                if let Some(body) = &mut method.default_body {
                    walk_block_mut(visitor, body);
                }
            }
            for member in comptime_members {
                walk_type_params_mut(visitor, &mut member.params);
                walk_type_mut(visitor, &mut member.ty);
                for condition in &mut member.where_clauses {
                    walk_expr_mut(visitor, condition);
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

/// Walk one expression in place, children first.
pub fn walk_expr_mut<V: MutVisitor>(visitor: &mut V, expr: &mut Expr) {
    match &mut expr.kind {
        ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
            walk_expr_mut(visitor, value);
        }
        ExprKind::Infix(_, left, right)
        | ExprKind::Index {
            object: left,
            index: right,
        } => {
            walk_expr_mut(visitor, left);
            walk_expr_mut(visitor, right);
        }
        ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } => {
            walk_param_args_mut(visitor, param_args);
            for arg in args {
                walk_expr_mut(visitor, arg);
            }
            for arg in kwargs {
                walk_expr_mut(visitor, &mut arg.value);
            }
        }
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            walk_expr_mut(visitor, callee);
            walk_param_args_mut(visitor, param_args);
            for arg in args {
                walk_expr_mut(visitor, arg);
            }
            for arg in kwargs {
                walk_expr_mut(visitor, &mut arg.value);
            }
        }
        ExprKind::Member { object, .. } => walk_expr_mut(visitor, object),
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            walk_expr_mut(visitor, object);
            for arg in args {
                walk_expr_mut(visitor, arg);
            }
            for arg in kwargs {
                walk_expr_mut(visitor, &mut arg.value);
            }
        }
        ExprKind::TypeApply { args, .. } => walk_param_args_mut(visitor, args),
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                walk_expr_mut(visitor, value);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                walk_expr_mut(visitor, key);
                if let Some(value) = value {
                    walk_expr_mut(visitor, value);
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
                walk_expr_mut(visitor, key);
            }
            walk_expr_mut(visitor, value);
            for clause in clauses {
                match clause {
                    ComprehensionClause::For { iter, .. } => walk_expr_mut(visitor, iter),
                    ComprehensionClause::If(condition) => walk_expr_mut(visitor, condition),
                }
            }
        }
        ExprKind::Named { value, .. } => walk_expr_mut(visitor, value),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr_mut(visitor, cond);
            walk_expr_mut(visitor, then_branch);
            walk_expr_mut(visitor, else_branch);
        }
        ExprKind::Compare { first, rest } => {
            walk_expr_mut(visitor, first);
            for (_, value) in rest {
                walk_expr_mut(visitor, value);
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            walk_expr_mut(visitor, object);
            for value in [lower, upper, step].into_iter().flatten() {
                walk_expr_mut(visitor, value);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            walk_expr_mut(visitor, object);
            for argument in args {
                match argument {
                    SubscriptArg::Index(value) | SubscriptArg::Keyword { value, .. } => {
                        walk_expr_mut(visitor, value);
                    }
                    SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            walk_expr_mut(visitor, value);
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    walk_expr_mut(visitor, value);
                }
            }
        }
        ExprKind::TypeValue(ty) => walk_type_mut(visitor, ty),
        ExprKind::Lambda { def } => walk_stmt_mut(visitor, def),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Uninitialized
        | ExprKind::EmptySubscript
        | ExprKind::Identifier(_) => {}
    }
    visitor.visit_expr_mut(expr);
}

/// Walk a type annotation's embedded expressions and nested types in place.
pub fn walk_type_mut<V: MutVisitor>(visitor: &mut V, ty: &mut Type) {
    match ty {
        Type::Named(_, args) => walk_param_args_mut(visitor, args),
        Type::Assoc { base, args, .. } => {
            walk_type_mut(visitor, base);
            walk_param_args_mut(visitor, args);
        }
        Type::IndexedProjection { base, index } => {
            walk_type_mut(visitor, base);
            walk_expr_mut(visitor, index);
        }
        Type::Ref { referent, origin } => {
            walk_type_mut(visitor, referent);
            if let Some(origins) = origin {
                for expression in origins {
                    walk_expr_mut(visitor, expression);
                }
            }
        }
        Type::Func {
            type_params,
            params,
            ret,
            capturing,
            raises_type,
            where_clauses,
            ..
        } => {
            walk_type_params_mut(visitor, type_params);
            for param in params {
                walk_type_mut(visitor, &mut param.ty);
                if let Some(origins) = &mut param.origin {
                    for expression in origins {
                        walk_expr_mut(visitor, expression);
                    }
                }
            }
            walk_type_mut(visitor, ret);
            for clause in where_clauses {
                walk_expr_mut(visitor, clause);
            }
            if let Some(origins) = capturing {
                for expression in origins {
                    walk_expr_mut(visitor, expression);
                }
            }
            if let Some(error) = raises_type {
                walk_type_mut(visitor, error);
            }
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

fn walk_param_args_mut<V: MutVisitor>(visitor: &mut V, args: &mut [ParamArg]) {
    for arg in args {
        match arg {
            ParamArg::Type(ty) => walk_type_mut(visitor, ty),
            ParamArg::Value(value) => walk_expr_mut(visitor, value),
            ParamArg::Named { value, .. } => {
                walk_param_args_mut(visitor, std::slice::from_mut(value));
            }
        }
    }
}

fn walk_type_params_mut<V: MutVisitor>(visitor: &mut V, params: &mut [TypeParam]) {
    for param in params {
        if let Some(ty) = &mut param.value_type {
            walk_type_mut(visitor, ty);
        }
        if let Some(ty) = &mut param.callable_bound {
            walk_type_mut(visitor, ty);
        }
        if let Some(value) = &mut param.origin_mutability {
            walk_expr_mut(visitor, value);
        }
        if let Some(value) = &mut param.default {
            walk_expr_mut(visitor, value);
        }
        for condition in &mut param.constraints {
            walk_expr_mut(visitor, condition);
        }
    }
}

fn walk_fn_param_mut<V: MutVisitor>(visitor: &mut V, param: &mut FnParam) {
    walk_type_mut(visitor, &mut param.ty);
    if let Some(value) = &mut param.default {
        walk_expr_mut(visitor, value);
    }
}

fn walk_decorators_mut<V: MutVisitor>(visitor: &mut V, decorators: &mut [Decorator]) {
    for decorator in decorators {
        for arg in &mut decorator.args {
            walk_expr_mut(visitor, arg);
        }
        for arg in &mut decorator.kwargs {
            walk_expr_mut(visitor, &mut arg.value);
        }
    }
}
