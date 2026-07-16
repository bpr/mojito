//! Checked explicit-destruction obligations over structured source CFGs.

use crate::ast::{ArgConvention, Expr, ExprKind, SourceType, Stmt, StmtKind, TStringPart};
use crate::checked::ExplicitDestroyInfo;
use crate::error::TypeError;
use crate::token::SourceSpan;
use crate::types::Ty;
use std::collections::HashMap;

#[derive(Clone)]
struct Var {
    name: String,
    explicit_type: Option<String>,
    message: Option<String>,
    live: bool,
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
            live,
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
            if var.live
                && let Some(message) = &var.message
            {
                return Err(TypeError::ExplicitDestroy {
                    var: var.name.clone(),
                    message: message.clone(),
                    problem: "was abandoned".to_string(),
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

pub(crate) fn check(
    statements: &[Stmt],
    binding_types: &HashMap<SourceSpan, Ty>,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    if types.is_empty() {
        return Ok(());
    }
    for statement in statements {
        match &statement.kind {
            StmtKind::Def { params, body, .. } => {
                check_function(
                    params.iter().map(|p| (&p.name, &p.ty, p.convention)),
                    body,
                    binding_types,
                    types,
                )?;
            }
            StmtKind::Struct { methods, .. } => {
                for method in methods {
                    let mut params = method
                        .params
                        .iter()
                        .map(|p| (&p.name, &p.ty, p.convention))
                        .collect::<Vec<_>>();
                    if method.has_self && method.self_convention != Some(ArgConvention::Deinit) {
                        // `self` is borrowed or initialized here, never a new obligation.
                        params.retain(|_| true);
                    }
                    check_function(params.into_iter(), &method.body, binding_types, types)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_function<'a>(
    params: impl Iterator<Item = (&'a String, &'a SourceType, Option<ArgConvention>)>,
    body: &[Stmt],
    binding_types: &HashMap<SourceSpan, Ty>,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let mut env = Env::default();
    env.push();
    for (name, ty, convention) in params {
        let explicit = source_explicit_name(ty, types);
        let live = explicit.is_some()
            && matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit));
        let message = explicit
            .as_ref()
            .and_then(|name| types.get(name))
            .map(|info| info.message.clone());
        env.declare(name, explicit, message, live);
    }
    let normal = check_block(body, env, false, binding_types, types)?;
    if let Some(env) = normal {
        env.check_ids(0..env.vars.len())?;
    }
    Ok(())
}

fn check_block(
    body: &[Stmt],
    mut env: Env,
    scoped: bool,
    binding_types: &HashMap<SourceSpan, Ty>,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    if scoped {
        env.push();
    }
    for statement in body {
        let Some(next) = check_stmt(statement, env, binding_types, types)? else {
            return Ok(None);
        };
        env = next;
    }
    if scoped {
        env.pop_checked()?;
    }
    Ok(Some(env))
}

fn check_stmt(
    stmt: &Stmt,
    mut env: Env,
    binding_types: &HashMap<SourceSpan, Ty>,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    match &stmt.kind {
        StmtKind::VarDecl { name, value, .. } => {
            check_expr(value, &mut env, types)?;
            let explicit = binding_types
                .get(&value.source_span())
                .and_then(|ty| ty_explicit_name(ty, types));
            let message = explicit
                .as_ref()
                .and_then(|name| types.get(name))
                .map(|info| info.message.clone());
            env.declare(name, explicit.clone(), message, explicit.is_some());
        }
        StmtKind::Assign { name, value } => {
            check_expr(value, &mut env, types)?;
            if let Some(id) = env.lookup(name) {
                if env.vars[id].live && env.vars[id].message.is_some() {
                    return explicit_error(&env.vars[id], "was overwritten");
                }
                env.vars[id].live = env.vars[id].explicit_type.is_some();
            }
        }
        StmtKind::Expr(expr) => check_expr(expr, &mut env, types)?,
        StmtKind::Return(expr) => {
            if let Some(expr) = expr {
                check_expr(expr, &mut env, types)?;
            }
            env.check_ids(0..env.vars.len())?;
            return Ok(None);
        }
        StmtKind::Raise(expr) => {
            check_expr(expr, &mut env, types)?;
            env.check_ids(0..env.vars.len())?;
            return Ok(None);
        }
        StmtKind::If { branches, orelse } => {
            let base = env.clone();
            let mut exits = Vec::new();
            for (condition, body) in branches {
                let mut branch = base.clone();
                check_expr(condition, &mut branch, types)?;
                if let Some(exit) = check_block(body, branch, true, binding_types, types)? {
                    exits.push(exit);
                }
            }
            if let Some(body) = orelse {
                if let Some(exit) = check_block(body, base.clone(), true, binding_types, types)? {
                    exits.push(exit);
                }
            } else {
                exits.push(base);
            }
            env = join(exits)?;
        }
        StmtKind::While { cond, body } => {
            check_expr(cond, &mut env, types)?;
            if let Some(after) = check_block(body, env.clone(), true, binding_types, types)? {
                ensure_same(&env, &after)?;
            }
        }
        StmtKind::For { iter, body, .. } => {
            check_expr(iter, &mut env, types)?;
            if let Some(after) = check_block(body, env.clone(), true, binding_types, types)? {
                ensure_same(&env, &after)?;
            }
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            let before = env.clone();
            let normal = check_block(body, before.clone(), true, binding_types, types)?;
            let mut exits = Vec::new();
            if let Some(normal) = normal {
                if let Some(orelse) = orelse {
                    if let Some(out) = check_block(orelse, normal, true, binding_types, types)? {
                        exits.push(out);
                    }
                } else {
                    exits.push(normal);
                }
            }
            if let Some((_, handler)) = except
                && let Some(out) = check_block(handler, before, true, binding_types, types)?
            {
                exits.push(out);
            }
            env = join(exits)?;
            if let Some(finalbody) = finalbody {
                let Some(out) = check_block(finalbody, env, true, binding_types, types)? else {
                    return Ok(None);
                };
                env = out;
            }
        }
        StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
            check_expr(place, &mut env, types)?;
            check_expr(value, &mut env, types)?;
        }
        StmtKind::Unpack { targets, value } => {
            check_expr(value, &mut env, types)?;
            for target in targets {
                check_expr(target, &mut env, types)?;
            }
        }
        StmtKind::RefDecl { value, .. } | StmtKind::Comptime { value, .. } => {
            check_expr(value, &mut env, types)?
        }
        StmtKind::Break | StmtKind::Continue => {
            env.check_current_scope()?;
            return Ok(None);
        }
        _ => {}
    }
    Ok(Some(env))
}

fn check_expr(
    expr: &Expr,
    env: &mut Env,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    match &expr.kind {
        ExprKind::Transfer(inner) => move_root(inner, env)?,
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            for arg in args {
                check_expr(arg, env, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, types)?;
            }
            if let ExprKind::Transfer(inner) = &object.kind
                && let Some(id) = root_id(inner, env)
                && let Some(type_name) = &env.vars[id].explicit_type
                && types
                    .get(type_name)
                    .is_some_and(|info| info.destructors.contains_key(method))
            {
                if !env.vars[id].live {
                    return explicit_error(&env.vars[id], "was destroyed more than once");
                }
                env.vars[id].live = false;
            } else {
                check_expr(object, env, types)?;
            }
        }
        ExprKind::Call { args, kwargs, .. } => {
            for arg in args {
                check_expr(arg, env, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, types)?;
            }
        }
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => {
            check_expr(callee, env, types)?;
            for arg in args {
                check_expr(arg, env, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, types)?;
            }
        }
        ExprKind::Prefix(_, value) | ExprKind::Named { value, .. } => {
            check_expr(value, env, types)?
        }
        ExprKind::Infix(_, left, right) => {
            check_expr(left, env, types)?;
            check_expr(right, env, types)?;
        }
        ExprKind::Member { object, .. } => check_expr(object, env, types)?,
        ExprKind::Index { object, index } => {
            check_expr(object, env, types)?;
            check_expr(index, env, types)?;
        }
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                check_expr(value, env, types)?;
            }
        }
        ExprKind::BraceLit(values) => {
            for (key, value) in values {
                check_expr(key, env, types)?;
                if let Some(value) = value {
                    check_expr(value, env, types)?;
                }
            }
        }
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            check_expr(cond, env, types)?;
            let mut a = env.clone();
            let mut b = env.clone();
            check_expr(then_branch, &mut a, types)?;
            check_expr(else_branch, &mut b, types)?;
            *env = join(vec![a, b])?;
        }
        ExprKind::Compare { first, rest } => {
            check_expr(first, env, types)?;
            for (_, value) in rest {
                check_expr(value, env, types)?;
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
        } => {
            check_expr(object, env, types)?;
            for value in [lower, upper, step].into_iter().flatten() {
                check_expr(value, env, types)?;
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    check_expr(value, env, types)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn move_root(expr: &Expr, env: &mut Env) -> Result<(), TypeError> {
    if let Some(id) = root_id(expr, env)
        && env.vars[id].explicit_type.is_some()
    {
        if !env.vars[id].live {
            return explicit_error(&env.vars[id], "was moved or destroyed more than once");
        }
        if !matches!(expr.kind, ExprKind::Identifier(_)) {
            return explicit_error(&env.vars[id], "was partially moved");
        }
        env.vars[id].live = false;
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
fn ty_explicit_name(ty: &Ty, types: &HashMap<String, ExplicitDestroyInfo>) -> Option<String> {
    match ty {
        Ty::Struct(name, _) if types.contains_key(name) => Some(name.clone()),
        _ => None,
    }
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
fn explicit_error<T>(var: &Var, problem: &str) -> Result<T, TypeError> {
    Err(TypeError::ExplicitDestroy {
        var: var.name.clone(),
        message: var.message.clone().unwrap_or_default(),
        problem: problem.to_string(),
    })
}

fn ensure_same(before: &Env, after: &Env) -> Result<(), TypeError> {
    for (a, b) in before.vars.iter().zip(&after.vars) {
        if a.message.is_some() && a.live != b.live {
            return explicit_error(a, "was conditionally destroyed");
        }
    }
    Ok(())
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
