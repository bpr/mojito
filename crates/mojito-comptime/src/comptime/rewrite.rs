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

/// A binding→concrete-source-type lookup for a dropped-parameter rewrite:
/// the expression spelling the type the binding stands for.
pub(super) type TypeSubs<'a> = &'a dyn Fn(&str) -> Option<Expr>;

pub(super) fn substitute_type_bindings_in_block(body: &mut [Stmt], subs: TypeSubs) {
    for statement in body {
        retype_stmt(statement, subs);
    }
}

/// Replace `e` by `replacement` at `e`'s occurrence, which keeps its span,
/// source, and identity; a part `replacement` synthesized without a span
/// takes `e`'s.
pub(super) fn put_replacement(e: &mut Expr, mut replacement: Expr) {
    struct Respan(mojito_common::token::Span);
    impl mojito_ast::visit::MutVisitor for Respan {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if expr.span == mojito_common::token::DUMMY_SPAN {
                expr.span = self.0;
            }
        }
    }
    mojito_ast::visit::walk_expr_mut(&mut Respan(e.span), &mut replacement);
    e.kind = replacement.kind;
}

/// Substitute a pack spread (`*Ts`, `*Self.Ts`) under the pack's
/// `*`-prefixed name; whether `subs` answered it.
pub(super) fn retype_spread(e: &mut Expr, subs: TypeSubs) -> bool {
    let ExprKind::Spread(operand) = &e.kind else {
        return false;
    };
    let replacement = operand.pack_spread_type().and_then(|pack| match pack {
        Type::Named(name, _) | Type::SelfParam(name) => subs(&name),
        _ => None,
    });
    replacement.is_some_and(|replacement| {
        put_replacement(e, replacement);
        true
    })
}

/// Substitutes the type occurrences of dropped bindings in an annotation
/// slot's expression, position by position along the type it spells: a bare
/// binder, `Self.binder`, or bracket operand naming one becomes the bound
/// type, while a bracket's head and any expression the annotation evaluates
/// keep their own spelling. A pack spread (`*Ts`, `*Self.Ts`) substitutes
/// under the pack's `*`-prefixed name.
pub(super) fn retype_annotation(e: &mut Expr, subs: TypeSubs) {
    if matches!(e.kind, ExprKind::Spread(_)) {
        retype_spread(e, subs);
        return;
    }
    let binder = match &e.kind {
        ExprKind::Identifier(name) if name != "Self" => Some(name.as_str()),
        _ => e.self_member(),
    };
    if let Some(replacement) = binder.and_then(subs) {
        put_replacement(e, replacement);
        return;
    }
    if e.self_member().is_some() {
        return;
    }
    let applied_name = e.bracket_operands().map(|(object, args)| {
        (
            matches!(&object.kind, ExprKind::Identifier(name) if name != "Self"),
            args,
        )
    });
    let evaluated = match &mut e.kind {
        ExprKind::Identifier(_) | ExprKind::None => false,
        ExprKind::Member { object, .. } if object.annotation_type().is_some() => {
            retype_annotation(object, subs);
            false
        }
        ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } if name == "type_of" && param_args.is_empty() && args.len() == 1 && kwargs.is_empty() => {
            false
        }
        ExprKind::Index { object, index } if let Some((applied, args)) = applied_name => {
            if applied {
                // `Named(name, [])` for the empty subscript `T[]`.
                if args.is_empty() {
                    if let ExprKind::Identifier(name) = &object.kind
                        && let Some(replacement) = subs(name)
                    {
                        put_replacement(e, replacement);
                    }
                    return;
                }
                retype_annotation_operand(index, subs);
                false
            } else {
                !retype_projection(object, &mut [index], args.len() == 1, subs)
            }
        }
        ExprKind::MultiIndex { object, args } if let Some((applied, operands)) = applied_name => {
            let mut indices: Vec<&mut Expr> = args
                .iter_mut()
                .filter_map(|argument| match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => Some(value),
                    _ => None,
                })
                .collect();
            if applied {
                for index in indices {
                    retype_annotation_operand(index, subs);
                }
                false
            } else {
                let projection = operands.len() == 1 && matches!(operands[0], ParamArg::Value(_));
                !retype_projection(object, &mut indices, projection, subs)
            }
        }
        _ => true,
    };
    if evaluated {
        retype_expr(e, subs);
    }
}

/// A bracket on a base other than a plain name: one value operand projects
/// the base (`Self[0]`), whose index keeps its spelling; any other list
/// applies an argument-less member (`M.A[T]`), whose base and operands
/// substitute as an applied type's do. `false` for any other bracket, the
/// value it evaluates to.
fn retype_projection(
    object: &mut Expr,
    indices: &mut [&mut Expr],
    projection: bool,
    subs: TypeSubs,
) -> bool {
    let Some(base) = object.annotation_type() else {
        return false;
    };
    if projection && indices.len() == 1 && !indices[0].is_type_expression() {
        retype_annotation(object, subs);
        return true;
    }
    let ExprKind::Member {
        object: base_object,
        ..
    } = &mut object.kind
    else {
        return false;
    };
    if !matches!(&base, Type::Assoc { args, .. } if args.is_empty()) {
        return false;
    }
    retype_annotation(base_object, subs);
    for index in indices {
        retype_annotation_operand(index, subs);
    }
    true
}

/// A bracket operand of an applied annotation: a function or generator
/// type substitutes inside, and a bare or `Self.` binder becomes
/// the bound type; any other value operand keeps its spelling.
fn retype_annotation_operand(operand: &mut Expr, subs: TypeSubs) {
    match &mut operand.kind {
        ExprKind::FunctionType(_) | ExprKind::GeneratorType { .. } => {
            retype_expr(operand, subs);
            return;
        }
        ExprKind::Spread(_) => {
            retype_spread(operand, subs);
            return;
        }
        _ => {}
    }
    let binder = match &operand.kind {
        ExprKind::Identifier(name) => Some(name.as_str()),
        _ => operand.self_member(),
    };
    if let Some(replacement) = binder.and_then(subs) {
        put_replacement(operand, replacement);
    }
}

/// [`retype_annotation`] for an associated `comptime` member's annotation,
/// where an `&` chain composes traits: each member substitutes as an annotation.
fn retype_composition(e: &mut Expr, subs: TypeSubs) {
    if let ExprKind::Infix(mojito_ast::ast::InfixOp::BitAnd, left, right) = &mut e.kind {
        retype_composition(left, subs);
        retype_composition(right, subs);
        return;
    }
    retype_annotation(e, subs);
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
        retype_annotation(value_type, subs);
    }
    if let Some(callable) = &mut parameter.callable_bound {
        retype_annotation(callable, subs);
    }
    if let Some(mutability) = &mut parameter.origin_mutability {
        retype_expr(mutability, subs);
    }
    if let Some(default) = &mut parameter.default {
        retype_expr(default, subs);
    }
}

fn retype_fn_parameter(parameter: &mut FnParam, subs: TypeSubs) {
    retype_annotation(&mut parameter.ty, subs);
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
                retype_annotation(ty, subs);
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
                retype_annotation(ty, &inner);
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
        StmtKind::Match { subject, cases } | StmtKind::ComptimeMatch { subject, cases, .. } => {
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
                retype_annotation(ret, &inner);
            }
            if let Some(error) = raises_type {
                retype_annotation(error, &inner);
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
                retype_annotation(callable, &inner);
            }
            for (_, condition) in conformance_conditions {
                retype_expr(condition, &inner);
            }
            for condition in where_clauses {
                retype_expr(condition, &inner);
            }
            for field in fields {
                retype_annotation(&mut field.ty, &inner);
            }
            for ty in enum_cases
                .iter_mut()
                .flatten()
                .flat_map(|case| &mut case.payload)
            {
                retype_annotation(ty, &inner);
            }
            for member in associated {
                let member_inner = without_shadowed(&inner, &member.params);
                for parameter in &mut member.params {
                    retype_type_parameter(parameter, &member_inner);
                }
                if let Some(ty) = &mut member.ty {
                    retype_composition(ty, &member_inner);
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
                    retype_annotation(ty, &inner);
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
                retype_composition(&mut member.ty, &inner);
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
    if retype_spread(e, subs) {
        return;
    }
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
        ExprKind::FunctionType(callable) => retype_function_type(callable, subs),
        ExprKind::GeneratorType { params, result } => {
            retype_generator_type(params, result, subs);
        }
    }
}

/// A function type's signature, which its own `def[...]` binders shadow;
/// parameter types, result, and typed error substitute as annotations.
fn retype_function_type(callable: &mut mojito_ast::ast::FunctionTypeExpr, subs: TypeSubs) {
    let inner = without_shadowed(subs, &callable.type_params);
    for parameter in &mut callable.type_params {
        retype_type_parameter(parameter, &inner);
    }
    for parameter in &mut callable.params {
        retype_annotation(&mut parameter.ty, &inner);
        if let Some(origins) = &mut parameter.origin {
            retype_exprs(origins, &inner);
        }
    }
    retype_annotation(&mut callable.ret, &inner);
    if let Some(error) = &mut callable.raises_type {
        retype_annotation(error, &inner);
    }
    retype_exprs(&mut callable.where_clauses, &inner);
}

/// A generator type's result, which its own binders shadow; the result
/// substitutes as a trait composition.
fn retype_generator_type(params: &mut [TypeParam], result: &mut Expr, subs: TypeSubs) {
    let inner = without_shadowed(subs, params);
    for parameter in params.iter_mut() {
        retype_type_parameter(parameter, &inner);
    }
    retype_composition(result, &inner);
}

fn retype_param_args(args: &mut Vec<ParamArg>, subs: TypeSubs) {
    for argument in args {
        retype_param_arg(argument, subs);
    }
}

fn retype_param_arg(argument: &mut ParamArg, subs: TypeSubs) {
    match argument {
        ParamArg::Named { value, .. } => retype_param_arg(value, subs),
        // A bare binder operand (`pick[T]`) is the bound type once the
        // binding is concrete.
        ParamArg::Value(expr) => {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(replacement) = subs(name)
            {
                put_replacement(expr, replacement);
            } else {
                retype_expr(expr, subs);
            }
        }
    }
}

/// The constructor name and bracket operands a substituted type spells,
/// the head of a `Name[args](...)` call; `None` for a replacement no
/// constructor head spells.
fn replacement_head(replacement: &Expr) -> Option<(String, Vec<ParamArg>)> {
    match &replacement.kind {
        ExprKind::Identifier(name) => Some((name.clone(), Vec::new())),
        _ => replacement
            .bracket_application()
            .map(|(name, args)| (name.to_string(), args.into_owned())),
    }
}

/// Rewrite a constructor/type-application head that names a dropped binding
/// (`T(…)` / `T[…]`) to the concrete type's own head, prepending the concrete
/// type's arguments (`List[Int](…)` is head `List` with argument `[Int]`).
fn retype_head(name: &mut String, args: &mut Vec<ParamArg>, subs: TypeSubs) {
    // No source constructor head exists (function types, references);
    // leave the call for the checker to report against the clone.
    let Some((head, head_args)) = subs(name).as_ref().and_then(replacement_head) else {
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
    let (ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. }) = &mut e.kind else {
        return;
    };
    let ExprKind::Identifier(name) = &object.kind else {
        return;
    };
    let Some((head, head_args)) = subs(name).as_ref().and_then(replacement_head) else {
        return;
    };
    let Some(mut operands) = head_args
        .into_iter()
        .map(mojito_ast::ast::subscript_arg)
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
        retype_annotation(ty, &inner);
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
) -> impl Fn(&str) -> Option<Expr> + 'a {
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

#[cfg(test)]
mod tests {
    use super::{retype_annotation, retype_composition};
    use mojito_ast::ast::{Expr, ExprKind, InfixOp, SubscriptArg};

    fn name(spelling: &str) -> Box<Expr> {
        Box::new(Expr::from(ExprKind::Identifier(spelling.to_string())))
    }

    fn member(object: Box<Expr>, field: &str) -> Box<Expr> {
        Box::new(Expr::from(ExprKind::Member {
            object,
            field: field.to_string(),
        }))
    }

    fn index(object: Box<Expr>, index: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::from(ExprKind::Index { object, index }))
    }

    fn call(callee: &str, operand: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::from(ExprKind::Call {
            name: callee.to_string(),
            param_args: Vec::new(),
            args: vec![*operand],
            kwargs: Vec::new(),
        }))
    }

    fn list_of_int() -> Box<Expr> {
        index(name("List"), name("Int"))
    }

    fn subs(binder: &str) -> Option<Expr> {
        matches!(binder, "T" | "U" | "M").then(|| *list_of_int())
    }

    /// `annotation`'s substitution is `expected`.
    fn assert_retyped(mut annotation: Box<Expr>, expected: Box<Expr>) {
        retype_annotation(&mut annotation, &subs);
        assert_eq!(annotation, expected);
    }

    #[test]
    fn an_annotation_substitutes_its_type_positions() {
        // `T`, `Self.T`, `Self`, `None`.
        assert_retyped(name("T"), list_of_int());
        assert_retyped(member(name("Self"), "T"), list_of_int());
        assert_retyped(name("Self"), name("Self"));
        let none = || Box::new(Expr::from(ExprKind::None));
        assert_retyped(none(), none());
        // `List[T]`, `T[Int]` (a head keeps its spelling), `Dict[T, Self.U]`,
        // `List[f(T)]` (a value operand keeps its spelling).
        assert_retyped(
            index(name("List"), name("T")),
            index(name("List"), list_of_int()),
        );
        assert_retyped(index(name("T"), name("Int")), index(name("T"), name("Int")));
        let dict = |key: Box<Expr>, value: Box<Expr>| {
            Box::new(Expr::from(ExprKind::MultiIndex {
                object: name("Dict"),
                args: vec![
                    SubscriptArg::Index(*key),
                    SubscriptArg::Keyword {
                        name: "V".to_string(),
                        value: *value,
                    },
                ],
            }))
        };
        assert_retyped(
            dict(name("T"), member(name("Self"), "U")),
            dict(list_of_int(), list_of_int()),
        );
        assert_retyped(
            index(name("List"), call("f", name("T"))),
            index(name("List"), call("f", name("T"))),
        );
        // `M.A` (a namespace base substitutes), `Self.A[T]` (a projection's
        // index keeps its spelling), `Self.T[0]`.
        assert_retyped(member(name("M"), "A"), member(list_of_int(), "A"));
        assert_retyped(
            index(member(name("Self"), "A"), name("T")),
            index(member(name("Self"), "A"), name("T")),
        );
        let zero = || Box::new(Expr::from(ExprKind::Int(0.into())));
        assert_retyped(
            index(member(name("Self"), "T"), zero()),
            index(list_of_int(), zero()),
        );
        // `type_of(T)` reads no binder; `T if c else U` evaluates, and a
        // name it reads is a value.
        assert_retyped(call("type_of", name("T")), call("type_of", name("T")));
        let conditional = || {
            Box::new(Expr::from(ExprKind::IfExpr {
                cond: name("c"),
                then_branch: name("T"),
                else_branch: name("U"),
            }))
        };
        assert_retyped(conditional(), conditional());
    }

    #[test]
    fn a_composition_substitutes_each_member() {
        let composition = |sized: Box<Expr>| {
            Expr::from(ExprKind::Infix(
                InfixOp::BitAnd,
                name("Copyable"),
                index(name("Sized"), sized),
            ))
        };
        let mut retyped = composition(name("T"));
        retype_composition(&mut retyped, &subs);
        assert_eq!(retyped, composition(list_of_int()));
    }
}
