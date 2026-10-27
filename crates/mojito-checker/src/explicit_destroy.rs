//! Checked explicit-destruction obligations over structured source CFGs.

use crate::checker::DeclarationTable;
use mojito_ast::ast::{
    ArgConvention, Condition, Expr, ExprKind, SourceType, Stmt, StmtKind, TStringPart,
};
use mojito_checked::checked::{AnnotationSite, ExplicitDestroyInfo, NodeFacts};
use mojito_checked::fact_store::FactMap;
use mojito_common::error::TypeError;
use mojito_common::token::SourceSpan;
use std::collections::HashMap;
use std::collections::HashSet;

/// The destruction verdicts the type checker recorded for owned values.
///
/// Each is decided in the lexical constraint environment where the value is
/// introduced (the checker's `destruction_verdict`): a value absent here is
/// `Deinitable` there.
///
/// Conditional conformances cannot be recovered from a nominal type name after
/// checking: `List[T]` is linear in general, while it is ordinarily droppable
/// under a proven `T: Deinitable` constraint.
#[derive(Debug, Default)]
pub struct CheckedDeletability {
    /// Parameter declarations whose type is not `Deinitable` there.
    pub declarations: FactMap<AnnotationSite, Linear>,
    /// Bindings (`var`, `for`, a comprehension's generator) whose type is
    /// not `Deinitable` there.
    pub bindings: FactMap<SourceSpan, Linear>,
}

/// Why a value cannot be implicitly destroyed, upstream's
/// `SpecialMemberInfo::unavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linear {
    /// The denying message: an archetype's first linear trait's, or the
    /// struct's.
    pub message: String,
    /// An archetype (a type parameter, an associated projection, an opaque
    /// pack element): its diagnostic carries upstream's `Deinitable` note.
    pub archetype: bool,
    /// The struct whose explicit-destroy entry names its destructors and
    /// linear fields; `None` for an archetype.
    pub destroy_type: Option<String>,
}

/// Per-expression facts the checker recorded for one program, carried on
/// every environment this analysis builds.
#[derive(Default)]
pub struct SpanFacts {
    /// Spans of `^` transfers the checker bound to a read parameter or
    /// receiver: they lend their place and consume nothing.
    pub lent: HashSet<SourceSpan>,
    /// Temporaries the enclosing body owns but cannot destroy, in a position
    /// that takes no ownership of them, each with its verdict.
    pub linear_temporaries: HashMap<SourceSpan, Linear>,
    /// Linear temporaries in a position that takes ownership of them: each
    /// is owned in flight from its evaluation until the operation that
    /// consumes it.
    pub owned_temporaries: HashMap<SourceSpan, Linear>,
    /// The linear binders of comprehension generators, keyed by their
    /// iterable, as [`CheckedDeletability::bindings`] records them.
    pub generators: HashMap<SourceSpan, Linear>,
    /// Occurrences that may raise: a raising call, operator, conversion, or
    /// `__iter__`. Each is a raise edge of its body.
    pub raise_points: HashSet<SourceSpan>,
}

/// Check `statements`, and each trait member of `members` (a `def` whose
/// first argument is its trait's receiver).
pub(crate) fn check(
    statements: &[Stmt],
    members: &[Stmt],
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
    spans: SpanFacts,
) -> Result<(), TypeError> {
    if types.is_empty()
        && deletability.declarations.is_empty()
        && deletability.bindings.is_empty()
        && spans.linear_temporaries.is_empty()
        && spans.owned_temporaries.is_empty()
    {
        return Ok(());
    }
    let root = Env {
        spans: std::rc::Rc::new(spans),
        ..Env::default()
    };
    for statement in statements {
        match &statement.kind {
            StmtKind::Def { .. } => {
                check_def(statement, 0, node_facts, deletability, types, &root)?;
            }
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } => {
                let owner = root.function_env(type_params);
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
                                p.convention,
                                deletability.declarations.get(&site).cloned(),
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
                        node_facts,
                        deletability,
                        types,
                        owner.function_env(&method.type_params),
                    )?;
                }
            }
            _ => {}
        }
    }
    // A trait member's receiver is a method receiver, which owes nothing
    // here, as a struct method's does not.
    for member in members {
        check_def(member, 1, node_facts, deletability, types, &root)?;
    }
    Ok(())
}

/// Check one expression in evaluation order. An operation consumes the owned
/// operands evaluated under it once they all ran, and only then may raise,
/// so a raise between an operand and its operation abandons the operand.
fn check_expr(
    expr: &Expr,
    env: &mut Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let span = expr.source_span();
    if let Some(linear) = env.spans.linear_temporaries.get(&span) {
        return Err(abandoned(TEMPORARY, linear));
    }
    let baseline = (env.in_flight.len(), env.pending.len());
    check_operands(expr, env, node_facts, types)?;
    if consumes_operands(expr) {
        consume_operands(env, baseline, types)?;
    }
    if env.spans.raise_points.contains(&span) {
        raise_edge(env)?;
    }
    if let Some(linear) = env.spans.owned_temporaries.get(&span) {
        let linear = linear.clone();
        env.in_flight.push(linear);
    }
    Ok(())
}

/// Whether `expr` is an operation that takes its operands: one whose owned
/// operands are consumed by it rather than passed through to an enclosing
/// operation.
const fn consumes_operands(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Invoke { .. }
            | ExprKind::Prefix(..)
            | ExprKind::Infix(..)
            | ExprKind::Compare { .. }
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
            | ExprKind::MultiIndex { .. }
            | ExprKind::ListLit(_)
            | ExprKind::TupleLit(_)
            | ExprKind::BraceLit(_)
            | ExprKind::Comprehension { .. }
            | ExprKind::TString { .. }
    )
}

/// Walk `expr`'s operands, upstream's evaluation order: a method call's
/// receiver before its arguments.
fn check_operands(
    expr: &Expr,
    env: &mut Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    match &expr.kind {
        ExprKind::Transfer(inner) if env.spans.lent.contains(&expr.source_span()) => {
            check_expr(inner, env, node_facts, types)?;
        }
        // A transferred temporary (`make(x^)^.close()`) is no place of its
        // own; the transfers inside it still move theirs.
        ExprKind::Transfer(inner) if root_id(inner, env).is_none() => {
            check_expr(inner, env, node_facts, types)?;
        }
        ExprKind::Transfer(inner) => {
            if let Some(place) = transferred_place(inner, env)? {
                env.pending.push(place);
            }
        }
        // A named destructor (`x^.close()`) consumes its receiver at the
        // call, after its arguments.
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => {
            let destructor = match &object.kind {
                ExprKind::Transfer(inner) => obligation_place(inner, env)
                    .filter(|(id, _)| !env.vars[*id].uninitialized)
                    .filter(|(id, path)| {
                        env.vars[*id].linear.as_ref().is_some_and(|linear| {
                            destructor_at_path(linear.destroy_type.as_deref(), path, method, types)
                        })
                    }),
                _ => None,
            };
            if destructor.is_none() {
                check_expr(object, env, node_facts, types)?;
            }
            for arg in args {
                check_expr(arg, env, node_facts, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, node_facts, types)?;
            }
            if let Some((id, path)) = destructor {
                consume_destructor(id, &path, env, types)?;
            }
        }
        ExprKind::Call { args, kwargs, .. } => {
            for arg in args {
                check_expr(arg, env, node_facts, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, node_facts, types)?;
            }
        }
        ExprKind::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => {
            check_expr(callee, env, node_facts, types)?;
            for arg in args {
                check_expr(arg, env, node_facts, types)?;
            }
            for arg in kwargs {
                check_expr(&arg.value, env, node_facts, types)?;
            }
        }
        ExprKind::Prefix(_, value) => check_expr(value, env, node_facts, types)?,
        // A walrus writes its target as `=` does: a name is not read, a
        // place's subexpressions are.
        ExprKind::Named { target, value } => {
            if !matches!(target.kind, ExprKind::Identifier(_)) {
                check_expr(target, env, node_facts, types)?;
            }
            check_expr(value, env, node_facts, types)?;
            reinitialize_place(target, env, types)?;
        }
        ExprKind::Infix(_, left, right) => {
            check_expr(left, env, node_facts, types)?;
            check_expr(right, env, node_facts, types)?;
        }
        ExprKind::Member { object, .. } => check_expr(object, env, node_facts, types)?,
        ExprKind::Index { object, index } => {
            check_expr(object, env, node_facts, types)?;
            check_expr(index, env, node_facts, types)?;
        }
        ExprKind::Identifier(name) => {
            if let Some(id) = env.lookup(name) {
                ensure_initialized(id, env)?;
            }
        }
        ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
            for value in values {
                check_expr(value, env, node_facts, types)?;
            }
        }
        ExprKind::BraceLit(values) => {
            for (key, value) in values {
                check_expr(key, env, node_facts, types)?;
                if let Some(value) = value {
                    check_expr(value, env, node_facts, types)?;
                }
            }
        }
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            check_comprehension_expr(expr, key.as_deref(), value, clauses, env, node_facts, types)?;
        }
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            check_expr(cond, env, node_facts, types)?;
            let mut a = env.clone();
            let mut b = env.clone();
            check_expr(then_branch, &mut a, node_facts, types)?;
            check_expr(else_branch, &mut b, node_facts, types)?;
            *env = join(vec![a, b])?;
        }
        ExprKind::Compare { first, rest } => {
            check_expr(first, env, node_facts, types)?;
            for (_, value) in rest {
                check_expr(value, env, node_facts, types)?;
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            check_expr(object, env, node_facts, types)?;
            for value in [lower, upper, step].into_iter().flatten() {
                check_expr(value, env, node_facts, types)?;
            }
        }
        ExprKind::MultiIndex { object, args } => {
            check_expr(object, env, node_facts, types)?;
            for argument in args {
                match argument {
                    mojito_ast::ast::SubscriptArg::Index(value)
                    | mojito_ast::ast::SubscriptArg::Keyword { value, .. } => {
                        check_expr(value, env, node_facts, types)?;
                    }
                    mojito_ast::ast::SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | mojito_ast::ast::SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            check_expr(value, env, node_facts, types)?;
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    check_expr(value, env, node_facts, types)?;
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
    spans: std::rc::Rc<SpanFacts>,
    /// Names of the compile-time parameters in scope: the enclosing
    /// declarations' own and the owning struct's.
    params: std::rc::Rc<HashSet<String>>,
    /// The verdicts of the owned linear temporaries evaluated but not yet
    /// consumed by their operation.
    in_flight: Vec<Linear>,
    /// Places transferred (`x^`) but not yet consumed by their operation.
    pending: Vec<(usize, Vec<String>)>,
    /// The enclosing `try` regions a raise reaches, innermost last.
    frames: Vec<RaiseFrame>,
}

/// A `try` region's raise target: its `except` arm, or, with none, its
/// `finally` body. Every raise edge inside the region records the state it
/// reaches the target with, and the target starts from their join.
#[derive(Clone)]
struct RaiseFrame {
    /// `scopes.len()` at the region's entry: the scopes from here on end
    /// with the raise.
    depth: usize,
    /// `vars.len()` at the region's entry.
    vars: usize,
    entries: std::rc::Rc<std::cell::RefCell<Vec<Env>>>,
}

impl Env {
    /// The entry environment of a declaration nested in this one: no
    /// bindings, the same span facts, and `type_params` added to the
    /// parameters in scope.
    fn function_env(&self, type_params: &[mojito_ast::ast::TypeParam]) -> Self {
        let mut params = (*self.params).clone();
        params.extend(
            type_params
                .iter()
                .map(|param| param.name.trim_start_matches('*').to_string()),
        );
        Self {
            spans: std::rc::Rc::clone(&self.spans),
            params: std::rc::Rc::new(params),
            ..Self::default()
        }
    }

    /// Whether a `comptime if` condition names a parameter in scope, so that
    /// which arm it selects differs between instantiations.
    fn is_parametric(&self, condition: &Expr) -> bool {
        let mut finder = ParamMention {
            params: &self.params,
            found: false,
        };
        mojito_ast::visit::walk_expr(&mut finder, condition);
        finder.found
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// A raise target for a `try` region entered here.
    fn raise_frame(&self) -> RaiseFrame {
        RaiseFrame {
            depth: self.scopes.len(),
            vars: self.vars.len(),
            entries: std::rc::Rc::default(),
        }
    }

    fn declare(&mut self, name: &str, linear: Option<Linear>, live: bool) {
        let id = self.vars.len();
        self.vars.push(Var {
            name: name.to_string(),
            linear,
            uninitialized: false,
            by_parts: false,
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
                && let Some(linear) = &var.linear
            {
                return Err(abandoned(&var.name, linear));
            }
        }
        Ok(())
    }

    /// [`Self::check_ids`] on a raise edge.
    fn check_raised(&self, ids: impl IntoIterator<Item = usize>) -> Result<(), TypeError> {
        for id in ids {
            let var = &self.vars[id];
            if !var.obligations.is_empty()
                && let Some(linear) = &var.linear
            {
                return Err(abandoned_on_raise(&var.name, linear));
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
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    if scoped {
        env.push();
    }
    for statement in body {
        // A kept compile-time block runs once, straight through, and its locals'
        // obligations end with it.
        let next = match &statement.kind {
            StmtKind::Scope(block) => {
                check_block(block, env, true, node_facts, deletability, types)?
            }
            _ => check_stmt(statement, env, node_facts, deletability, types)?,
        };
        let Some(next) = next else {
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
        message: var
            .linear
            .as_ref()
            .map(|linear| linear.message.clone())
            .unwrap_or_default(),
        problem: problem.to_string(),
    })
}

#[derive(Clone)]
struct Var {
    name: String,
    /// Why the value cannot be implicitly destroyed; `None` when it is
    /// `Deinitable` and owes nothing.
    linear: Option<Linear>,
    /// Consumed on a path that may have raised into the enclosing `except`
    /// arm: every use there is a use of an uninitialized value, as upstream.
    uninitialized: bool,
    /// A `deinit` parameter the callee tears down by parts
    /// ([`marks_storage_destroyed`]), as it does a `deinit self` receiver.
    by_parts: bool,
    /// Minimal linear subobjects that still require explicit destruction. The
    /// empty path denotes the intact whole value. Once a field is moved, that
    /// whole obligation is decomposed into its linear child fields.
    obligations: HashSet<Vec<String>>,
    moved: HashSet<Vec<String>>,
}

/// The description of an expression temporary in a diagnostic.
const TEMPORARY: &str = "(expression temporary)";

/// The error for leaving `var`, which `linear` describes, undischarged.
fn abandoned(var: &str, linear: &Linear) -> TypeError {
    let (var, message) = (var.to_string(), linear.message.clone());
    if linear.archetype {
        TypeError::LinearAbandoned { var, message }
    } else {
        TypeError::Abandoned { var, message }
    }
}

/// The error for a raise edge leaving `var`, which `linear` describes,
/// undischarged.
fn abandoned_on_raise(var: &str, linear: &Linear) -> TypeError {
    TypeError::AbandonedOnRaise {
        var: var.to_string(),
        message: linear.message.clone(),
        linear: linear.archetype,
    }
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

/// Join the arms of a `comptime if` whose condition names a parameter in
/// scope. An arm may assume nothing about the instantiation, so the arms join
/// as the branches of an `if` do, whichever of them the program's
/// instantiations select. A value destroyed in one arm only is left
/// conditionally initialized: a later use of it is a use of an uninitialized
/// value, and with no later use it is abandoned at the end of its scope.
///
/// No exit at all means every arm is terminal (`return`, `raise`, `break`,
/// `continue`): there is no environment to carry on with, and the caller
/// stops walking the block.
fn join_parametric(mut exits: Vec<Env>) -> Option<Env> {
    let mut joined = exits.pop()?;
    for other in &exits {
        for (var, arm) in joined.vars.iter_mut().zip(&other.vars) {
            if var.linear.is_some()
                && (var.obligations != arm.obligations || var.moved != arm.moved)
            {
                var.obligations.extend(arm.obligations.iter().cloned());
                var.moved.retain(|path| arm.moved.contains(path));
                var.uninitialized = true;
            }
        }
    }
    Some(joined)
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

/// The exits of an `if`'s branches. Every branch runs from the entry
/// environment, a diverging one contributes no exit, and a missing `else`
/// contributes the entry environment itself — so a value consumed on one
/// branch only is conditionally destroyed.
fn branch_exits(
    branches: &[(Condition, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Vec<Env>, TypeError> {
    let mut exits = Vec::new();
    for (condition, body) in branches {
        let mut branch = env.clone();
        check_expr(condition.value(), &mut branch, node_facts, types)?;
        finish_statement(&mut branch, types)?;
        if let Some(exit) = check_block(body, branch, true, node_facts, deletability, types)? {
            exits.push(exit);
        }
    }
    match orelse {
        Some(body) => {
            if let Some(exit) = check_block(body, env, true, node_facts, deletability, types)? {
                exits.push(exit);
            }
        }
        None => exits.push(env),
    }
    Ok(exits)
}

/// Check a runtime `if`: every branch runs from the entry environment and
/// their exits must agree (`join`).
///
/// `None` means every branch is terminal, so nothing follows the statement.
fn check_if(
    branches: &[(Condition, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    let exits = branch_exits(branches, orelse, env, node_facts, deletability, types)?;
    if exits.is_empty() {
        return Ok(None);
    }
    join(exits).map(Some)
}

/// Check a `comptime if`. A condition naming a parameter in scope selects a
/// different arm per instantiation, and the arms join as branches
/// (`join_parametric`). Any other condition folds to one arm before this
/// analysis would run on the elaborated body; which one is not known here, so
/// the arms are alternatives (`join_comptime`), which can miss an abandonment
/// but never reports one the taken arm does not have.
///
/// `None` means every arm is terminal, so nothing follows the statement.
fn check_comptime_if(
    branches: &[(Condition, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    let parametric = branches
        .iter()
        .any(|(condition, _)| env.is_parametric(condition.value()));
    let mut exits = branch_exits(
        branches,
        orelse,
        env.clone(),
        node_facts,
        deletability,
        types,
    )?;
    if parametric {
        return Ok(join_parametric(exits));
    }
    exits.push(env);
    Ok(Some(join_comptime(exits)))
}

/// The blocks of a `try` statement.
struct TryBlocks<'a> {
    body: &'a [Stmt],
    except: Option<&'a [Stmt]>,
    orelse: Option<&'a [Stmt]>,
    finalbody: Option<&'a [Stmt]>,
}

/// Check a `try` statement. The body raises into its `except` arm, or with
/// none into its `finally` body, and the arm and the `else` block raise into
/// the `finally` body. A raise target starts from the join of the raise
/// edges that reach it (`join_parametric`), and one no edge reaches is
/// unreachable and unchecked. A raise continues from the end of the
/// `finally` body to the enclosing target.
///
/// `None` means every route through the statement is terminal.
fn check_try(
    blocks: &TryBlocks<'_>,
    env: Env,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    let with_frames = |mut env: Env, frames: &[RaiseFrame]| {
        env.frames = frames.to_vec();
        env
    };
    let outer = env.frames.clone();
    let finally = blocks.finalbody.map(|_| env.raise_frame());
    let handler = blocks.except.map(|_| env.raise_frame());
    let mut after_body = outer.clone();
    after_body.extend(finally.clone());
    let mut region = env.clone();
    region
        .frames
        .extend(finally.iter().chain(&handler).cloned());
    let mut normal = Vec::new();
    if let Some(exit) = check_block(blocks.body, region, true, node_facts, deletability, types)? {
        let exit = with_frames(exit, &after_body);
        match blocks.orelse {
            Some(orelse) => {
                normal.extend(check_block(
                    orelse,
                    exit,
                    true,
                    node_facts,
                    deletability,
                    types,
                )?);
            }
            None => normal.push(exit),
        }
    }
    let mut raised = Vec::new();
    if let (Some(frame), Some(arm)) = (&handler, blocks.except)
        && let Some(entry) = join_parametric(frame.entries.take())
    {
        let entry = with_frames(entry, &after_body);
        raised.extend(check_block(
            arm,
            entry,
            true,
            node_facts,
            deletability,
            types,
        )?);
    }
    if let (Some(frame), Some(finalbody)) = (&finally, blocks.finalbody)
        && let Some(entry) = join_parametric(frame.entries.take())
        && let Some(exit) = check_block(
            finalbody,
            with_frames(entry, &outer),
            true,
            node_facts,
            deletability,
            types,
        )?
    {
        raise_edge(&exit)?;
    }
    let Some(joined) = join_try(normal, raised)? else {
        // Every route is terminal (return, raise, break, or continue), but
        // the `finally` body still runs: check it from the entry state, so
        // the enclosing scope stack stays intact.
        if let Some(finalbody) = blocks.finalbody {
            check_block(finalbody, env, true, node_facts, deletability, types)?;
        }
        return Ok(None);
    };
    let joined = with_frames(joined, &outer);
    match blocks.finalbody {
        Some(finalbody) => check_block(finalbody, joined, true, node_facts, deletability, types),
        None => Ok(Some(joined)),
    }
}

/// Join a `try`'s normal exits with its `except` arm's. A value the normal
/// routes consumed but the arm kept was not consumed where the error was
/// thrown, which upstream reports on the raise edge. Any other value whose
/// routes disagree is left conditionally initialized (`join_parametric`).
fn join_try(normal: Vec<Env>, raised: Vec<Env>) -> Result<Option<Env>, TypeError> {
    for arm in &raised {
        for exit in &normal {
            for (kept, consumed) in arm.vars.iter().zip(&exit.vars) {
                if !kept.obligations.is_empty()
                    && consumed.obligations.is_empty()
                    && let Some(linear) = &kept.linear
                {
                    return Err(abandoned_on_raise(&kept.name, linear));
                }
            }
        }
    }
    Ok(join_parametric(normal.into_iter().chain(raised).collect()))
}

/// Join the alternatives of a `comptime if` that folds without the parameters
/// in scope. Exactly one is taken, in every instantiation alike, so an
/// obligation any alternative discharges is discharged: the result keeps
/// only the obligations every alternative kept. The entry environment is the
/// last exit, so it carries the scope stack and the smallest `vars` arena.
fn join_comptime(mut exits: Vec<Env>) -> Env {
    let Some(mut joined) = exits.pop() else {
        return Env::default();
    };
    for other in &exits {
        for (id, var) in joined.vars.iter_mut().enumerate() {
            let Some(alternative) = other.vars.get(id) else {
                break;
            };
            var.obligations
                .retain(|path| alternative.obligations.contains(path));
            var.moved.extend(alternative.moved.iter().cloned());
            var.uninitialized &= alternative.uninitialized;
        }
    }
    joined
}

/// Finds a name of a compile-time parameter in scope, in expression or type
/// position (`T`, `Self.T`, `List[T]`).
struct ParamMention<'a> {
    params: &'a HashSet<String>,
    found: bool,
}

impl mojito_ast::visit::Visitor for ParamMention<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        self.found |= match &expr.kind {
            ExprKind::Identifier(name) => self.params.contains(name.trim_start_matches('*')),
            ExprKind::Member { object, field } => {
                matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")
                    && self.params.contains(field)
            }
            _ => false,
        };
    }

    fn visit_type(&mut self, ty: &SourceType) {
        self.found |= match ty {
            SourceType::Named(name, _) | SourceType::SelfParam(name) => {
                self.params.contains(name.trim_start_matches('*'))
            }
            _ => false,
        };
    }
}

/// The struct at `path` beneath a value of the struct `root_type`; an
/// archetype has none.
fn type_at_path(
    root_type: Option<&str>,
    path: &[String],
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Option<String> {
    let mut current = root_type?.to_string();
    for field in path {
        current = types.get(&current)?.fields.get(field)?.clone();
    }
    Some(current)
}

fn ensure_same(before: &Env, after: &Env) -> Result<(), TypeError> {
    for (a, b) in before.vars.iter().zip(&after.vars) {
        if a.linear.is_some() && (a.obligations != b.obligations || a.moved != b.moved) {
            return explicit_error(
                a,
                "was conditionally destroyed or has inconsistent residual field obligations",
            );
        }
    }
    Ok(())
}

fn check_function<'a>(
    params: impl Iterator<Item = (&'a String, Option<ArgConvention>, Option<Linear>)>,
    body: &[Stmt],
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
    mut env: Env,
) -> Result<(), TypeError> {
    env.push();
    for (name, convention, linear) in params {
        // A `deinit` parameter whose storage the body marks destroyed is
        // torn down by parts, as a `deinit self` receiver is, so it owes no
        // whole-value destruction.
        let by_parts =
            convention == Some(ArgConvention::Deinit) && marks_storage_destroyed(body, name);
        let live = linear.is_some()
            && !by_parts
            && matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit));
        env.declare(name, linear, live);
        if let Some(parameter) = env.vars.last_mut() {
            parameter.by_parts = by_parts;
        }
    }
    let normal = check_block(body, env, false, node_facts, deletability, types)?;
    if let Some(env) = normal {
        env.check_ids(0..env.vars.len())?;
    }
    Ok(())
}

/// Whether `body` marks storage of the binding `name` destroyed.
///
/// That is upstream's `lit.ownership.mark_destroyed` statement over
/// `__get_mvalue_as_litref(other.storage)`: the teardown by parts the
/// bundled private-storage structs spell after moving each element out
/// through a pointer.
fn marks_storage_destroyed(body: &[Stmt], name: &str) -> bool {
    struct Finder<'a> {
        name: &'a str,
        found: bool,
    }
    impl mojito_ast::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            let ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } = &expr.kind
            else {
                return;
            };
            if !matches!(&object.kind, ExprKind::Identifier(op) if op == "__mlir_op")
                || method != "lit.ownership.mark_destroyed"
            {
                return;
            }
            let [
                Expr {
                    kind: ExprKind::Call { args: operand, .. },
                    ..
                },
            ] = args.as_slice()
            else {
                return;
            };
            let Some(mut root) = operand.first() else {
                return;
            };
            while let ExprKind::Member { object, .. } = &root.kind {
                root = object;
            }
            self.found |= matches!(&root.kind, ExprKind::Identifier(root) if root == self.name);
        }
    }
    let mut finder = Finder { name, found: false };
    mojito_ast::visit::walk_block(&mut finder, body);
    finder.found
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
    root_type: Option<&str>,
    path: &[String],
    env: &mut Env,
    types: &DeclarationTable<ExplicitDestroyInfo>,
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
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    match &stmt.kind {
        StmtKind::VarDecl { name, value, .. } => {
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            let linear = deletability.bindings.get(&value.source_span()).cloned();
            let live = linear.is_some();
            env.declare(name, linear, live);
        }
        StmtKind::Assign { name, value } => {
            // `_ = x^` destroys the transferred value implicitly, which a
            // linear or explicit-destroy value cannot be: it is abandoned.
            if name == "_"
                && let ExprKind::Transfer(inner) = &value.kind
                && let Some((id, _)) = obligation_place(inner, &env)
                && env.vars[id].linear.is_some()
            {
                env.check_ids([id])?;
            }
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            if let Some(id) = env.lookup(name) {
                if !env.vars[id].obligations.is_empty()
                    && let Some(linear) = &env.vars[id].linear
                {
                    return Err(abandoned(&env.vars[id].name, linear));
                }
                env.vars[id].uninitialized = false;
                env.vars[id].obligations = if env.vars[id].linear.is_some() {
                    HashSet::from([Vec::new()])
                } else {
                    HashSet::new()
                };
                env.vars[id].moved.clear();
            }
        }
        StmtKind::Expr(expr) => {
            check_expr(expr, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            // The runtime trap (the body of an unspecialized generic template
            // stub) never returns: nothing past it is abandoned.
            if is_runtime_trap(expr) {
                return Ok(None);
            }
        }
        StmtKind::Return(expr) => {
            if let Some(expr) = expr {
                check_expr(expr, &mut env, node_facts, types)?;
                finish_statement(&mut env, types)?;
            }
            env.check_ids(0..env.vars.len())?;
            return Ok(None);
        }
        StmtKind::Raise(expr) => {
            check_expr(expr, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            raise_edge(&env)?;
            return Ok(None);
        }
        // The same branch walk, joined by each form's own rule; no exit at
        // all means every branch is terminal.
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            let check = if matches!(stmt.kind, StmtKind::ComptimeIf { .. }) {
                check_comptime_if
            } else {
                check_if
            };
            let Some(next) = check(
                branches,
                orelse.as_deref(),
                env,
                node_facts,
                deletability,
                types,
            )?
            else {
                return Ok(None);
            };
            env = next;
        }
        // The loop unrolls per instantiation, possibly to nothing or to
        // several copies, so its body must leave every outer obligation as it
        // found it, as a `while` body must.
        StmtKind::ComptimeFor { iter, body, .. } => {
            check_expr(iter, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            if let Some(after) =
                check_block(body, env.clone(), true, node_facts, deletability, types)?
            {
                ensure_same(&env, &after)?;
            }
        }
        // Each case runs from the state after the subject; their exits and
        // the fall-through (unless a case concludes the match) must agree.
        StmtKind::Match { subject, cases } => {
            check_expr(subject, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            let plan = node_facts
                .get(&stmt.source_span())
                .and_then(|facts| facts.match_plan.as_ref());
            let mut exits = Vec::new();
            for (index, case) in cases.iter().enumerate() {
                let mut branch = env.clone();
                if let Some(guard) = &case.guard {
                    check_expr(guard, &mut branch, node_facts, types)?;
                    finish_statement(&mut branch, types)?;
                }
                let exit = check_block(&case.body, branch, true, node_facts, deletability, types)?;
                // A case earlier ones cover is never reached: its exit joins
                // nothing.
                let unreachable = plan
                    .and_then(|plan| plan.unreachable.get(index).copied())
                    .unwrap_or(false);
                exits.extend(exit.filter(|_| !unreachable));
            }
            let concluded = plan.is_some_and(|plan| plan.concluded)
                || cases.iter().any(|case| {
                    case.guard.is_none() && crate::checker::irrefutable_pattern(&case.pattern)
                });
            if !concluded {
                exits.push(env);
            }
            if exits.is_empty() {
                return Ok(None);
            }
            env = join(exits)?;
        }
        // Each case is a `comptime if` arm, run from the entry state; the
        // fall-through, unless a case concludes the match, is the missing
        // `else`.
        StmtKind::ComptimeMatch { subject, cases } => {
            let plan = node_facts
                .get(&stmt.source_span())
                .and_then(|facts| facts.match_plan.as_ref());
            let mut exits = Vec::new();
            for (index, case) in cases.iter().enumerate() {
                let exit = check_block(
                    &case.body,
                    env.clone(),
                    true,
                    node_facts,
                    deletability,
                    types,
                )?;
                let unreachable = plan
                    .and_then(|plan| plan.unreachable.get(index).copied())
                    .unwrap_or(false);
                exits.extend(exit.filter(|_| !unreachable));
            }
            let concluded = plan.is_some_and(|plan| plan.concluded)
                || cases.iter().any(|case| {
                    case.guard.is_none() && crate::checker::irrefutable_pattern(&case.pattern)
                });
            if !concluded {
                exits.push(env.clone());
            }
            if env.is_parametric(subject) {
                let Some(next) = join_parametric(exits) else {
                    return Ok(None);
                };
                env = next;
            } else {
                exits.push(env);
                env = join_comptime(exits);
            }
        }
        StmtKind::While { cond, body, orelse } => {
            check_expr(cond.value(), &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            if let Some(after) =
                check_block(body, env.clone(), true, node_facts, deletability, types)?
            {
                ensure_same(&env, &after)?;
            }
            if let Some(body) = orelse
                && let Some(after) =
                    check_block(body, env.clone(), true, node_facts, deletability, types)?
            {
                env = after;
            }
        }
        StmtKind::For {
            target,
            iter,
            body,
            orelse,
        } => {
            let Some((var, _)) = target.loop_binding() else {
                return Err(TypeError::InvariantViolation(
                    "a loop target reached explicit destruction unspliced".to_string(),
                ));
            };
            check_expr(iter, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            let linear = deletability.bindings.get(&stmt.source_span()).cloned();
            let live = linear.is_some();
            let mut iteration = env.clone();
            iteration.push();
            iteration.declare(var, linear, live);
            if let Some(mut after) =
                check_block(body, iteration, false, node_facts, deletability, types)?
            {
                after.pop_checked()?;
                ensure_same(&env, &after)?;
            }
            if let Some(body) = orelse
                && let Some(after) =
                    check_block(body, env.clone(), true, node_facts, deletability, types)?
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
            let blocks = TryBlocks {
                body,
                except: except.as_ref().map(|(_, arm)| arm.as_slice()),
                orelse: orelse.as_deref(),
                finalbody: finalbody.as_deref(),
            };
            let Some(next) = check_try(&blocks, env, node_facts, deletability, types)? else {
                return Ok(None);
            };
            env = next;
        }
        StmtKind::SetPlace { place, value } => {
            check_expr(place, &mut env, node_facts, types)?;
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            reinitialize_place(place, &mut env, types)?;
        }
        StmtKind::AugAssign { place, value, .. } => {
            check_expr(place, &mut env, node_facts, types)?;
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
        }
        StmtKind::Destructure { target, value } => {
            check_expr(value, &mut env, node_facts, types)?;
            for place in target.places() {
                check_expr(place, &mut env, node_facts, types)?;
            }
            finish_statement(&mut env, types)?;
        }
        StmtKind::RefDecl { value, .. } => {
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
        }
        // A constant computed from a parameter varies with the instantiation
        // as the parameter does (`comptime k = T == Int`).
        StmtKind::Comptime { name, value, .. } => {
            check_expr(value, &mut env, node_facts, types)?;
            finish_statement(&mut env, types)?;
            if env.is_parametric(value) {
                std::rc::Rc::make_mut(&mut env.params).insert(name.clone());
            }
        }
        StmtKind::Break | StmtKind::Continue => {
            env.check_current_scope()?;
            return Ok(None);
        }
        // A nested function's parameters and locals carry their own
        // obligations; its captures never move the enclosing values.
        StmtKind::Def { .. } => check_def(stmt, 0, node_facts, deletability, types, &env)?,
        _ => {}
    }
    Ok(Some(env))
}

/// Check one free (top-level or nested) function definition statement,
/// whose first `receiver` arguments are a method receiver that owes nothing.
fn check_def(
    statement: &Stmt,
    receiver: usize,
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    deletability: &CheckedDeletability,
    types: &DeclarationTable<ExplicitDestroyInfo>,
    outer: &Env,
) -> Result<(), TypeError> {
    let StmtKind::Def {
        type_params,
        params,
        body,
        ..
    } = &statement.kind
    else {
        return Ok(());
    };
    let params = params.iter().enumerate().skip(receiver).map(|(param, p)| {
        let site = AnnotationSite::FunctionParam {
            module: statement.module.clone(),
            declaration: statement.span,
            syntax: statement.syntax_id,
            param,
        };
        (
            &p.name,
            p.convention,
            deletability.declarations.get(&site).cloned(),
        )
    });
    check_function(
        params,
        body,
        node_facts,
        deletability,
        types,
        outer.function_env(type_params),
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
    node_facts: &HashMap<SourceSpan, NodeFacts>,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let bindings = node_facts
        .get(&expression.source_span())
        .and_then(|facts| facts.comprehension_bindings.as_ref())
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
                mojito_ast::ast::ComprehensionClause::For { iter, unpack, .. } => {
                    check_expr(iter, env, node_facts, types)?;
                    let binding = bindings.get(binding_index).ok_or_else(|| {
                        TypeError::InvariantViolation(
                            "comprehension binder metadata is incomplete".to_string(),
                        )
                    })?;
                    binding_index += 1;
                    let linear = env.spans.generators.get(&iter.source_span()).cloned();
                    let live = linear.is_some();
                    env.push();
                    env.declare(&binding.name, linear, live);
                    if let Some(unpack) = unpack
                        && let StmtKind::Destructure { target, .. } = &unpack.kind
                    {
                        for place in target.places() {
                            check_expr(place, env, node_facts, types)?;
                        }
                    }
                }
                mojito_ast::ast::ComprehensionClause::If(condition) => {
                    check_expr(condition, env, node_facts, types)?;
                }
            }
        }
        if binding_index != bindings.len() {
            return Err(TypeError::InvariantViolation(
                "comprehension binder metadata has extra entries".to_string(),
            ));
        }
        if let Some(key) = key {
            check_expr(key, env, node_facts, types)?;
        }
        check_expr(value, env, node_facts, types)
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

/// The obligation place a `^` transfers, which its operation consumes: a
/// field path of a linear local.
fn transferred_place(expr: &Expr, env: &Env) -> Result<Option<(usize, Vec<String>)>, TypeError> {
    if contains_index(expr)
        && let Some(id) = root_id(expr, env)
        && env.vars[id].linear.is_some()
    {
        return explicit_error(
            &env.vars[id],
            "uses a dynamic indexed projection that cannot form a stable residual field obligation",
        );
    }
    let Some((id, path)) = obligation_place(expr, env) else {
        return Ok(None);
    };
    ensure_initialized(id, env)?;
    Ok(env.vars[id].linear.is_some().then_some((id, path)))
}

/// Consume what the operands evaluated since `baseline` own: the in-flight
/// temporaries, and the transferred places, whose obligations move to the
/// operation.
fn consume_operands(
    env: &mut Env,
    (in_flight, pending): (usize, usize),
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    env.in_flight.truncate(in_flight);
    for (id, path) in env.pending.split_off(pending) {
        let root_type = linear_root(id, env);
        expose_path(id, root_type.as_deref(), &path, env, types)?;
        // A whole linear subobject carries its obligation to the
        // destination. An ordinary field has no entry after decomposition
        // and needs no discharge.
        env.vars[id].obligations.remove(&path);
        env.vars[id].moved.insert(path);
    }
    Ok(())
}

/// A statement's end consumes every operand still owned: the statement is
/// the operation that takes them.
fn finish_statement(
    env: &mut Env,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    consume_operands(env, (0, 0), types)
}

/// A raise edge: every value it leaves behind must already be consumed. An
/// in-flight temporary or a transferred place is not yet; a value declared
/// inside the innermost `try` region ends with the raise; with no region,
/// the raise leaves the function and every value ends. The state the target
/// starts from is recorded on the region.
fn raise_edge(env: &Env) -> Result<(), TypeError> {
    if let Some(linear) = env.in_flight.first() {
        return Err(abandoned_on_raise(TEMPORARY, linear));
    }
    let Some(frame) = env.frames.last() else {
        return env.check_raised(0..env.vars.len());
    };
    env.check_raised(
        env.scopes[frame.depth..]
            .iter()
            .flat_map(|scope| scope.values().copied()),
    )?;
    let mut entry = env.clone();
    entry.scopes.truncate(frame.depth);
    entry.vars.truncate(frame.vars);
    entry.in_flight.clear();
    entry.pending.clear();
    entry.frames.pop();
    frame.entries.borrow_mut().push(entry);
    Ok(())
}

fn destructor_at_path(
    root_type: Option<&str>,
    path: &[String],
    method: &str,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> bool {
    type_at_path(root_type, path, types)
        .and_then(|name| types.get(&name))
        .is_some_and(|info| info.destructors.contains_key(method))
}

fn consume_destructor(
    id: usize,
    path: &[String],
    env: &mut Env,
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let root_type = linear_root(id, env);
    expose_path(id, root_type.as_deref(), path, env, types)?;
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
    types: &DeclarationTable<ExplicitDestroyInfo>,
) -> Result<(), TypeError> {
    let Some((id, path)) = obligation_place(expr, env) else {
        return Ok(());
    };
    let Some(linear) = env.vars[id].linear.clone() else {
        return Ok(());
    };
    env.vars[id]
        .moved
        .retain(|moved| !(moved == &path || moved.starts_with(&path)));
    if env.vars[id].moved.is_empty() {
        env.vars[id].obligations.clear();
        env.vars[id].obligations.insert(Vec::new());
    } else if type_at_path(linear.destroy_type.as_deref(), &path, types)
        .is_some_and(|name| types.contains_key(&name))
    {
        env.vars[id].obligations.insert(path);
    }
    Ok(())
}

/// The struct a linear local's obligations decompose by: its
/// explicit-destroy entry's key, or `None` for an archetype.
fn linear_root(id: usize, env: &Env) -> Option<String> {
    env.vars[id]
        .linear
        .as_ref()
        .and_then(|linear| linear.destroy_type.clone())
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
