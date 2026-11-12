//! AST rewriting helpers used by compile-time elaboration.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::FnParam;

// --- Type-binding substitution ----------------------------------------------
//
// Substitutes the *type* occurrences of dropped parameter bindings:
// annotations, compile-time argument lists, and constructor-call heads. It
// always descends into nested `def`/`struct` bodies — a dropped binding is
// in scope throughout the clone — shadowed only by a nested declaration's
// own type parameter of the same name.

/// A binding→concrete-source-type lookup for a dropped-parameter rewrite.
pub(super) type TypeSubs<'a> = &'a dyn Fn(&str) -> Option<Type>;

pub(super) fn substitute_type_bindings_in_block(body: &mut [Stmt], subs: TypeSubs) {
    for statement in body {
        retype_stmt(statement, subs);
    }
}

pub(super) fn substitute_type_bindings_in_type(ty: &mut Type, subs: TypeSubs) {
    substitute_source_type_bindings(ty, subs);
}

pub(super) fn substitute_type_bindings_in_expr(e: &mut Expr, subs: TypeSubs) {
    retype_expr(e, subs);
}

fn retype_condition(condition: &mut mojito_ast::ast::Condition, subs: TypeSubs) {
    if let Some(pattern) = condition.pattern_mut() {
        for value in pattern.expressions_mut() {
            retype_expr(value, subs);
        }
    }
    retype_expr(condition.value_mut(), subs);
}

fn retype_target(target: &mut mojito_ast::ast::DestructureTarget, subs: TypeSubs) {
    for place in target.places_mut() {
        retype_expr(place, subs);
    }
}

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    reason = "TODO: split this pass; TODO: split this pass"
)]
fn retype_type_parameter(parameter: &mut TypeParam, subs: TypeSubs) {
    if let Some(value_type) = &mut parameter.value_type {
        substitute_type_bindings_in_type(value_type, subs);
    }
    if let Some(callable) = &mut parameter.callable_bound {
        substitute_type_bindings_in_type(callable, subs);
    }
    if let Some(mutability) = &mut parameter.origin_mutability {
        retype_expr(mutability, subs);
    }
    if let Some(default) = &mut parameter.default {
        retype_expr(default, subs);
    }
}

fn retype_fn_parameter(parameter: &mut FnParam, subs: TypeSubs) {
    substitute_type_bindings_in_type(&mut parameter.ty, subs);
    if let Some(origins) = &mut parameter.origin {
        retype_exprs(origins, subs);
    }
    if let Some(default) = &mut parameter.default {
        retype_expr(default, subs);
    }
}

fn retype_decorators(decorators: &mut [mojito_ast::ast::Decorator], subs: TypeSubs) {
    for decorator in decorators {
        retype_exprs(&mut decorator.args, subs);
        for argument in &mut decorator.kwargs {
            retype_expr(&mut argument.value, subs);
        }
    }
}

fn retype_stmt(s: &mut Stmt, subs: TypeSubs) {
    match &mut s.kind {
        StmtKind::VarDecl { ty, value, .. } => {
            if let Some(ty) = ty {
                substitute_type_bindings_in_type(ty, subs);
            }
            retype_expr(value, subs);
        }
        StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value)) => retype_expr(value, subs),
        StmtKind::Comptime {
            type_params,
            ty,
            where_clauses,
            value,
            ..
        } => {
            let inner = without_shadowed(subs, type_params);
            for parameter in type_params {
                retype_type_parameter(parameter, &inner);
            }
            if let Some(ty) = ty {
                substitute_type_bindings_in_type(ty, &inner);
            }
            for condition in where_clauses {
                retype_expr(condition, &inner);
            }
            retype_expr(value, &inner);
        }
        StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            retype_expr(place, subs);
            retype_expr(value, subs);
        }
        StmtKind::Destructure { target, value } => {
            retype_target(target, subs);
            retype_expr(value, subs);
        }
        StmtKind::Match { subject, cases } | StmtKind::ComptimeMatch { subject, cases } => {
            retype_expr(subject, subs);
            for case in cases {
                for value in case.pattern.expressions_mut() {
                    retype_expr(value, subs);
                }
                if let Some(guard) = &mut case.guard {
                    retype_expr(guard, subs);
                }
                substitute_type_bindings_in_block(&mut case.body, subs);
            }
        }
        StmtKind::Expr(e) => retype_expr(e, subs),
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (condition, body) in branches {
                retype_condition(condition, subs);
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::ComptimeAssert { cond, message } | StmtKind::Assert { cond, message } => {
            std::iter::once(cond)
                .chain(message)
                .for_each(|value| retype_expr(value, subs));
        }
        StmtKind::While { cond, body, orelse } => {
            retype_condition(cond, subs);
            substitute_type_bindings_in_block(body, subs);
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::For {
            target,
            iter,
            body,
            orelse,
        } => {
            retype_target(target, subs);
            retype_expr(iter, subs);
            substitute_type_bindings_in_block(body, subs);
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::ComptimeFor { iter, body, .. } => {
            retype_expr(iter, subs);
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            substitute_type_bindings_in_block(body, subs);
            if let Some((_, body)) = except {
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
            if let Some(body) = finalbody {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::Scope(body) => {
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::With { items, body } => {
            for WithItem { context, .. } in items {
                retype_expr(context, subs);
            }
            substitute_type_bindings_in_block(body, subs);
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
            let inner = without_shadowed(subs, type_params);
            retype_decorators(decorators, &inner);
            for parameter in type_params.iter_mut() {
                retype_type_parameter(parameter, &inner);
            }
            for parameter in params {
                retype_fn_parameter(parameter, &inner);
            }
            if let Some(ret) = ret {
                substitute_type_bindings_in_type(ret, &inner);
            }
            if let Some(error) = raises_type {
                substitute_type_bindings_in_type(error, &inner);
            }
            for predicate in where_clauses {
                retype_expr(predicate, &inner);
            }
            substitute_type_bindings_in_block(body, &inner);
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
            enum_cases,
            ..
        } => {
            let inner = without_shadowed(subs, type_params);
            retype_decorators(decorators, &inner);
            for parameter in type_params.iter_mut() {
                retype_type_parameter(parameter, &inner);
            }
            if let Some(callable) = callable_conformance {
                substitute_type_bindings_in_type(callable, &inner);
            }
            for (_, condition) in conformance_conditions {
                retype_expr(condition, &inner);
            }
            for condition in where_clauses {
                retype_expr(condition, &inner);
            }
            for field in fields {
                substitute_type_bindings_in_type(&mut field.ty, &inner);
            }
            for ty in enum_cases
                .iter_mut()
                .flatten()
                .flat_map(|case| &mut case.payload)
            {
                substitute_type_bindings_in_type(ty, &inner);
            }
            for member in associated {
                let member_inner = without_shadowed(&inner, &member.params);
                for parameter in &mut member.params {
                    retype_type_parameter(parameter, &member_inner);
                }
                if let Some(ty) = &mut member.ty {
                    substitute_type_bindings_in_type(ty, &member_inner);
                }
                for condition in &mut member.where_clauses {
                    retype_expr(condition, &member_inner);
                }
                retype_expr(&mut member.value, &member_inner);
            }
            for method in methods {
                retype_method(method, &inner);
            }
        }
        StmtKind::Trait {
            methods,
            comptime_members,
            ..
        } => {
            for method in methods {
                let inner = without_shadowed(subs, &method.type_params);
                for parameter in &mut method.type_params {
                    retype_type_parameter(parameter, &inner);
                }
                if let Some(origins) = &mut method.self_origin {
                    retype_exprs(origins, &inner);
                }
                for parameter in &mut method.params {
                    retype_fn_parameter(parameter, &inner);
                }
                for ty in [
                    &mut method.ret,
                    &mut method.self_type,
                    &mut method.raises_type,
                ]
                .into_iter()
                .flatten()
                {
                    substitute_type_bindings_in_type(ty, &inner);
                }
                for condition in &mut method.where_clauses {
                    retype_expr(condition, &inner);
                }
                if let Some(body) = &mut method.default_body {
                    substitute_type_bindings_in_block(body, &inner);
                }
            }
            for member in comptime_members {
                let inner = without_shadowed(subs, &member.params);
                for parameter in &mut member.params {
                    retype_type_parameter(parameter, &inner);
                }
                substitute_type_bindings_in_type(&mut member.ty, &inner);
                for condition in &mut member.where_clauses {
                    retype_expr(condition, &inner);
                }
                if let Some(default) = &mut member.default {
                    retype_expr(default, &inner);
                }
            }
        }
    }
}

fn retype_expr(e: &mut Expr, subs: TypeSubs) {
    retype_bracket_head(e, subs);
    match &mut e.kind {
        ExprKind::Identifier(_) => {}
        ExprKind::Spread(value) => retype_expr(value, subs),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::EmptySubscript
        | ExprKind::IsRunInComptimeInterpreter => {}
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let mojito_ast::ast::TStringPart::Expr(value) = part {
                    retype_expr(value, subs);
                }
            }
        }
        ExprKind::TypeApply { name, args } => {
            retype_head(name, args, subs);
            retype_param_args(args, subs);
        }
        ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) => retype_expr(inner, subs),
        ExprKind::Infix(_, l, r) => {
            retype_expr(l, subs);
            retype_expr(r, subs);
        }
        ExprKind::Compare { first, rest } => {
            retype_expr(first, subs);
            for (_, r) in rest {
                retype_expr(r, subs);
            }
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } => {
            retype_head(name, param_args, subs);
            retype_param_args(param_args, subs);
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
        }
        ExprKind::Member { object, .. } => retype_expr(object, subs),
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            retype_exprs(args, subs);
            for k in kwargs.iter_mut() {
                retype_expr(&mut k.value, subs);
            }
            // `Self.T(...)` on a dropped binding constructs the bound type,
            // spelled as the ordinary constructor call it names.
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && subs(method).is_some()
            {
                let mut name = method.clone();
                let mut param_args = Vec::new();
                retype_head(&mut name, &mut param_args, subs);
                if name != *method {
                    e.kind = ExprKind::Call {
                        name,
                        param_args,
                        args: std::mem::take(args),
                        kwargs: std::mem::take(kwargs),
                    };
                    return;
                }
            }
            retype_expr(object, subs);
        }
        ExprKind::Index { object, index } => {
            retype_expr(object, subs);
            retype_expr(index, subs);
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            retype_expr(object, subs);
            for b in [lower, upper, step].into_iter().flatten() {
                retype_expr(b, subs);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            retype_expr(object, subs);
            for argument in args {
                match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        retype_expr(value, subs);
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            retype_expr(value, subs);
                        }
                    }
                }
            }
        }
        ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => retype_exprs(elems, subs),
        ExprKind::TypeValue(ty) => substitute_type_bindings_in_type(ty, subs),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            retype_expr(callee, subs);
            retype_param_args(param_args, subs);
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
        }
        ExprKind::InitializerList { args, kwargs } => {
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                retype_expr(key, subs);
                if let Some(value) = value {
                    retype_expr(value, subs);
                }
            }
        }
        // Comprehension targets bind runtime values, which cannot shadow a
        // type-parameter binding in type position, so descend unconditionally.
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            if let Some(key) = key {
                retype_expr(key, subs);
            }
            retype_expr(value, subs);
            for clause in clauses {
                match clause {
                    mojito_ast::ast::ComprehensionClause::For { target, iter, .. } => {
                        retype_target(target, subs);
                        retype_expr(iter, subs);
                    }
                    mojito_ast::ast::ComprehensionClause::If(condition) => {
                        retype_expr(condition, subs);
                    }
                }
            }
        }
        ExprKind::Uninitialized => {}
        ExprKind::Named { target, value } => {
            retype_expr(target, subs);
            retype_expr(value, subs);
        }
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            retype_expr(cond, subs);
            retype_expr(then_branch, subs);
            retype_expr(else_branch, subs);
        }
        // A lambda's hidden definition retypes like the equivalent nested
        // `def` statement (signature and body, minus its shadowed names).
        ExprKind::Lambda { def } => retype_stmt(def, subs),
    }
}

fn retype_param_args(args: &mut Vec<ParamArg>, subs: TypeSubs) {
    for argument in args {
        retype_param_arg(argument, subs);
    }
}

fn retype_param_arg(argument: &mut ParamArg, subs: TypeSubs) {
    match argument {
        ParamArg::Type(ty) => substitute_type_bindings_in_type(ty, subs),
        ParamArg::Named { value, .. } => retype_param_arg(value, subs),
        // The parser encodes a bare identifier argument (`pick[T]`) as a value
        // expression; once the binding is concrete it is a type argument.
        ParamArg::Value(expr) => {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(replacement) = subs(name)
            {
                *argument = ParamArg::Type(replacement);
            } else {
                retype_expr(expr, subs);
            }
        }
    }
}

/// Rewrite a constructor/type-application head that names a dropped binding
/// (`T(…)` / `T[…]`) to the concrete type's own head, prepending the concrete
/// type's arguments (`List[Int](…)` is head `List` with argument `[Int]`).
fn retype_head(name: &mut String, args: &mut Vec<ParamArg>, subs: TypeSubs) {
    let Some(replacement) = subs(name) else {
        return;
    };
    // No source constructor head exists (function types, references);
    // leave the call for the checker to report against the clone.
    let Type::Named(head, head_args) = replacement else {
        return;
    };
    *name = head;
    let mut merged = head_args;
    merged.append(args);
    *args = merged;
}

/// [`retype_head`] for a bracket the parser left a subscript (`T[3]`,
/// `T[a, k=b]`) over a dropped binding: the base becomes the concrete
/// type's head, and its own arguments lead the operands.
fn retype_bracket_head(e: &mut Expr, subs: TypeSubs) {
    use mojito_ast::ast::SubscriptArg;
    let span = e.span;
    let (ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. }) = &mut e.kind else {
        return;
    };
    let ExprKind::Identifier(name) = &object.kind else {
        return;
    };
    let Some(Type::Named(head, head_args)) = subs(name) else {
        return;
    };
    let Some(mut operands) = head_args
        .into_iter()
        .map(|argument| mojito_ast::ast::subscript_arg(argument, span))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    object.kind = ExprKind::Identifier(head);
    if operands.is_empty() {
        return;
    }
    let object = match std::mem::replace(&mut e.kind, ExprKind::None) {
        ExprKind::Index { object, index } => {
            if !matches!(index.kind, ExprKind::EmptySubscript) {
                operands.push(SubscriptArg::Index(*index));
            }
            object
        }
        ExprKind::MultiIndex { object, args } => {
            operands.extend(args);
            object
        }
        other => {
            e.kind = other;
            return;
        }
    };
    e.kind = match <[_; 1]>::try_from(operands) {
        Ok([SubscriptArg::Index(index)]) => ExprKind::Index {
            object,
            index: Box::new(index),
        },
        Ok(operands) => ExprKind::MultiIndex {
            object,
            args: operands.into(),
        },
        Err(operands) => ExprKind::MultiIndex {
            object,
            args: operands,
        },
    };
}

fn retype_exprs(exprs: &mut [Expr], subs: TypeSubs) {
    for value in exprs {
        retype_expr(value, subs);
    }
}

/// Substitute `subs` through a struct method, which its own parameters
/// shadow.
fn retype_method(method: &mut mojito_ast::ast::Method, subs: TypeSubs) {
    let inner = without_shadowed(subs, &method.type_params);
    retype_decorators(&mut method.decorators, &inner);
    for parameter in &mut method.type_params {
        retype_type_parameter(parameter, &inner);
    }
    if let Some(origins) = &mut method.self_origin {
        retype_exprs(origins, &inner);
    }
    for parameter in &mut method.params {
        retype_fn_parameter(parameter, &inner);
    }
    for ty in [
        &mut method.ret,
        &mut method.self_type,
        &mut method.raises_type,
    ]
    .into_iter()
    .flatten()
    {
        substitute_type_bindings_in_type(ty, &inner);
    }
    for predicate in &mut method.where_clauses {
        retype_expr(predicate, &inner);
    }
    substitute_type_bindings_in_block(&mut method.body, &inner);
}

/// The substitutions still live inside a nested declaration that introduces its
/// own type parameters: a same-named parameter shadows the outer binding.
fn without_shadowed<'a>(
    subs: TypeSubs<'a>,
    type_params: &[TypeParam],
) -> impl Fn(&str) -> Option<Type> + 'a {
    let binders: Vec<String> = type_params
        .iter()
        .map(|parameter| parameter.name.trim_start_matches('*').to_string())
        .collect();
    move |name| {
        if binders.iter().any(|binder| binder == name) {
            None
        } else {
            subs(name)
        }
    }
}
