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

/// Which declarations [`check`] walks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DestroyScope<'a> {
    /// Every function and method of the elaborated program.
    Program,
    /// Only the template bodies source validation checked with the
    /// declaration's parameters symbolic (`validates_body`), which needs the
    /// bodies a `rebind` keys, less the pack-keyed bodies it reached no
    /// verdict on. Every other body is left to the executable check, whose
    /// facts this run does not have. Both sets key a body at its first
    /// statement.
    ValidatedTemplates {
        rebind_keyed: &'a HashSet<SourceSpan>,
        no_verdict: &'a HashSet<SourceSpan>,
    },
}

/// Per-expression facts the checker recorded for one program, carried on
/// every environment this analysis builds.
#[derive(Default)]
pub struct SpanFacts {
    /// Spans of `^` transfers the checker bound to a read parameter or
    /// receiver: they lend their place and consume nothing.
    pub lent: HashSet<SourceSpan>,
    /// Call results the enclosing body owns but cannot destroy — their type is
    /// one of its own type parameters, whose bounds do not prove `Deinitable`
    /// — in a position that takes no ownership of them.
    pub linear_temporaries: HashSet<SourceSpan>,
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
    spans: SpanFacts,
    scope: DestroyScope<'_>,
) -> Result<(), TypeError> {
    if types.is_empty() {
        return Ok(());
    }
    let root = Env {
        spans: std::rc::Rc::new(spans),
        ..Env::default()
    };
    for statement in statements {
        match &statement.kind {
            StmtKind::Def {
                type_params, body, ..
            } if walks_body(scope, &[], type_params, body) => check_def(
                statement,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
                &root,
            )?,
            StmtKind::Def { .. } => {}
            // A template shell carries signatures only.
            StmtKind::Struct {
                template_shell: true,
                ..
            } => {}
            StmtKind::Struct {
                name,
                type_params,
                methods,
                ..
            } => {
                let owner = root.function_env(type_params);
                for (method_index, method) in methods.iter().enumerate() {
                    if !walks_body(scope, type_params, &method.type_params, &method.body) {
                        continue;
                    }
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
                        owner.function_env(&method.type_params),
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
    if env.spans.linear_temporaries.contains(&expr.source_span())
        && let Some(info) = types.get(LINEAR_TYPE_PARAMETER)
    {
        return Err(TypeError::LinearAbandoned {
            var: "(expression temporary)".to_string(),
            message: info.message.clone(),
        });
    }
    match &expr.kind {
        ExprKind::Transfer(inner) if env.spans.lent.contains(&expr.source_span()) => {
            check_expr(inner, env, comprehension_bindings, types)?;
        }
        // A transferred temporary (`make(x^)^.close()`) is no place of its
        // own; the transfers inside it still move theirs.
        ExprKind::Transfer(inner) if root_id(inner, env).is_none() => {
            check_expr(inner, env, comprehension_bindings, types)?;
        }
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
    spans: std::rc::Rc<SpanFacts>,
    /// Names of the compile-time parameters in scope: the enclosing
    /// declarations' own and the owning struct's.
    params: std::rc::Rc<HashSet<String>>,
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
                return Err(var.abandoned(message));
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
        // A kept compile-time block runs once, straight through, and its locals'
        // obligations end with it.
        let next = match &statement.kind {
            StmtKind::Scope(block) => check_block(
                block,
                env,
                true,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )?,
            _ => check_stmt(
                statement,
                env,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )?,
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

impl Var {
    /// The error for leaving this variable's obligations undischarged.
    fn abandoned(&self, message: &str) -> TypeError {
        if self.explicit_type.as_deref() == Some(LINEAR_TYPE_PARAMETER) {
            TypeError::LinearAbandoned {
                var: self.name.clone(),
                message: message.to_string(),
            }
        } else {
            TypeError::Abandoned {
                var: self.name.clone(),
                message: message.to_string(),
            }
        }
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
            if var.message.is_some()
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
    branches: &[(Expr, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Vec<Env>, TypeError> {
    let mut exits = Vec::new();
    for (condition, body) in branches {
        let mut branch = env.clone();
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
    match orelse {
        Some(body) => {
            if let Some(exit) = check_block(
                body,
                env,
                true,
                binding_types,
                comprehension_bindings,
                deletability,
                types,
            )? {
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
    branches: &[(Expr, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    let exits = branch_exits(
        branches,
        orelse,
        env,
        binding_types,
        comprehension_bindings,
        deletability,
        types,
    )?;
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
    branches: &[(Expr, Vec<Stmt>)],
    orelse: Option<&[Stmt]>,
    env: Env,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
) -> Result<Option<Env>, TypeError> {
    let parametric = branches
        .iter()
        .any(|(condition, _)| env.is_parametric(condition));
    let mut exits = branch_exits(
        branches,
        orelse,
        env.clone(),
        binding_types,
        comprehension_bindings,
        deletability,
        types,
    )?;
    if parametric {
        return Ok(join_parametric(exits));
    }
    exits.push(env);
    Ok(Some(join_comptime(exits)))
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
    mut env: Env,
) -> Result<(), TypeError> {
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
        // The loop unrolls per instantiation, possibly to nothing or to
        // several copies, so its body must leave every outer obligation as it
        // found it, as a `while` body must.
        StmtKind::ComptimeFor { iter, body, .. } => {
            check_expr(iter, &mut env, comprehension_bindings, types)?;
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
        StmtKind::RefDecl { value, .. } => {
            check_expr(value, &mut env, comprehension_bindings, types)?;
        }
        // A constant computed from a parameter varies with the instantiation
        // as the parameter does (`comptime k = T == Int`).
        StmtKind::Comptime { name, value, .. } => {
            check_expr(value, &mut env, comprehension_bindings, types)?;
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
        StmtKind::Def { .. } => check_def(
            stmt,
            binding_types,
            comprehension_bindings,
            deletability,
            types,
            &env,
        )?,
        _ => {}
    }
    Ok(Some(env))
}

/// Check one free (top-level or nested) function definition statement.
fn check_def(
    statement: &Stmt,
    binding_types: &HashMap<SourceSpan, Ty>,
    comprehension_bindings: &HashMap<
        SourceSpan,
        Vec<mojito_checked::checked::CheckedComprehensionBinding>,
    >,
    deletability: &CheckedDeletability,
    types: &HashMap<String, ExplicitDestroyInfo>,
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
        outer.function_env(type_params),
    )
}

/// Whether `scope` walks a declaration with these parameters and body. Source
/// validation checked exactly the bodies `validates_body` selects; the facts
/// this pass reads exist for no other body in that run.
fn walks_body(
    scope: DestroyScope<'_>,
    enclosing: &[mojito_ast::ast::TypeParam],
    type_params: &[mojito_ast::ast::TypeParam],
    body: &[Stmt],
) -> bool {
    match scope {
        DestroyScope::Program => true,
        DestroyScope::ValidatedTemplates {
            rebind_keyed,
            no_verdict,
        } => {
            crate::checker::validates_comptime_body(enclosing, type_params, body, rebind_keyed)
                && !body
                    .first()
                    .is_some_and(|first| no_verdict.contains(&first.source_span()))
        }
    }
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
            StmtKind::Scope(body) => consumed_roots_in_stmts(body, env, roots),
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
        ExprKind::Transfer(inner) if !env.spans.lent.contains(&expr.source_span()) => {
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
