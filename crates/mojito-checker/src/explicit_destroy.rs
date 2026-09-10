//! Checked explicit-destruction obligations over structured source CFGs.

use mojito_ast::ast::{ArgConvention, Expr, ExprKind, SourceType, Stmt, StmtKind, TStringPart};
use mojito_checked::checked::{AnnotationSite, ExplicitDestroyInfo};
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use mojito_types::types::Ty;
use std::collections::HashMap;
use std::collections::HashSet;

/// Positive deletability facts proved by the type checker in the lexical
/// constraint environment where each binding is introduced.
///
/// Conditional conformances cannot be recovered from a nominal type name after
/// checking: `List[T]` is linear in general, while it is ordinarily droppable
/// under a proven `T: Deinitable` constraint.
#[derive(Default)]
pub struct CheckedDeletability {
    pub declarations: HashSet<AnnotationSite>,
    pub bindings: HashSet<SourceSpan>,
    /// Parameters typed by a type parameter (or an opaque dependent pack
    /// element) whose bounds do not prove `Deinitable`: an owned one is
    /// linear, as upstream.
    pub linear_declarations: HashSet<AnnotationSite>,
    /// Bindings whose declared type is such a type parameter.
    pub linear_bindings: HashSet<SourceSpan>,
}

/// The synthetic explicit-destroy type name of a value typed by a
/// non-`Deinitable` type parameter (`ExplicitDestroyInfo` key).
pub const LINEAR_TYPE_PARAMETER: &str = "$linear";

#[allow(clippy::implicit_hasher, reason = "TODO: generalize over BuildHasher")]
pub fn check(
    statements: &[Stmt],
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    if types.is_empty() {
        return Ok(());
    }
    for statement in statements {
        match &statement.kind {
            StmtKind::Def { params, body, .. } => check_def(
                statement,
                params,
                body,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )?,
            // A template shell carries signatures only.
            StmtKind::Struct {
                template_shell: true,
                ..
            } => {}
            StmtKind::Struct { name, methods, .. } => {
                for (method_index, method) in methods.iter().enumerate() {
                    let mut params = method
                        .params
                        .iter()
                        .enumerate()
                        .map(|(param, p)| {
                            let site = AnnotationSite::MethodParam {
                                module: statement.module.clone(),
                                declaration: name.clone(),
                                method: method_index,
                                param,
                            };
                            (
                                &p.name,
                                &p.ty,
                                p.convention,
                                deletability.declarations.contains(&site),
                                deletability.linear_declarations.contains(&site),
                            )
                        })
                        .collect::<Vec<_>>();
                    if method.has_self && method.self_convention != Some(ArgConvention::Deinit) {
                        // `self` is borrowed or initialized here, never a new obligation.
                        params.retain(|_| true);
                    }
                    check_function(
                        params.into_iter(),
                        &method.body,
                        binding_types,
                        comprehension_bindings,
                        deletability,
                        types,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_expr(
    expr: &Expr,
    env: &mut Env,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    match &expr.kind {
        ExprKind::Transfer(inner) => move_root(inner, env, types)?,
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            for arg in args {
                check_expr(arg, env, comprehension_bindings, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, comprehension_bindings, types)?;
            }
            if let ExprKind::Transfer(inner) = &object.kind
                && let Some((id, path)) = obligation_place(inner, env)
                && ensure_initialized(id, env)?
                && let Some(type_name) = &env.vars[id].explicit_type
                && types
                    .get(type_name)
                    .is_some_and(|_| destructor_at_path(type_name, &path, method, types))
            {
                consume_destructor(id, &path, env, types)?;
            } else {
                check_expr(object, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::Call { args, kwargs, .. } => {
            for arg in args {
                check_expr(arg, env, comprehension_bindings, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => {
            check_expr(callee, env, comprehension_bindings, types)?;
            for arg in args {
                check_expr(arg, env, comprehension_bindings, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::Prefix(_, value) | ExprKind::Named { value, .. } => {
            check_expr(value, env, comprehension_bindings, types)?;
        }
        ExprKind::Infix(_, left, right) => {
            check_expr(left, env, comprehension_bindings, types)?;
            check_expr(right, env, comprehension_bindings, types)?;
        }
        ExprKind::Member { object, .. } => check_expr(object, env, comprehension_bindings, types)?,
        ExprKind::Index { object, index } => {
            check_expr(object, env, comprehension_bindings, types)?;
            check_expr(index, env, comprehension_bindings, types)?;
        }
        ExprKind::Identifier(name) => {
            if let Some(id) = env.lookup(name) {
                ensure_initialized(id, env)?;
            }
        }
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                check_expr(value, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::BraceLit(values) => {
            for (key, value) in values {
                check_expr(key, env, comprehension_bindings, types)?;
                if let Some(value) = value {
                    check_expr(value, env, comprehension_bindings, types)?;
                }
            }
        }
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => check_comprehension_expr(
            expr,
            key.as_deref(),
            value,
            clauses,
            env,
            comprehension_bindings,
            types,
        )?,
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            check_expr(cond, env, comprehension_bindings, types)?;
            let mut a = env.clone();
            let mut b = env.clone();
            check_expr(then_branch, &mut a, comprehension_bindings, types)?;
            check_expr(else_branch, &mut b, comprehension_bindings, types)?;
            *env = join(vec![a, b])?;
        }
        ExprKind::Compare { first, rest } => {
            check_expr(first, env, comprehension_bindings, types)?;
            for (_, value) in rest {
                check_expr(value, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            check_expr(object, env, comprehension_bindings, types)?;
            for value in [lower, upper, step].into_iter().flatten() {
                check_expr(value, env, comprehension_bindings, types)?;
            }
        }
        ExprKind::MultiIndex { object, args } => {
            check_expr(object, env, comprehension_bindings, types)?;
            for argument in args {
                match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        check_expr(value, env, comprehension_bindings, types)?;
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            check_expr(value, env, comprehension_bindings, types)?;
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    check_expr(value, env, comprehension_bindings, types)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Default)]
struct Env {
    scopes: Vec<HashMap<String, usize>>,
    vars: Vec<Var>,
}

impl Env {
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn declare(
        &mut self,
        name: &str,
        explicit_type: Option<String>,
        message: Option<String>,
        live: bool,
    ) {
        let id = self.vars.len();
        self.vars.push(Var {
            name: name.to_string(),
            explicit_type,
            message,
            uninitialized: false,
            obligations: if live {
                HashSet::from([Vec::new()])
            } else {
                HashSet::new()
            },
            moved: HashSet::new(),
        });
        self.scopes
            .last_mut()
            .expect("explicit-destroy scope")
            .insert(name.to_string(), id);
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn check_ids(&self, ids: impl IntoIterator<Item = usize>) -> Result<(), TypeError> {
        for id in ids {
            let var = &self.vars[id];
            if !var.obligations.is_empty()
                && let Some(message) = &var.message
            {
                if var.explicit_type.as_deref() == Some(LINEAR_TYPE_PARAMETER) {
                    return Err(TypeError::LinearAbandoned {
                        var: var.name.clone(),
                        message: message.clone(),
                    });
                }
                return Err(TypeError::Abandoned {
                    var: var.name.clone(),
                    message: message.clone(),
                });
            }
        }
        Ok(())
    }

    fn pop_checked(&mut self) -> Result<(), TypeError> {
        let scope = self.scopes.pop().expect("explicit-destroy scope");
        self.check_ids(scope.into_values())
    }

    fn check_current_scope(&self) -> Result<(), TypeError> {
        self.check_ids(
            self.scopes
                .last()
                .expect("explicit-destroy scope")
                .values()
                .copied(),
        )
    }
}

fn check_block(
    body: &[Stmt],
    mut env: Env,
    scoped: bool,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    if scoped {
        env.push();
    }
    for statement in body {
        let Some(next) = check_stmt(
            statement,
            env,
            binding_types,
            comprehension_bindings,
            deletability,
            types,
        )?
        else {
            return Ok(None);
        };
        env = next;
    }
    if scoped {
        env.pop_checked()?;
    }
    Ok(Some(env))
}

fn explicit_error<T>(var: &Var, problem: &str) -> Result<T, TypeError> {
    Err(TypeError::ExplicitDestroy {
        var: var.name.clone(),
        message: var.message.clone().unwrap_or_default(),
        problem: problem.to_string(),
    })
}

#[derive(Clone)]
struct Var {
    name: String,
    explicit_type: Option<String>,
    message: Option<String>,
    /// Consumed on a path that may have raised into the enclosing `except`
    /// arm: every use there is a use of an uninitialized value, as upstream.
    uninitialized: bool,
    /// Minimal linear subobjects that still require explicit destruction. The
    /// empty path denotes the intact whole value. Once a field is moved, that
    /// whole obligation is decomposed into its linear child fields.
    obligations: HashSet<Vec<String>>,
    moved: HashSet<Vec<String>>,
}

/// Whether `expr` is the compiler's diverging runtime trap call.
fn is_runtime_trap(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Call { name, .. } if name == "_mojito_abort")
}

fn obligation_place(expr: &Expr, env: &Env) -> Option<(usize, Vec<String>)> {
    match &expr.kind {
        ExprKind::Identifier(name) => env.lookup(name).map(|id| (id, Vec::new())),
        ExprKind::Member { object, field } => {
            let (id, mut path) = obligation_place(object, env)?;
            path.push(field.clone());
            Some((id, path))
        }
        // Dynamic indexed projections cannot be represented as stable residual
        // field obligations.
        ExprKind::Index { .. } => None,
        _ => None,
    }
}

fn join(mut exits: Vec<Env>) -> Result<Env, TypeError> {
    let Some(first) = exits.pop() else {
        return Ok(Env::default());
    };
    for other in exits {
        ensure_same(&first, &other)?;
    }
    Ok(first)
}

fn type_at_path(
    root_type: &str,
    path: &[String],
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Option<String> {
    let mut current = root_type.to_string();
    for field in path {
        current = types.get(&current)?.fields.get(field)?.clone();
    }
    Some(current)
}

fn ty_explicit_name(ty: &Ty, types: &HashMap<String, ExplicitDestroyInfo>) -> Option<String> {
    match ty {
        Ty::Struct(name, _) if types.contains_key(name) => Some(name.clone()),
        _ => None,
    }
}

fn ensure_same(before: &Env, after: &Env) -> Result<(), TypeError> {
    for (a, b) in before.vars.iter().zip(&after.vars) {
        if a.message.is_some() && (a.obligations != b.obligations || a.moved != b.moved) {
            return explicit_error(
                a,
                "was conditionally destroyed or has inconsistent residual field obligations",
            );
        }
    }
    Ok(())
}

fn check_function<'a>(
    params: impl Iterator<
        Item = (
            &'a String,
            &'a SourceType,
            Option<ArgConvention>,
            bool,
            bool,
        ),
    >,
    body: &[Stmt],
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let mut env = Env::default();
    env.push();
    for (name, ty, convention, deinitable, linear) in params {
        let explicit = if deinitable {
            None
        } else {
            source_explicit_name(ty, types)
                .or_else(|| linear.then(|| LINEAR_TYPE_PARAMETER.to_string()))
        };
        let live = explicit.is_some()
            && matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit));
        let message = explicit
            .as_ref()
            .and_then(|name| types.get(name))
            .map(|info| info.message.clone());
        env.declare(name, explicit, message, live);
    }
    let normal = check_block(
        body,
        env,
        false,
        binding_types,
        comprehension_bindings,
        deletability,
        types,
    )?;
    if let Some(env) = normal {
        env.check_ids(0..env.vars.len())?;
    }
    Ok(())
}

fn root_id(expr: &Expr, env: &Env) -> Option<usize> {
    match &expr.kind {
        ExprKind::Identifier(name) => env.lookup(name),
        ExprKind::Member { object, .. } | ExprKind::Index { object, .. } => root_id(object, env),
        _ => None,
    }
}

fn contains_index(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Index { .. } => true,
        ExprKind::Member { object, .. } => contains_index(object),
        _ => false,
    }
}

/// Replace every intact linear ancestor of `path` with its direct linear child
/// obligations. Ordinary fields do not appear in the set: after decomposition
/// they are handled by ordinary residual field dropping.
fn expose_path(
    id: usize,
    root_type: &str,
    path: &[String],
    env: &mut Env,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    for depth in 0..path.len() {
        let ancestor = path[..depth].to_vec();
        if !env.vars[id].obligations.remove(&ancestor) {
            continue;
        }
        let Some(ancestor_type) = type_at_path(root_type, &ancestor, types) else {
            return explicit_error(&env.vars[id], "has an invalid residual field obligation");
        };
        if let Some(info) = types.get(&ancestor_type) {
            for field in info.fields.keys() {
                let mut child = ancestor.clone();
                child.push(field.clone());
                env.vars[id].obligations.insert(child);
            }
        }
    }
    Ok(())
}

fn check_stmt(
    stmt: &Stmt,
    mut env: Env,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    match &stmt.kind {
        StmtKind::VarDecl { name, value, .. } => {
            check_expr(value, &mut env, comprehension_bindings, types)?;
            let explicit = if deletability.bindings.contains(&value.source_span()) {
                None
            } else {
                binding_types
                    .get(&value.source_span())
                    .and_then(|ty| ty_explicit_name(ty, types))
                    .or_else(|| {
                        deletability
                            .linear_bindings
                            .contains(&value.source_span())
                            .then(|| LINEAR_TYPE_PARAMETER.to_string())
                    })
            };
            let message = explicit
                .as_ref()
                .and_then(|name| types.get(name))
                .map(|info| info.message.clone());
            env.declare(name, explicit.clone(), message, explicit.is_some());
        }
        StmtKind::Assign { name, value } => {
            // `_ = x^` destroys the transferred value implicitly, which a
            // linear or explicit-destroy value cannot be: it is abandoned.
            if name == "_"
                && let ExprKind::Transfer(inner) = &value.kind
                && let Some((id, _)) = obligation_place(inner, &env)
                && env.vars[id].message.is_some()
            {
                env.check_ids([id])?;
            }
            check_expr(value, &mut env, comprehension_bindings, types)?;
            if let Some(id) = env.lookup(name) {
                if !env.vars[id].obligations.is_empty() && env.vars[id].message.is_some() {
                    return Err(TypeError::Abandoned {
                        var: env.vars[id].name.clone(),
                        message: env.vars[id].message.clone().unwrap_or_default(),
                    });
                }
                env.vars[id].uninitialized = false;
                env.vars[id].obligations = if env.vars[id].explicit_type.is_some() {
                    HashSet::from([Vec::new()])
                } else {
                    HashSet::new()
                };
                env.vars[id].moved.clear();
            }
        }
        StmtKind::Expr(expr) => {
            check_expr(expr, &mut env, comprehension_bindings, types)?;
            // The runtime trap (the body of an unspecialized generic template
            // stub) never returns: nothing past it is abandoned.
            if is_runtime_trap(expr) {
                return Ok(None);
            }
        }
        StmtKind::Return(expr) => {
            if let Some(expr) = expr {
                check_expr(expr, &mut env, comprehension_bindings, types)?;
            }
            env.check_ids(0..env.vars.len())?;
            return Ok(None);
        }
        StmtKind::Raise(expr) => {
            check_expr(expr, &mut env, comprehension_bindings, types)?;
            env.check_ids(0..env.vars.len())?;
            return Ok(None);
        }
        StmtKind::If { branches, orelse } => {
            let base = env.clone();
            let mut exits = Vec::new();
            for (condition, body) in branches {
                let mut branch = base.clone();
                check_expr(condition, &mut branch, comprehension_bindings, types)?;
                if let Some(exit) = check_block(
                    body,
                    branch,
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )? {
                    exits.push(exit);
                }
            }
            if let Some(body) = orelse {
                if let Some(exit) = check_block(
                    body,
                    base,
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )? {
                    exits.push(exit);
                }
            } else {
                exits.push(base);
            }
            env = join(exits)?;
        }
        StmtKind::While { cond, body, orelse } => {
            check_expr(cond, &mut env, comprehension_bindings, types)?;
            if let Some(after) = check_block(
                body,
                env.clone(),
                true,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )? {
                ensure_same(&env, &after)?;
            }
            if let Some(body) = orelse
                && let Some(after) = check_block(
                    body,
                    env.clone(),
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )?
            {
                env = after;
            }
        }
        StmtKind::For {
            var,
            iter,
            body,
            orelse,
            ..
        } => {
            check_expr(iter, &mut env, comprehension_bindings, types)?;
            let explicit = if deletability.bindings.contains(&stmt.source_span()) {
                None
            } else {
                binding_types
                    .get(&stmt.source_span())
                    .and_then(|ty| ty_explicit_name(ty, types))
                    .or_else(|| {
                        deletability
                            .linear_bindings
                            .contains(&stmt.source_span())
                            .then(|| LINEAR_TYPE_PARAMETER.to_string())
                    })
            };
            let message = explicit
                .as_ref()
                .and_then(|name| types.get(name))
                .map(|info| info.message.clone());
            let mut iteration = env.clone();
            iteration.push();
            iteration.declare(var, explicit.clone(), message, explicit.is_some());
            if let Some(mut after) = check_block(
                body,
                iteration,
                false,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )? {
                after.pop_checked()?;
                ensure_same(&env, &after)?;
            }
            if let Some(body) = orelse
                && let Some(after) = check_block(
                    body,
                    env.clone(),
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )?
            {
                env = after;
            }
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            let before = env.clone();
            let normal = check_block(
                body,
                before.clone(),
                true,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )?;
            let mut exits = Vec::new();
            if let Some(normal) = normal {
                if let Some(orelse) = orelse {
                    if let Some(out) = check_block(
                        orelse,
                        normal,
                        true,
                        binding_types,
                        comprehension_bindings,
                        deletability,
                        types,
                    )? {
                        exits.push(out);
                    }
                } else {
                    exits.push(normal);
                }
            }
            if let Some((_, handler)) = except
                && let Some(out) = check_block(
                    handler,
                    handler_entry(&before, body),
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )?
            {
                exits.push(out);
            }
            if exits.is_empty() {
                // Every route through the try/except is terminal (return,
                // raise, break, or continue).  There is no normal environment
                // to join, but the finally body must still be checked.  Use the
                // pre-try environment so its lexical scope stack remains
                // intact; constructing Env::default() here used to lose the
                // enclosing loop/function scopes and panic on their next pop.
                if let Some(finalbody) = finalbody {
                    let _ = check_block(
                        finalbody,
                        before,
                        true,
                        binding_types,
                        comprehension_bindings,
                        deletability,
                        types,
                    )?;
                }
                return Ok(None);
            }
            env = join(exits)?;
            if let Some(finalbody) = finalbody {
                let Some(out) = check_block(
                    finalbody,
                    env,
                    true,
                    binding_types,
                    comprehension_bindings,
                    deletability,
                    types,
                )?
                else {
                    return Ok(None);
                };
                env = out;
            }
        }
        StmtKind::SetPlace { place, value } => {
            check_expr(place, &mut env, comprehension_bindings, types)?;
            check_expr(value, &mut env, comprehension_bindings, types)?;
            reinitialize_place(place, &mut env, types)?;
        }
        StmtKind::AugAssign { place, value, .. } => {
            check_expr(place, &mut env, comprehension_bindings, types)?;
            check_expr(value, &mut env, comprehension_bindings, types)?;
        }
        StmtKind::Unpack { targets, value, .. } => {
            check_expr(value, &mut env, comprehension_bindings, types)?;
            for target in targets {
                check_expr(target, &mut env, comprehension_bindings, types)?;
            }
        }
        StmtKind::RefDecl { value, .. } | StmtKind::Comptime { value, .. } => {
            check_expr(value, &mut env, comprehension_bindings, types)?;
        }
        StmtKind::Break | StmtKind::Continue => {
            env.check_current_scope()?;
            return Ok(None);
        }
        // A nested function's parameters and locals carry their own
        // obligations; its captures never move the enclosing values.
        StmtKind::Def { params, body, .. } => check_def(
            stmt,
            params,
            body,
            binding_types,
            comprehension_bindings,
            deletability,
            types,
        )?,
        _ => {}
    }
    Ok(Some(env))
}

/// Check one free (top-level or nested) function definition.
fn check_def(
    statement: &Stmt,
    params: &[mojito_ast::ast::FnParam],
    body: &[Stmt],
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let params = params.iter().enumerate().map(|(param, p)| {
        let site = AnnotationSite::FunctionParam {
            module: statement.module.clone(),
            declaration: statement.span,
            syntax: statement.syntax_id,
            param,
        };
        (
            &p.name,
            &p.ty,
            p.convention,
            deletability.declarations.contains(&site),
            deletability.linear_declarations.contains(&site),
        )
    });
    check_function(
        params,
        body,
        binding_types,
        comprehension_bindings,
        deletability,
        types,
    )
}

/// Check one conceptual comprehension iteration. Each generator introduces a
/// lexical binding for the clauses to its right and for the produced key/value.
/// Explicit-destroy obligations must therefore be discharged before leaving
/// that generator's iteration, just as for an ordinary owned `for` binder.
fn check_comprehension_expr(
    expression: &Expr,
    key: Option<&Expr>,
    value: &Expr,
    clauses: &[mojito_ast::ast::ComprehensionClause],
    env: &mut Env,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let bindings = comprehension_bindings
        .get(&expression.source_span())
        .ok_or_else(|| {
            TypeError::InvariantViolation(
                "checked comprehension has no retained binder metadata".to_string(),
            )
        })?;
    let mut binding_index = 0;
    let scope_base = env.scopes.len();
    let result = (|| {
        for clause in clauses {
            match clause {
                mojito_ast::ast::ComprehensionClause::For { iter, .. } => {
                    check_expr(iter, env, comprehension_bindings, types)?;
                    let binding = bindings.get(binding_index).ok_or_else(|| {
                        TypeError::InvariantViolation(
                            "comprehension binder metadata is incomplete".to_string(),
                        )
                    })?;
                    binding_index += 1;
                    let explicit = if binding.deinitable {
                        None
                    } else {
                        ty_explicit_name(&binding.ty, types)
                    };
                    let message = explicit
                        .as_ref()
                        .and_then(|name| types.get(name))
                        .map(|info| info.message.clone());
                    env.push();
                    env.declare(&binding.name, explicit.clone(), message, explicit.is_some());
                }
                mojito_ast::ast::ComprehensionClause::If(condition) => {
                    check_expr(condition, env, comprehension_bindings, types)?;
                }
            }
        }
        if binding_index != bindings.len() {
            return Err(TypeError::InvariantViolation(
                "comprehension binder metadata has extra entries".to_string(),
            ));
        }
        if let Some(key) = key {
            check_expr(key, env, comprehension_bindings, types)?;
        }
        check_expr(value, env, comprehension_bindings, types)
    })();

    let mut cleanup = Ok(());
    while env.scopes.len() > scope_base {
        if let Err(error) = env.pop_checked() {
            cleanup = Err(error);
            break;
        }
    }
    result.and(cleanup)
}

fn move_root(
    expr: &Expr,
    env: &mut Env,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    if contains_index(expr)
        && let Some(id) = root_id(expr, env)
        && env.vars[id].explicit_type.is_some()
    {
        return explicit_error(
            &env.vars[id],
            "uses a dynamic indexed projection that cannot form a stable residual field obligation",
        );
    }
    let Some((id, path)) = obligation_place(expr, env) else {
        return Ok(());
    };
    ensure_initialized(id, env)?;
    let Some(root_type) = env.vars[id].explicit_type.clone() else {
        return Ok(());
    };
    expose_path(id, &root_type, &path, env, types)?;
    // A whole linear subobject carries its obligation to the destination. An
    // ordinary field has no entry after decomposition and needs no discharge.
    env.vars[id].obligations.remove(&path);
    env.vars[id].moved.insert(path);
    Ok(())
}

fn destructor_at_path(
    root_type: &str,
    path: &[String],
    method: &str,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> bool {
    type_at_path(root_type, path, types)
        .and_then(|name| types.get(&name))
        .is_some_and(|info| info.destructors.contains_key(method))
}

fn consume_destructor(
    id: usize,
    path: &[String],
    env: &mut Env,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let root_type = env.vars[id]
        .explicit_type
        .clone()
        .expect("linear obligation has a type");
    expose_path(id, &root_type, path, env, types)?;
    if env.vars[id]
        .moved
        .iter()
        .any(|moved| moved.starts_with(path) && moved != path)
    {
        return explicit_error(
            &env.vars[id],
            "is incomplete and cannot use a whole-value destructor",
        );
    }
    if env.vars[id].obligations.remove(path) {
        env.vars[id].moved.insert(path.to_vec());
        return Ok(());
    }
    if env.vars[id]
        .obligations
        .iter()
        .any(|obligation| obligation.starts_with(path))
    {
        return explicit_error(
            &env.vars[id],
            "is incomplete and cannot use a whole-value destructor",
        );
    }
    Err(TypeError::UninitializedUse {
        var: env.vars[id].name.clone(),
    })
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "TODO: drop the Result once callers stop using ?"
)]
fn reinitialize_place(
    expr: &Expr,
    env: &mut Env,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let Some((id, path)) = obligation_place(expr, env) else {
        return Ok(());
    };
    let Some(root_type) = env.vars[id].explicit_type.clone() else {
        return Ok(());
    };
    env.vars[id]
        .moved
        .retain(|moved| !(moved == &path || moved.starts_with(&path)));
    if env.vars[id].moved.is_empty() {
        env.vars[id].obligations.clear();
        env.vars[id].obligations.insert(Vec::new());
    } else if type_at_path(&root_type, &path, types).is_some_and(|name| types.contains_key(&name)) {
        env.vars[id].obligations.insert(path);
    }
    Ok(())
}

fn source_explicit_name(
    ty: &SourceType,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Option<String> {
    match ty {
        SourceType::Named(name, _) if types.contains_key(name) => Some(name.clone()),
        _ => None,
    }
}

/// The environment an `except` arm starts from: the pre-`try` state with
/// every linear value the body consumes marked uninitialized. Consumption
/// happens at the consuming call, so a raise on that call — or any later
/// raise — reaches the handler after the value is gone (upstream's rule).
fn handler_entry(before: &Env, body: &[Stmt]) -> Env {
    let mut consumed = HashSet::new();
    consumed_roots_in_stmts(body, before, &mut consumed);
    let mut entry = before.clone();
    for id in consumed {
        let var = &mut entry.vars[id];
        if var.message.is_some() {
            var.uninitialized = true;
            var.obligations.clear();
            var.moved.insert(Vec::new());
        }
    }
    entry
}

/// Reject a use of a value consumed on a path that may have raised.
fn ensure_initialized(id: usize, env: &Env) -> Result<bool, TypeError> {
    if env.vars[id].uninitialized {
        return Err(TypeError::UninitializedUse {
            var: env.vars[id].name.clone(),
        });
    }
    Ok(true)
}

/// Roots (indices into `env.vars`) transferred anywhere in `statements`.
fn consumed_roots_in_stmts(statements: &[Stmt], env: &Env, roots: &mut HashSet<usize>) {
    for statement in statements {
        match &statement.kind {
            StmtKind::Expr(value)
            | StmtKind::Raise(value)
            | StmtKind::Return(Some(value))
            | StmtKind::VarDecl { value, .. }
            | StmtKind::RefDecl { value, .. }
            | StmtKind::Assign { value, .. }
            | StmtKind::AugAssign { value, .. }
            | StmtKind::SetPlace { value, .. }
            | StmtKind::Comptime { value, .. } => consumed_roots_in_expr(value, env, roots),
            StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                for (condition, body) in branches {
                    consumed_roots_in_expr(condition, env, roots);
                    consumed_roots_in_stmts(body, env, roots);
                }
                if let Some(orelse) = orelse {
                    consumed_roots_in_stmts(orelse, env, roots);
                }
            }
            StmtKind::While { cond, body, orelse } => {
                consumed_roots_in_expr(cond, env, roots);
                consumed_roots_in_stmts(body, env, roots);
                if let Some(orelse) = orelse {
                    consumed_roots_in_stmts(orelse, env, roots);
                }
            }
            StmtKind::For {
                iter, body, orelse, ..
            } => {
                consumed_roots_in_expr(iter, env, roots);
                consumed_roots_in_stmts(body, env, roots);
                if let Some(orelse) = orelse {
                    consumed_roots_in_stmts(orelse, env, roots);
                }
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                consumed_roots_in_expr(iter, env, roots);
                consumed_roots_in_stmts(body, env, roots);
            }
            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                consumed_roots_in_stmts(body, env, roots);
                if let Some((_, handler)) = except {
                    consumed_roots_in_stmts(handler, env, roots);
                }
                for block in [orelse, finalbody].into_iter().flatten() {
                    consumed_roots_in_stmts(block, env, roots);
                }
            }
            _ => {}
        }
    }
}

fn consumed_roots_in_expr(expr: &Expr, env: &Env, roots: &mut HashSet<usize>) {
    match &expr.kind {
        ExprKind::Transfer(inner) => {
            if let Some((id, _)) = obligation_place(inner, env) {
                roots.insert(id);
            }
        }
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            consumed_roots_in_expr(object, env, roots);
            for arg in args {
                consumed_roots_in_expr(arg, env, roots);
            }
            for arg in kwargs {
                consumed_roots_in_expr(&arg.value, env, roots);
            }
        }
        ExprKind::Call { args, kwargs, .. } => {
            for arg in args {
                consumed_roots_in_expr(arg, env, roots);
            }
            for arg in kwargs {
                consumed_roots_in_expr(&arg.value, env, roots);
            }
        }
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => {
            consumed_roots_in_expr(callee, env, roots);
            for arg in args {
                consumed_roots_in_expr(arg, env, roots);
            }
            for arg in kwargs {
                consumed_roots_in_expr(&arg.value, env, roots);
            }
        }
        ExprKind::Prefix(_, value) | ExprKind::Named { value, .. } => {
            consumed_roots_in_expr(value, env, roots);
        }
        ExprKind::Infix(_, left, right) => {
            consumed_roots_in_expr(left, env, roots);
            consumed_roots_in_expr(right, env, roots);
        }
        ExprKind::Member { object, .. } => consumed_roots_in_expr(object, env, roots),
        ExprKind::Index { object, index } => {
            consumed_roots_in_expr(object, env, roots);
            consumed_roots_in_expr(index, env, roots);
        }
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                consumed_roots_in_expr(value, env, roots);
            }
        }
        _ => {}
    }
}
