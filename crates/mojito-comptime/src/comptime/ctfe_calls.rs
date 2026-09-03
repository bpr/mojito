//! VM-CTFE call collection over types, expressions, statements, and
//! blocks.

use super::*;

/// Collect bare free-function callees from an expression. This is a declaration
/// dependency walk, not a purity classifier: it traverses every child so the
/// checked VM-CTFE subprogram retains helpers mentioned anywhere in a retained
/// function or nominal method body.
pub(super) fn collect_vm_ctfe_type_calls(ty: &Type, calls: &mut HashSet<String>) {
    let argument = |argument: &ParamArg, calls: &mut HashSet<String>| {
        fn collect(argument: &ParamArg, calls: &mut HashSet<String>) {
            match argument {
                ParamArg::Type(ty) => collect_vm_ctfe_type_calls(ty, calls),
                ParamArg::Value(value) => collect_vm_ctfe_expr_calls(value, calls),
                ParamArg::Named { value, .. } => collect(value, calls),
            }
        }
        collect(argument, calls);
    };
    let type_parameter = |parameter: &TypeParam, calls: &mut HashSet<String>| {
        if let Some(value_type) = &parameter.value_type {
            collect_vm_ctfe_type_calls(value_type, calls);
        }
        if let Some(callable) = &parameter.callable_bound {
            collect_vm_ctfe_type_calls(callable, calls);
        }
        if let Some(mutability) = &parameter.origin_mutability {
            collect_vm_ctfe_expr_calls(mutability, calls);
        }
        if let Some(default) = &parameter.default {
            collect_vm_ctfe_expr_calls(default, calls);
        }
        for constraint in &parameter.constraints {
            collect_vm_ctfe_expr_calls(constraint, calls);
        }
    };

    match ty {
        Type::Named(_, arguments) => {
            for value in arguments {
                argument(value, calls);
            }
        }
        Type::Assoc { base, args, .. } => {
            collect_vm_ctfe_type_calls(base, calls);
            for value in args {
                argument(value, calls);
            }
        }
        Type::IndexedProjection { base, index } => {
            collect_vm_ctfe_type_calls(base, calls);
            collect_vm_ctfe_expr_calls(index, calls);
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
            for parameter in type_params {
                type_parameter(parameter, calls);
            }
            for parameter in params {
                collect_vm_ctfe_type_calls(&parameter.ty, calls);
                for origin in parameter.origin.iter().flatten() {
                    collect_vm_ctfe_expr_calls(origin, calls);
                }
            }
            collect_vm_ctfe_type_calls(ret, calls);
            for origin in capturing.iter().flatten() {
                collect_vm_ctfe_expr_calls(origin, calls);
            }
            if let Some(error) = raises_type {
                collect_vm_ctfe_type_calls(error, calls);
            }
            for clause in where_clauses {
                collect_vm_ctfe_expr_calls(clause, calls);
            }
        }
        Type::Ref { referent, origin } => {
            collect_vm_ctfe_type_calls(referent, calls);
            for value in origin.iter().flatten() {
                collect_vm_ctfe_expr_calls(value, calls);
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

pub(super) fn collect_vm_ctfe_expr_calls(expression: &Expr, calls: &mut HashSet<String>) {
    let param_args = |arguments: &[ParamArg], calls: &mut HashSet<String>| {
        fn collect(argument: &ParamArg, calls: &mut HashSet<String>) {
            match argument {
                ParamArg::Type(ty) => collect_vm_ctfe_type_calls(ty, calls),
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
                    mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                        collect_vm_ctfe_expr_calls(iter, calls)
                    }
                    mojito_ast::ast::ComprehensionClause::If(condition) => {
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
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        collect_vm_ctfe_expr_calls(value, calls)
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
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
                if let mojito_ast::ast::TStringPart::Expr(value) = part {
                    collect_vm_ctfe_expr_calls(value, calls);
                }
            }
        }
        // A lambda's hidden definition is not CTFE-evaluated in place; its
        // body's calls are not comptime-needed here.
        ExprKind::Lambda { .. } => {}
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Uninitialized
        | ExprKind::EmptySubscript
        | ExprKind::Identifier(_) => {}
        ExprKind::TypeValue(ty) => collect_vm_ctfe_type_calls(ty, calls),
    }
}

pub(super) fn collect_vm_ctfe_block_calls(statements: &[Stmt], calls: &mut HashSet<String>) {
    for statement in statements {
        collect_vm_ctfe_stmt_calls(statement, calls);
    }
}

pub(super) fn collect_vm_ctfe_stmt_calls(statement: &Stmt, calls: &mut HashSet<String>) {
    let decorators = |decorators: &[mojito_ast::ast::Decorator], calls: &mut HashSet<String>| {
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
            collect_vm_ctfe_type_calls(&parameter.ty, calls);
            for origin in parameter.origin.iter().flatten() {
                collect_vm_ctfe_expr_calls(origin, calls);
            }
            if let Some(default) = &parameter.default {
                collect_vm_ctfe_expr_calls(default, calls);
            }
        }
    };
    let type_parameters = |parameters: &[TypeParam], calls: &mut HashSet<String>| {
        for parameter in parameters {
            if let Some(value_type) = &parameter.value_type {
                collect_vm_ctfe_type_calls(value_type, calls);
            }
            if let Some(callable) = &parameter.callable_bound {
                collect_vm_ctfe_type_calls(callable, calls);
            }
            if let Some(mutability) = &parameter.origin_mutability {
                collect_vm_ctfe_expr_calls(mutability, calls);
            }
            if let Some(default) = &parameter.default {
                collect_vm_ctfe_expr_calls(default, calls);
            }
            for constraint in &parameter.constraints {
                collect_vm_ctfe_expr_calls(constraint, calls);
            }
        }
    };

    match &statement.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                collect_vm_ctfe_type_calls(ty, calls);
            }
            collect_vm_ctfe_expr_calls(value, calls);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value))
        | StmtKind::Expr(value) => collect_vm_ctfe_expr_calls(value, calls),
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            type_parameters(type_params, calls);
            if let Some(ty) = ty {
                collect_vm_ctfe_type_calls(ty, calls);
            }
            for condition in where_clauses {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            collect_vm_ctfe_expr_calls(value, calls);
        }
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            collect_vm_ctfe_expr_calls(place, calls);
            collect_vm_ctfe_expr_calls(value, calls);
        }
        StmtKind::Unpack { targets, value, .. } => {
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
            type_params,
            params,
            raises_type,
            ret,
            where_clauses,
            body,
            ..
        } => {
            decorators(declaration_decorators, calls);
            type_parameters(type_params, calls);
            parameters(params, calls);
            if let Some(error) = raises_type {
                collect_vm_ctfe_type_calls(error, calls);
            }
            if let Some(ret) = ret {
                collect_vm_ctfe_type_calls(ret, calls);
            }
            for condition in where_clauses {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            collect_vm_ctfe_block_calls(body, calls);
        }
        StmtKind::Struct {
            decorators: declaration_decorators,
            type_params,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            ..
        } => {
            decorators(declaration_decorators, calls);
            type_parameters(type_params, calls);
            if let Some(callable) = callable_conformance {
                collect_vm_ctfe_type_calls(callable, calls);
            }
            for (_, condition) in conformance_conditions {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            for condition in where_clauses {
                collect_vm_ctfe_expr_calls(condition, calls);
            }
            for field in fields {
                collect_vm_ctfe_type_calls(&field.ty, calls);
            }
            for member in associated {
                type_parameters(&member.params, calls);
                if let Some(ty) = &member.ty {
                    collect_vm_ctfe_type_calls(ty, calls);
                }
                for condition in &member.where_clauses {
                    collect_vm_ctfe_expr_calls(condition, calls);
                }
                collect_vm_ctfe_expr_calls(&member.value, calls);
            }
            for method in methods {
                decorators(&method.decorators, calls);
                type_parameters(&method.type_params, calls);
                for origin in method.self_origin.iter().flatten() {
                    collect_vm_ctfe_expr_calls(origin, calls);
                }
                parameters(&method.params, calls);
                if let Some(error) = &method.raises_type {
                    collect_vm_ctfe_type_calls(error, calls);
                }
                if let Some(ret) = &method.ret {
                    collect_vm_ctfe_type_calls(ret, calls);
                }
                for condition in &method.where_clauses {
                    collect_vm_ctfe_expr_calls(condition, calls);
                }
                collect_vm_ctfe_block_calls(&method.body, calls);
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            for method in methods {
                type_parameters(&method.type_params, calls);
                for origin in method.self_origin.iter().flatten() {
                    collect_vm_ctfe_expr_calls(origin, calls);
                }
                parameters(&method.params, calls);
                if let Some(error) = &method.raises_type {
                    collect_vm_ctfe_type_calls(error, calls);
                }
                if let Some(ret) = &method.ret {
                    collect_vm_ctfe_type_calls(ret, calls);
                }
                for condition in &method.where_clauses {
                    collect_vm_ctfe_expr_calls(condition, calls);
                }
                if let Some(body) = &method.default_body {
                    collect_vm_ctfe_block_calls(body, calls);
                }
            }
            for member in comptime_members {
                type_parameters(&member.params, calls);
                collect_vm_ctfe_type_calls(&member.ty, calls);
                for condition in &member.where_clauses {
                    collect_vm_ctfe_expr_calls(condition, calls);
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
