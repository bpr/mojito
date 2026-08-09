//! AST rewriting helpers used by compile-time elaboration and specialization.

use super::*;
use crate::ast::{FnParam, ImportNames, Method};

// --- Substitution / materialization -----------------------------------------
//
// One generic rewrite over the AST parameterized by a name→value lookup, used for
// two things: substituting a `comptime for` loop variable with its literal (in the
// unrolled body — does NOT descend into nested `def`/`struct`), and materializing
// module-level `comptime` constants into runtime literals (does descend, minus a
// function's own parameter names, which shadow).

/// A name→compile-time-value lookup for a rewrite.
pub(super) type Subs<'a> = &'a dyn Fn(&str) -> Option<CtValue>;

/// Materialize module-level comptime constants throughout a program.
pub(super) fn materialize_block(stmts: Vec<Stmt>, consts: &HashMap<String, CtValue>) -> Vec<Stmt> {
    let subs: Subs = &|n| consts.get(n).cloned();
    stmts
        .into_iter()
        .map(|s| rewrite_stmt_cloned(&s, subs, true))
        .collect()
}

pub(super) fn materialize_expression(expr: &Expr, consts: &HashMap<String, CtValue>) -> Expr {
    let mut expr = expr.clone();
    let subs: Subs = &|name| consts.get(name).cloned();
    rewrite_expr(&mut expr, subs);
    expr
}

/// Replace a specialized type-pack use such as `Tuple[*Ts]` with the concrete
/// parameter list selected for this specialization. Root signature types have
/// no nested binders, so they use a one-scope rewriter; bodies use the scoped
/// entry points below.
pub(super) fn expand_type_packs(ty: &mut Type, packs: &HashMap<String, Vec<Type>>) {
    PackRewriter::new(packs).expand_type(ty);
}

/// Expand pack operations in one specialized function body. Runtime pack
/// identity comes from the already-specialized `$pack[...]` parameter type, not
/// from a name-to-length side table.
pub(super) fn expand_pack_spreads_in_function_body(
    statements: &mut [Stmt],
    parameters: &[FnParam],
    type_packs: &HashMap<String, Vec<Type>>,
) {
    let mut rewriter = PackRewriter::new(type_packs);
    rewriter.declare_parameters(parameters);
    rewriter.expand_block(statements);
}

/// Expand a declaration tree such as a specialized variadic struct. Each method
/// establishes its own parameter identities, preventing pack metadata from one
/// method leaking into a same-named ordinary parameter in another.
pub(super) fn expand_pack_spreads_in_stmt(
    statement: &mut Stmt,
    type_packs: &HashMap<String, Vec<Type>>,
) {
    PackRewriter::new(type_packs).expand_statement(statement);
}

pub(super) fn rewrite_stmt_cloned(s: &Stmt, subs: Subs, into_defs: bool) -> Stmt {
    let mut s = s.clone();
    rewrite_stmt(&mut s, subs, into_defs);
    s
}

// --- Dropped type-parameter substitution ------------------------------------
//
// `generate_def_spec` bakes concrete type arguments out of a clone's signature.
// `materialize_block` above substitutes *value* occurrences of the bindings;
// this second rewrite substitutes the *type* occurrences it leaves behind:
// annotations, compile-time argument lists, and constructor-call heads. Unlike
// the value rewrite it always descends into nested `def`/`struct` bodies —
// a dropped binding is in scope throughout the clone — shadowed only by a
// nested declaration's own type parameter of the same name.

/// A binding→concrete-source-type lookup for a dropped-parameter rewrite.
pub(super) type TypeSubs<'a> = &'a HashMap<String, Type>;

pub(super) fn substitute_type_bindings_in_block(body: &mut [Stmt], subs: TypeSubs) {
    for statement in body {
        retype_stmt(statement, subs);
    }
}

pub(super) fn substitute_type_bindings_in_type(ty: &mut Type, subs: TypeSubs) {
    for (binding, replacement) in subs {
        substitute_source_type_binding(ty, binding, replacement);
    }
}

pub(super) fn substitute_type_bindings_in_expr(expr: &mut Expr, subs: TypeSubs) {
    retype_expr(expr, subs);
}

fn rewrite_expr(e: &mut Expr, subs: Subs) {
    match &mut e.kind {
        ExprKind::Identifier(name) => {
            if let Some(v) = subs(name)
                && let Some(materialized) = v.materialize(e.span)
            {
                *e = materialized;
            }
        }
        ExprKind::Spread(value) => rewrite_expr(value, subs),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::TString { .. } => {}
        // A value **parameter** argument (`Box[CAP](…)`, `UnsafePointer[CAP]`) may
        // reference a comptime constant, so rewrite the `Value` param args too.
        ExprKind::TypeApply { args, .. } => rewrite_param_args(args, subs),
        ExprKind::Prefix(_, inner) | ExprKind::Transfer(inner) => rewrite_expr(inner, subs),
        ExprKind::Infix(_, l, r) => {
            rewrite_expr(l, subs);
            rewrite_expr(r, subs);
        }
        ExprKind::Compare { first, rest } => {
            rewrite_expr(first, subs);
            for (_, r) in rest {
                rewrite_expr(r, subs);
            }
        }
        ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } => {
            rewrite_param_args(param_args, subs);
            rewrite_exprs(args, subs);
            for k in kwargs {
                rewrite_expr(&mut k.value, subs);
            }
        }
        ExprKind::Member { object, .. } => rewrite_expr(object, subs),
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            rewrite_expr(object, subs);
            rewrite_exprs(args, subs);
            for k in kwargs {
                rewrite_expr(&mut k.value, subs);
            }
        }
        ExprKind::Index { object, index } => {
            rewrite_expr(object, subs);
            rewrite_expr(index, subs);
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            rewrite_expr(object, subs);
            for b in [lower, upper, step].into_iter().flatten() {
                rewrite_expr(b, subs);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            rewrite_expr(object, subs);
            for argument in args {
                match argument {
                    crate::ast::SubscriptArg::Index(value)
                    | crate::ast::SubscriptArg::Keyword { value, .. } => rewrite_expr(value, subs),
                    crate::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            rewrite_expr(value, subs);
                        }
                    }
                }
            }
        }
        ExprKind::ListLit(elems) | ExprKind::TupleLit(elems) => rewrite_exprs(elems, subs),
        ExprKind::TypeValue(_) => {}
        ExprKind::Invoke { .. } => {}
        ExprKind::BraceLit(_) => {}
        // Comprehension targets introduce a nested lexical environment. The
        // runtime checker/lowerer handles their expressions; blindly applying
        // this flat comptime substitution could replace a shadowed target.
        ExprKind::Comprehension { .. } => {}
        ExprKind::Uninitialized => {}
        ExprKind::Named { value, .. } => rewrite_expr(value, subs),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite_expr(cond, subs);
            rewrite_expr(then_branch, subs);
            rewrite_expr(else_branch, subs);
        }
    }
}

fn rewrite_block(body: &mut [Stmt], subs: Subs, into_defs: bool) {
    for s in body {
        rewrite_stmt(s, subs, into_defs);
    }
}

/// Identity assigned by the pre-check pack rewriter. These IDs are deliberately
/// private to elaboration: checked `OwnerId`s do not exist yet, while source
/// names are not identities in the presence of lexical shadowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ElabBindingId(u32);

/// Substitute compile-time loop bindings inside nested type syntax as well as
/// value arguments. A dependent type such as `Ts[i]` stores `i` below a
/// `ParamArg::Type`, so rewriting only top-level value arguments leaves an
/// unbound index in an otherwise-unrolled specialization.
pub(super) fn rewrite_type(ty: &mut Type, subs: Subs) {
    match ty {
        Type::Named(_, arguments) => rewrite_param_args(arguments, subs),
        Type::Assoc { base, .. } => rewrite_type(base, subs),
        Type::IndexedProjection { base, index } => {
            rewrite_type(base, subs);
            rewrite_expr(index, subs);
        }
        Type::Func {
            type_params,
            params,
            ret,
            capturing,
            raises_type,
            ..
        } => {
            for parameter in type_params {
                if let Some(value_type) = &mut parameter.value_type {
                    rewrite_type(value_type, subs);
                }
                if let Some(callable) = &mut parameter.callable_bound {
                    rewrite_type(callable, subs);
                }
                if let Some(mutability) = &mut parameter.origin_mutability {
                    rewrite_expr(mutability, subs);
                }
                if let Some(default) = &mut parameter.default {
                    rewrite_expr(default, subs);
                }
                for constraint in &mut parameter.constraints {
                    rewrite_expr(constraint, subs);
                }
            }
            for parameter in params {
                rewrite_type(&mut parameter.ty, subs);
                if let Some(origins) = &mut parameter.origin {
                    for origin in origins {
                        rewrite_expr(origin, subs);
                    }
                }
            }
            rewrite_type(ret, subs);
            for origin in capturing.iter_mut().flatten() {
                rewrite_expr(origin, subs);
            }
            if let Some(error) = raises_type {
                rewrite_type(error, subs);
            }
        }
        Type::Ref { referent, origin } => {
            rewrite_type(referent, subs);
            for origin in origin.iter_mut().flatten() {
                rewrite_expr(origin, subs);
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

/// Scope-aware resolver for the two namespaces pack rewriting touches. A
/// specialized `$pack[T0, ...]` parameter records its length against the
/// parameter's value binding, and a concrete type-pack expansion is associated
/// with the type binding seeded for the specialization. Every shadowing binder
/// receives a distinct ID even when it has the same source spelling.
struct PackRewriter {
    next_binding: u32,
    value_scopes: Vec<HashMap<String, ElabBindingId>>,
    type_scopes: Vec<HashMap<String, ElabBindingId>>,
    runtime_packs: HashMap<ElabBindingId, usize>,
    type_packs: HashMap<ElabBindingId, Vec<Type>>,
}

impl PackRewriter {
    fn new(type_packs: &HashMap<String, Vec<Type>>) -> Self {
        let mut rewriter = Self {
            next_binding: 0,
            value_scopes: vec![HashMap::new()],
            type_scopes: vec![HashMap::new()],
            runtime_packs: HashMap::new(),
            type_packs: HashMap::new(),
        };
        let mut packs: Vec<_> = type_packs
            .iter()
            .map(|(name, types)| (name.clone(), types.clone()))
            .collect();
        packs.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, types) in packs {
            let binding = rewriter.declare_type(&name);
            rewriter.type_packs.insert(binding, types);
        }
        rewriter
    }

    fn fresh_binding(&mut self) -> ElabBindingId {
        let binding = ElabBindingId(self.next_binding);
        self.next_binding += 1;
        binding
    }

    fn declare_value(&mut self, name: &str, runtime_pack: Option<usize>) -> ElabBindingId {
        let binding = self.fresh_binding();
        self.value_scopes
            .last_mut()
            .expect("pack rewriting always has a value scope")
            .insert(name.to_string(), binding);
        if let Some(length) = runtime_pack {
            self.runtime_packs.insert(binding, length);
        }
        binding
    }

    fn declare_type(&mut self, name: &str) -> ElabBindingId {
        let binding = self.fresh_binding();
        self.type_scopes
            .last_mut()
            .expect("pack rewriting always has a type scope")
            .insert(name.trim_start_matches('*').to_string(), binding);
        binding
    }

    fn resolve_value(&self, name: &str) -> Option<ElabBindingId> {
        self.value_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn resolve_type(&self, name: &str) -> Option<ElabBindingId> {
        let name = name.trim_start_matches('*');
        self.type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn runtime_pack_length(&self, name: &str) -> Option<usize> {
        self.resolve_value(name)
            .and_then(|binding| self.runtime_packs.get(&binding).copied())
    }

    fn type_pack_expansion(&self, name: &str) -> Option<Vec<Type>> {
        self.resolve_type(name)
            .and_then(|binding| self.type_packs.get(&binding).cloned())
    }

    fn push_value_scope(&mut self) {
        self.value_scopes.push(HashMap::new());
    }

    fn pop_value_scope(&mut self) {
        self.value_scopes.pop();
    }

    fn push_type_scope(&mut self) {
        self.type_scopes.push(HashMap::new());
    }

    fn pop_type_scope(&mut self) {
        self.type_scopes.pop();
    }

    fn runtime_pack_parameter_length(parameter: &FnParam) -> Option<usize> {
        match &parameter.ty {
            Type::Named(name, elements) if name == "$pack" => Some(elements.len()),
            _ => None,
        }
    }

    fn declare_parameter(&mut self, parameter: &FnParam) {
        self.declare_value(
            &parameter.name,
            Self::runtime_pack_parameter_length(parameter),
        );
    }

    fn declare_parameters(&mut self, parameters: &[FnParam]) {
        for parameter in parameters {
            self.declare_parameter(parameter);
        }
    }

    fn expand_type(&mut self, ty: &mut Type) {
        match ty {
            Type::Named(_, arguments) => self.expand_type_pack_arguments(arguments),
            Type::Assoc { base, .. } => self.expand_type(base),
            Type::IndexedProjection { base, index } => {
                self.expand_type(base);
                self.expand_expression(index);
            }
            Type::Func {
                type_params,
                params,
                ret,
                capturing,
                raises_type,
                ..
            } => {
                self.push_type_scope();
                for parameter in type_params {
                    self.expand_type_parameter(parameter);
                    self.declare_type(&parameter.name);
                }
                for param in params {
                    self.expand_type(&mut param.ty);
                    if let Some(origins) = &mut param.origin {
                        for origin in origins {
                            self.expand_expression(origin);
                        }
                    }
                }
                self.expand_type(ret);
                if let Some(origins) = capturing {
                    for origin in origins {
                        self.expand_expression(origin);
                    }
                }
                if let Some(error) = raises_type {
                    self.expand_type(error);
                }
                self.pop_type_scope();
            }
            Type::Ref { referent, origin } => {
                self.expand_type(referent);
                if let Some(origins) = origin {
                    for origin in origins {
                        self.expand_expression(origin);
                    }
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

    fn expand_type_pack_arguments(&mut self, arguments: &mut Vec<ParamArg>) {
        let mut expanded = Vec::with_capacity(arguments.len());
        for mut argument in std::mem::take(arguments) {
            match &mut argument {
                ParamArg::Type(Type::Named(name, nested))
                    if name.starts_with('*') && nested.is_empty() =>
                {
                    if let Some(types) = self.type_pack_expansion(name) {
                        expanded.extend(types.into_iter().map(ParamArg::Type));
                    } else {
                        expanded.push(argument);
                    }
                }
                ParamArg::Type(ty) => {
                    self.expand_type(ty);
                    expanded.push(argument);
                }
                ParamArg::Named { value, .. } => {
                    self.expand_type_pack_argument(value);
                    expanded.push(argument);
                }
                ParamArg::Value(value) => {
                    self.expand_expression(value);
                    expanded.push(argument);
                }
            }
        }
        *arguments = expanded;
    }

    fn expand_type_pack_argument(&mut self, argument: &mut ParamArg) {
        match argument {
            ParamArg::Type(ty) => self.expand_type(ty),
            ParamArg::Named { value, .. } => self.expand_type_pack_argument(value),
            ParamArg::Value(value) => self.expand_expression(value),
        }
    }

    fn expand_type_parameter(&mut self, parameter: &mut TypeParam) {
        if let Some(value_type) = &mut parameter.value_type {
            self.expand_type(value_type);
        }
        if let Some(callable) = &mut parameter.callable_bound {
            self.expand_type(callable);
        }
        if let Some(mutability) = &mut parameter.origin_mutability {
            self.expand_expression(mutability);
        }
        if let Some(default) = &mut parameter.default {
            self.expand_expression(default);
        }
        for constraint in &mut parameter.constraints {
            self.expand_expression(constraint);
        }
    }

    fn expand_block(&mut self, statements: &mut [Stmt]) {
        for statement in statements {
            self.expand_statement(statement);
        }
    }

    fn expand_scoped_block(&mut self, statements: &mut [Stmt]) {
        self.push_value_scope();
        self.expand_block(statements);
        self.pop_value_scope();
    }

    fn expand_statement(&mut self, statement: &mut Stmt) {
        match &mut statement.kind {
            StmtKind::VarDecl { name, ty, value } => {
                if let Some(ty) = ty {
                    self.expand_type(ty);
                }
                self.expand_expression(value);
                self.declare_value(name, None);
            }
            StmtKind::RefDecl { name, value } | StmtKind::Comptime { name, value } => {
                self.expand_expression(value);
                self.declare_value(name, None);
            }
            StmtKind::Assign { name, value } => {
                self.expand_expression(value);
                if self.resolve_value(name).is_none() {
                    self.declare_value(name, None);
                }
            }
            StmtKind::Raise(value) | StmtKind::Return(Some(value)) | StmtKind::Expr(value) => {
                self.expand_expression(value)
            }
            StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
                self.expand_expression(place);
                self.expand_expression(value);
            }
            StmtKind::Unpack { targets, value, .. } => {
                self.expand_expression(value);
                for target in targets {
                    if let ExprKind::Identifier(name) = &target.kind {
                        if self.resolve_value(name).is_none() {
                            self.declare_value(name, None);
                        }
                    } else {
                        self.expand_expression(target);
                    }
                }
            }
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (condition, body) in branches {
                    self.expand_expression(condition);
                    self.expand_scoped_block(body);
                }
                if let Some(body) = orelse {
                    self.expand_scoped_block(body);
                }
            }
            StmtKind::While { cond, body, orelse } => {
                self.expand_expression(cond);
                self.expand_scoped_block(body);
                if let Some(body) = orelse {
                    self.expand_scoped_block(body);
                }
            }
            StmtKind::For {
                var,
                iter,
                body,
                orelse,
                ..
            } => {
                self.expand_expression(iter);
                self.push_value_scope();
                self.declare_value(var, None);
                self.expand_block(body);
                self.pop_value_scope();
                if let Some(body) = orelse {
                    self.expand_scoped_block(body);
                }
            }
            StmtKind::ComptimeFor { var, iter, body } => {
                self.expand_expression(iter);
                self.push_value_scope();
                self.declare_value(var, None);
                self.expand_block(body);
                self.pop_value_scope();
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                self.expand_scoped_block(body);
                if let Some((name, body)) = except {
                    self.push_value_scope();
                    if let Some(name) = name {
                        self.declare_value(name, None);
                    }
                    self.expand_block(body);
                    self.pop_value_scope();
                }
                if let Some(body) = orelse {
                    self.expand_scoped_block(body);
                }
                if let Some(body) = finalbody {
                    self.expand_scoped_block(body);
                }
            }
            StmtKind::With { items, body } => {
                self.push_value_scope();
                for item in items {
                    self.expand_expression(&mut item.context);
                    if let Some(name) = &item.var {
                        self.declare_value(name, None);
                    }
                }
                self.expand_block(body);
                self.pop_value_scope();
            }
            StmtKind::Def {
                name,
                type_params,
                params,
                raises_type,
                ret,
                where_clause,
                body,
                ..
            } => {
                // The nested declaration itself is a binding in the enclosing
                // scope. Its own parameters then shadow that scope in the body.
                self.declare_value(name, None);
                self.push_type_scope();
                for parameter in type_params {
                    self.expand_type_parameter(parameter);
                    self.declare_type(&parameter.name);
                }
                self.push_value_scope();
                for parameter in params {
                    self.expand_type(&mut parameter.ty);
                    if let Some(default) = &mut parameter.default {
                        self.expand_expression(default);
                    }
                    self.declare_parameter(parameter);
                }
                if let Some(error) = raises_type {
                    self.expand_type(error);
                }
                if let Some(ret) = ret {
                    self.expand_type(ret);
                }
                if let Some(condition) = where_clause {
                    self.expand_expression(condition);
                }
                self.expand_block(body);
                self.pop_value_scope();
                self.pop_type_scope();
            }
            StmtKind::Struct {
                name,
                type_params,
                callable_conformance,
                conformance_conditions,
                fields,
                associated,
                methods,
                ..
            } => {
                self.declare_value(name, None);
                self.push_type_scope();
                for parameter in type_params {
                    self.expand_type_parameter(parameter);
                    self.declare_type(&parameter.name);
                }
                if let Some(callable) = callable_conformance {
                    self.expand_type(callable);
                }
                for (_, condition) in conformance_conditions {
                    self.expand_expression(condition);
                }
                for field in fields {
                    self.expand_type(&mut field.ty);
                }
                for item in associated {
                    self.expand_expression(&mut item.value);
                }
                for method in methods {
                    self.expand_method(method);
                }
                self.pop_type_scope();
            }
            StmtKind::Import { path, alias } => {
                if let Some(name) = alias.as_ref().or_else(|| path.first()) {
                    self.declare_value(name, None);
                }
            }
            StmtKind::FromImport { names, .. } => {
                if let ImportNames::Names(names) = names {
                    for imported in names {
                        self.declare_value(
                            imported.alias.as_deref().unwrap_or(&imported.name),
                            None,
                        );
                    }
                }
            }
            StmtKind::Return(None)
            | StmtKind::Pass
            | StmtKind::Break
            | StmtKind::Continue
            | StmtKind::Trait { .. } => {}
        }
    }

    fn expand_method(&mut self, method: &mut Method) {
        self.push_type_scope();
        for parameter in &mut method.type_params {
            self.expand_type_parameter(parameter);
            self.declare_type(&parameter.name);
        }
        self.push_value_scope();
        if method.has_self {
            self.declare_value("self", None);
        }
        for parameter in &mut method.params {
            self.expand_type(&mut parameter.ty);
            if let Some(default) = &mut parameter.default {
                self.expand_expression(default);
            }
            self.declare_parameter(parameter);
        }
        if let Some(error) = &mut method.raises_type {
            self.expand_type(error);
        }
        if let Some(ret) = &mut method.ret {
            self.expand_type(ret);
        }
        if let Some(condition) = &mut method.where_clause {
            self.expand_expression(condition);
        }
        self.expand_block(&mut method.body);
        self.pop_value_scope();
        self.pop_type_scope();
    }

    fn expand_expression(&mut self, expression: &mut Expr) {
        // A specialized heterogeneous runtime pack already has the exact native
        // Tuple shape selected for `*Ts`. Moving every element into
        // `Tuple(*args^)` is one whole-value relocation. The eligibility check is
        // by the resolved parameter identity, never by the spelling `args`.
        let whole_pack_transfer = matches!(
            &expression.kind,
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } if name == "__RuntimeTuple"
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [argument]
                    if matches!(&argument.kind, ExprKind::Spread(spread)
                        if matches!(&spread.kind, ExprKind::Transfer(inner)
                            if matches!(&inner.kind, ExprKind::Identifier(pack)
                                if self.runtime_pack_length(pack).is_some()))))
        );
        if whole_pack_transfer {
            let ExprKind::Call { args, .. } = &expression.kind else {
                unreachable!();
            };
            let ExprKind::Spread(spread) = &args[0].kind else {
                unreachable!();
            };
            let ExprKind::Transfer(inner) = &spread.kind else {
                unreachable!();
            };
            expression.kind = ExprKind::Transfer(inner.clone());
            return;
        }

        match &mut expression.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                self.expand_type_pack_arguments(param_args);
                for argument in args.iter_mut() {
                    self.expand_expression(argument);
                }
                for argument in kwargs {
                    self.expand_expression(&mut argument.value);
                }
                if name == "__RuntimeTuple" || name == "Tuple" {
                    *args = self.expand_tuple_spread_arguments(std::mem::take(args));
                }
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.expand_expression(callee);
                self.expand_type_pack_arguments(param_args);
                for argument in args {
                    self.expand_expression(argument);
                }
                for argument in kwargs {
                    self.expand_expression(&mut argument.value);
                }
            }
            ExprKind::TypeApply { args, .. } => self.expand_type_pack_arguments(args),
            ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
                self.expand_expression(value)
            }
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.expand_expression(left);
                self.expand_expression(right);
            }
            ExprKind::Compare { first, rest } => {
                self.expand_expression(first);
                for (_, value) in rest {
                    self.expand_expression(value);
                }
            }
            ExprKind::Member { object, .. } => self.expand_expression(object),
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.expand_expression(object);
                for argument in args {
                    self.expand_expression(argument);
                }
                for argument in kwargs {
                    self.expand_expression(&mut argument.value);
                }
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.expand_expression(object);
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.expand_expression(bound);
                }
            }
            ExprKind::MultiIndex { object, args } => {
                self.expand_expression(object);
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value)
                        | crate::ast::SubscriptArg::Keyword { value, .. } => {
                            self.expand_expression(value)
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for value in [lower, upper, step].into_iter().flatten() {
                                self.expand_expression(value);
                            }
                        }
                    }
                }
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                for value in values {
                    self.expand_expression(value);
                }
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.expand_expression(key);
                    if let Some(value) = value {
                        self.expand_expression(value);
                    }
                }
            }
            ExprKind::Comprehension {
                key,
                value,
                clauses,
                ..
            } => {
                // Each generator binds to its right, but not in its own iterable.
                self.push_value_scope();
                for clause in clauses {
                    match clause {
                        crate::ast::ComprehensionClause::For { var, iter, .. } => {
                            self.expand_expression(iter);
                            self.declare_value(var, None);
                        }
                        crate::ast::ComprehensionClause::If(condition) => {
                            self.expand_expression(condition)
                        }
                    }
                }
                if let Some(key) = key {
                    self.expand_expression(key);
                }
                self.expand_expression(value);
                self.pop_value_scope();
            }
            ExprKind::TypeValue(ty) => self.expand_type(ty),
            ExprKind::Named { value, .. } => self.expand_expression(value),
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expand_expression(cond);
                self.expand_expression(then_branch);
                self.expand_expression(else_branch);
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let crate::ast::TStringPart::Expr(value) = part {
                        self.expand_expression(value);
                    }
                }
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Uninitialized
            | ExprKind::Identifier(_) => {}
        }
    }

    fn expand_tuple_spread_arguments(&self, arguments: Vec<Expr>) -> Vec<Expr> {
        let mut expanded = Vec::new();
        for argument in arguments {
            let ExprKind::Spread(value) = &argument.kind else {
                expanded.push(argument);
                continue;
            };
            let (pack, transferring) = match &value.kind {
                ExprKind::Identifier(name) => (name.as_str(), false),
                ExprKind::Transfer(value) => match &value.kind {
                    ExprKind::Identifier(name) => (name.as_str(), true),
                    _ => {
                        expanded.push(argument);
                        continue;
                    }
                },
                _ => {
                    expanded.push(argument);
                    continue;
                }
            };
            let Some(length) = self.runtime_pack_length(pack) else {
                expanded.push(argument);
                continue;
            };
            let source = argument.source.clone();
            let span = argument.span;
            for index in 0..length {
                let base = Expr {
                    kind: ExprKind::Identifier(pack.to_string()),
                    span,
                    source: source.clone(),
                    syntax_id: crate::token::SyntaxId::fresh(),
                };
                let index = Expr {
                    kind: ExprKind::Int((index as i64).into()),
                    span,
                    source: source.clone(),
                    syntax_id: crate::token::SyntaxId::fresh(),
                };
                let indexed = Expr {
                    kind: ExprKind::Index {
                        object: Box::new(base),
                        index: Box::new(index),
                    },
                    span,
                    source: source.clone(),
                    syntax_id: crate::token::SyntaxId::fresh(),
                };
                expanded.push(if transferring {
                    Expr {
                        kind: ExprKind::Transfer(Box::new(indexed)),
                        span,
                        source: source.clone(),
                        syntax_id: crate::token::SyntaxId::fresh(),
                    }
                } else {
                    indexed
                });
            }
        }
        expanded
    }
}

fn rewrite_exprs(es: &mut [Expr], subs: Subs) {
    for e in es {
        rewrite_expr(e, subs);
    }
}

fn rewrite_param_args(args: &mut [crate::ast::ParamArg], subs: Subs) {
    for a in args {
        match a {
            crate::ast::ParamArg::Type(ty) => rewrite_type(ty, subs),
            crate::ast::ParamArg::Value(e) => rewrite_expr(e, subs),
            crate::ast::ParamArg::Named { value, .. } => {
                rewrite_param_args(std::slice::from_mut(value), subs);
            }
        }
    }
}

fn rewrite_stmt(s: &mut Stmt, subs: Subs, into_defs: bool) {
    match &mut s.kind {
        StmtKind::VarDecl { value, .. }
        | StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Comptime { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value)) => rewrite_expr(value, subs),
        StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            rewrite_expr(place, subs);
            rewrite_expr(value, subs);
        }
        StmtKind::Unpack { targets, value, .. } => {
            rewrite_exprs(targets, subs);
            rewrite_expr(value, subs);
        }
        StmtKind::Expr(e) => rewrite_expr(e, subs),
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (c, b) in branches {
                rewrite_expr(c, subs);
                rewrite_block(b, subs, into_defs);
            }
            if let Some(b) = orelse {
                rewrite_block(b, subs, into_defs);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            rewrite_expr(cond, subs);
            rewrite_block(body, subs, into_defs);
            if let Some(body) = orelse {
                rewrite_block(body, subs, into_defs);
            }
        }
        StmtKind::For {
            iter, body, orelse, ..
        } => {
            rewrite_expr(iter, subs);
            rewrite_block(body, subs, into_defs);
            if let Some(body) = orelse {
                rewrite_block(body, subs, into_defs);
            }
        }
        StmtKind::ComptimeFor { iter, body, .. } => {
            rewrite_expr(iter, subs);
            rewrite_block(body, subs, into_defs);
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            rewrite_block(body, subs, into_defs);
            if let Some((_, b)) = except {
                rewrite_block(b, subs, into_defs);
            }
            if let Some(b) = orelse {
                rewrite_block(b, subs, into_defs);
            }
            if let Some(b) = finalbody {
                rewrite_block(b, subs, into_defs);
            }
        }
        StmtKind::With { items, body } => {
            for WithItem { context, .. } in items {
                rewrite_expr(context, subs);
            }
            rewrite_block(body, subs, into_defs);
        }
        // A nested `def`/`struct` is a separate scope. For materialization
        // (`into_defs`), descend but shadow the function's parameters (a parameter
        // named like a module constant is *not* that constant). For loop-variable
        // substitution, don't descend (the loop var is an outer compile-time symbol).
        StmtKind::Def { params, body, .. } => {
            if into_defs {
                let shadowed: HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
                let inner: Subs = &|n| if shadowed.contains(n) { None } else { subs(n) };
                rewrite_block(body, inner, into_defs);
            }
        }
        StmtKind::Struct { methods, .. } => {
            if into_defs {
                for m in methods {
                    let mut shadowed: HashSet<&str> =
                        m.params.iter().map(|p| p.name.as_str()).collect();
                    shadowed.insert("self");
                    let inner: Subs = &|n| if shadowed.contains(n) { None } else { subs(n) };
                    rewrite_block(&mut m.body, inner, into_defs);
                }
            }
        }
        StmtKind::Trait { .. } => {}
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
        | StmtKind::Comptime { value, .. }
        | StmtKind::Raise(value)
        | StmtKind::Return(Some(value)) => retype_expr(value, subs),
        StmtKind::Return(None) | StmtKind::Pass | StmtKind::Break | StmtKind::Continue => {}
        StmtKind::Import { .. } | StmtKind::FromImport { .. } => {}
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            retype_expr(place, subs);
            retype_expr(value, subs);
        }
        StmtKind::Unpack { targets, value, .. } => {
            retype_exprs(targets, subs);
            retype_expr(value, subs);
        }
        StmtKind::Expr(e) => retype_expr(e, subs),
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (c, b) in branches {
                retype_expr(c, subs);
                substitute_type_bindings_in_block(b, subs);
            }
            if let Some(b) = orelse {
                substitute_type_bindings_in_block(b, subs);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            retype_expr(cond, subs);
            substitute_type_bindings_in_block(body, subs);
            if let Some(body) = orelse {
                substitute_type_bindings_in_block(body, subs);
            }
        }
        StmtKind::For {
            iter, body, orelse, ..
        } => {
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
            if let Some((_, b)) = except {
                substitute_type_bindings_in_block(b, subs);
            }
            if let Some(b) = orelse {
                substitute_type_bindings_in_block(b, subs);
            }
            if let Some(b) = finalbody {
                substitute_type_bindings_in_block(b, subs);
            }
        }
        StmtKind::With { items, body } => {
            for WithItem { context, .. } in items {
                retype_expr(context, subs);
            }
            substitute_type_bindings_in_block(body, subs);
        }
        StmtKind::Def {
            type_params,
            params,
            raises_type,
            ret,
            where_clause,
            body,
            ..
        } => {
            let Some(inner) = without_shadowed(subs, type_params) else {
                return;
            };
            for parameter in params.iter_mut() {
                substitute_type_bindings_in_type(&mut parameter.ty, &inner);
                if let Some(default) = &mut parameter.default {
                    retype_expr(default, &inner);
                }
            }
            if let Some(ret) = ret {
                substitute_type_bindings_in_type(ret, &inner);
            }
            if let Some(error) = raises_type {
                substitute_type_bindings_in_type(error, &inner);
            }
            if let Some(predicate) = where_clause {
                retype_expr(predicate, &inner);
            }
            substitute_type_bindings_in_block(body, &inner);
        }
        StmtKind::Struct {
            type_params,
            fields,
            associated,
            methods,
            ..
        } => {
            let Some(inner) = without_shadowed(subs, type_params) else {
                return;
            };
            for field in fields.iter_mut() {
                substitute_type_bindings_in_type(&mut field.ty, &inner);
            }
            for member in associated.iter_mut() {
                retype_expr(&mut member.value, &inner);
            }
            for method in methods.iter_mut() {
                let Some(inner) = without_shadowed(&inner, &method.type_params) else {
                    continue;
                };
                for parameter in method.params.iter_mut() {
                    substitute_type_bindings_in_type(&mut parameter.ty, &inner);
                    if let Some(default) = &mut parameter.default {
                        retype_expr(default, &inner);
                    }
                }
                if let Some(ret) = &mut method.ret {
                    substitute_type_bindings_in_type(ret, &inner);
                }
                if let Some(error) = &mut method.raises_type {
                    substitute_type_bindings_in_type(error, &inner);
                }
                if let Some(predicate) = &mut method.where_clause {
                    retype_expr(predicate, &inner);
                }
                substitute_type_bindings_in_block(&mut method.body, &inner);
            }
        }
        StmtKind::Trait { .. } => {}
    }
}

fn retype_expr(e: &mut Expr, subs: TypeSubs) {
    match &mut e.kind {
        ExprKind::Identifier(_) => {}
        ExprKind::Spread(value) => retype_expr(value, subs),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::TString { .. } => {}
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
            args,
            kwargs,
            ..
        } => {
            retype_expr(object, subs);
            retype_exprs(args, subs);
            for k in kwargs {
                retype_expr(&mut k.value, subs);
            }
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
                    crate::ast::SubscriptArg::Index(value)
                    | crate::ast::SubscriptArg::Keyword { value, .. } => retype_expr(value, subs),
                    crate::ast::SubscriptArg::Slice {
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
                    crate::ast::ComprehensionClause::For { iter, .. } => retype_expr(iter, subs),
                    crate::ast::ComprehensionClause::If(condition) => retype_expr(condition, subs),
                }
            }
        }
        ExprKind::Uninitialized => {}
        ExprKind::Named { value, .. } => retype_expr(value, subs),
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            retype_expr(cond, subs);
            retype_expr(then_branch, subs);
            retype_expr(else_branch, subs);
        }
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
                && let Some(replacement) = subs.get(name)
            {
                *argument = ParamArg::Type(replacement.clone());
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
    let Some(replacement) = subs.get(name.as_str()) else {
        return;
    };
    let (head, head_args) = match replacement {
        Type::Named(head, head_args) => (head.clone(), head_args.clone()),
        Type::Int => ("Int".to_string(), Vec::new()),
        Type::UInt => ("UInt".to_string(), Vec::new()),
        Type::Bool => ("Bool".to_string(), Vec::new()),
        Type::StringLiteral => ("String".to_string(), Vec::new()),
        Type::Float64 => ("Float64".to_string(), Vec::new()),
        // No source constructor head exists (function types, references);
        // leave the call for the checker to report against the clone.
        _ => return,
    };
    *name = head;
    let mut merged = head_args;
    merged.append(args);
    *args = merged;
}

fn retype_exprs(exprs: &mut [Expr], subs: TypeSubs) {
    for value in exprs {
        retype_expr(value, subs);
    }
}

/// The substitutions still live inside a nested declaration that introduces its
/// own type parameters: a same-named parameter shadows the outer binding.
/// `None` means nothing is left to substitute.
fn without_shadowed(subs: TypeSubs, type_params: &[TypeParam]) -> Option<HashMap<String, Type>> {
    let inner: HashMap<String, Type> = subs
        .iter()
        .filter(|(binding, _)| {
            !type_params
                .iter()
                .any(|parameter| parameter.name.trim_start_matches('*') == binding.as_str())
        })
        .map(|(binding, replacement)| (binding.clone(), replacement.clone()))
        .collect();
    if inner.is_empty() { None } else { Some(inner) }
}
