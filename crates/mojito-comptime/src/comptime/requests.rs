//! Module constants whose initializer applies a callable (`comptime C =
//! f(1)`), evaluated on demand: a function body's value read of one is a
//! `comptime(...)` request the elaborator below MIR serves from its worklist,
//! and a reader above the check — a type, a compile-time condition, another
//! constant — forces it here through the compile-time route, once. A
//! constant nothing forces is never evaluated here.
//! See `docs/notes/ctfe-request-path.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// A module constant awaiting its first demand: its initializer, its
/// annotation, and the module environment it was declared in.
pub(super) struct PendingConstant {
    value: Expr,
    annotation: Option<Type>,
    env: HashMap<String, CtValue>,
}

impl Elab<'_> {
    /// Record the module constant `name` as pending when its initializer
    /// applies a callable or reads a pending constant, and no `where`
    /// clause constrains it. Whether it was recorded.
    pub(super) fn defer_constant(
        &self,
        name: &str,
        annotation: Option<&Type>,
        constrained: bool,
        value: &Expr,
        env: &HashMap<String, CtValue>,
    ) -> bool {
        if constrained {
            return false;
        }
        let reads_pending = {
            let pending = self.pending_constants.borrow();
            let names: HashSet<String> = pending.keys().cloned().collect();
            !names.is_empty() && super::elab::expression_names_any(value, &names)
        };
        // A display or a subscript is folded here: a body's read of an
        // element would subscript a written display at run time (R274).
        let folded_root = matches!(
            value.kind,
            ExprKind::ListLit(_)
                | ExprKind::TupleLit(_)
                | ExprKind::BraceLit(_)
                | ExprKind::Index { .. }
        );
        if folded_root || (!reads_pending && !self.applies_callable(value)) {
            return false;
        }
        self.pending_constants.borrow_mut().insert(
            name.to_string(),
            PendingConstant {
                value: value.clone(),
                annotation: annotation.cloned(),
                env: env.clone(),
            },
        );
        true
    }

    /// The value of the pending module constant `name`, evaluated now on its
    /// first demand and kept; `None` for any other name. A constant whose
    /// evaluation demands itself is a cycle in the parameter domain.
    pub(super) fn force_constant(&self, name: &str) -> Result<Option<CtValue>, ComptimeError> {
        if let Some(value) = self.forced_constants.borrow().get(name) {
            return Ok(Some(value.clone()));
        }
        let pending = self.pending_constants.borrow().get(name).map(|pending| {
            (
                pending.value.clone(),
                pending.annotation.clone(),
                pending.env.clone(),
            )
        });
        let Some((value, annotation, env)) = pending else {
            return Ok(None);
        };
        if !self.forcing_constants.borrow_mut().insert(name.to_string()) {
            return Err(ComptimeError::NotComptime(format!(
                "function instantiation in parameter domain that recursively requires itself: \
                 the initializer of '{name}' reads '{name}'"
            )));
        }
        // A body's demand of a module constant is a reader above the check.
        let in_body = self.evaluating_body.replace(false);
        let forced = self.eval(&value, &env);
        self.evaluating_body.set(in_body);
        self.forcing_constants.borrow_mut().remove(name);
        let forced = match &annotation {
            Some(annotation) => self.typed_by_annotation(forced?, annotation, &env),
            None => forced?,
        };
        self.forced_constants
            .borrow_mut()
            .insert(name.to_string(), forced.clone());
        self.top_consts
            .borrow_mut()
            .insert(name.to_string(), forced.clone());
        Ok(Some(forced))
    }

    /// What a materializing rewrite reads a pending constant as: the value
    /// forced now, a failure kept in `failure` for the caller to report.
    pub(super) fn pending_lookup<'s>(
        &'s self,
        failure: &'s RefCell<Option<ComptimeError>>,
    ) -> impl Fn(&str) -> Option<CtValue> + 's {
        move |name| match self.force_constant(name) {
            Ok(value) => value,
            Err(error) => {
                failure.borrow_mut().get_or_insert(error);
                None
            }
        }
    }

    /// Rewrite each value read of a pending constant in a function body of
    /// the elaborated `program` to the request `comptime(<initializer>)`, and
    /// take every pending declaration out of it, each with its position: a
    /// reader above the check forces the constant through the
    /// materializing rewrite ([`Self::pending_lookup`]), and
    /// [`Self::restore_forced_constants`] puts back the ones it forced.
    pub(super) fn request_pending_reads(&self, program: &mut Vec<Stmt>) -> Vec<(usize, Stmt)> {
        let expansions = self.pending_expansions();
        if expansions.is_empty() {
            return Vec::new();
        }
        for statement in program.iter_mut() {
            match &mut statement.kind {
                StmtKind::Def {
                    type_params,
                    params,
                    body,
                    ..
                } => {
                    let shadowed = super::def_bound_names(type_params, params, body);
                    request_value_reads(body, &expansions, &shadowed, self);
                }
                StmtKind::Struct { methods, .. } => {
                    for method in methods {
                        let shadowed = super::def_bound_names(
                            &method.type_params,
                            &method.params,
                            &method.body,
                        );
                        request_value_reads(&mut method.body, &expansions, &shadowed, self);
                    }
                }
                _ => {}
            }
        }
        let mut deferred = Vec::new();
        let mut kept = Vec::with_capacity(program.len());
        for (position, statement) in program.drain(..).enumerate() {
            if matches!(&statement.kind, StmtKind::Comptime { name, type_params, .. }
                if type_params.is_empty() && expansions.contains_key(name))
            {
                deferred.push((position, statement));
            } else {
                kept.push(statement);
            }
        }
        *program = kept;
        deferred
    }

    /// Put each `deferred` declaration a reader forced back at its
    /// position, its value the forced one; one nothing forced is dropped.
    pub(super) fn restore_forced_constants(
        &self,
        program: &mut Vec<Stmt>,
        deferred: Vec<(usize, Stmt)>,
    ) {
        let forced = self.forced_constants.borrow();
        let mut dropped = 0;
        for (position, statement) in deferred {
            let StmtKind::Comptime {
                name,
                type_params,
                ty,
                where_clauses,
                ..
            } = &statement.kind
            else {
                continue;
            };
            let Some(value) = forced
                .get(name)
                .filter(|value| !value.is_runtime_collection())
                .and_then(|value| value.materialize(statement.span))
            else {
                dropped += 1;
                continue;
            };
            let kind = StmtKind::Comptime {
                name: name.clone(),
                type_params: type_params.clone(),
                ty: ty.clone(),
                where_clauses: where_clauses.clone(),
                value,
            };
            program.insert(position - dropped, rebuilt(&statement, kind));
        }
    }

    /// Each pending constant's initializer with every pending constant it
    /// reads replaced by that one's own, so that it stands alone in a body.
    pub(super) fn pending_expansions(&self) -> HashMap<String, Expr> {
        let pending = self.pending_constants.borrow();
        let mut expansions: HashMap<String, Expr> = HashMap::new();
        let mut names: Vec<&String> = pending.keys().collect();
        names.sort();
        for name in names {
            expand_constant(name, &pending, &mut expansions, &mut HashSet::new());
        }
        expansions
    }
}

/// `expression` standing where `at` stood: its span, source, and identity.
pub(super) fn located(mut expression: Expr, at: &Expr) -> Expr {
    expression.span = at.span;
    expression.source.clone_from(&at.source);
    expression.syntax_id = at.syntax_id;
    expression
}

/// The address of every expression a walk visits, which tells one
/// occurrence from another of the same syntax.
struct ExprAddresses<'s>(&'s mut HashSet<usize>);

impl mojito_ast::visit::Visitor for ExprAddresses<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        self.0.insert(std::ptr::from_ref(expr) as usize);
    }
}

/// The names `statements` read: identifiers and type names, anywhere.
#[derive(Default)]
struct NamesRead(HashSet<String>);

impl mojito_ast::visit::Visitor for NamesRead {
    fn visit_expr(&mut self, expr: &Expr) {
        if let ExprKind::Identifier(name) = &expr.kind {
            self.0.insert(name.clone());
        }
    }

    fn visit_type(&mut self, ty: &Type) {
        if let Type::Named(name, _) = ty {
            self.0.insert(name.clone());
        }
    }
}

/// The expansion of the pending constant `name` ([`Elab::pending_expansions`]).
/// A constant whose expansion reaches itself keeps the name it reads there,
/// which forcing reports as a cycle.
fn expand_constant(
    name: &str,
    pending: &HashMap<String, PendingConstant>,
    expansions: &mut HashMap<String, Expr>,
    visiting: &mut HashSet<String>,
) -> Option<Expr> {
    if let Some(expansion) = expansions.get(name) {
        return Some(expansion.clone());
    }
    let constant = pending.get(name)?;
    if !visiting.insert(name.to_string()) {
        return None;
    }
    let mut expansion = constant.value.clone();
    let mut reads = Vec::new();
    collect_identifier_reads(&expansion, pending, &mut reads);
    let replacements: HashMap<String, Expr> = reads
        .into_iter()
        .filter_map(|read| {
            let inner = expand_constant(&read, pending, expansions, visiting)?;
            Some((read, inner))
        })
        .collect();
    replace_identifiers(&mut expansion, &replacements);
    // An annotation other than `Int` or `Bool` converts the value, which
    // the check proved implicit where the constant is declared.
    if let Some(Type::Named(annotation, param_args)) = &constant.annotation
        && !matches!(annotation.as_str(), "Int" | "Bool")
    {
        expansion = Expr {
            kind: ExprKind::Call {
                name: annotation.clone(),
                param_args: param_args.clone(),
                args: vec![expansion.clone()],
                kwargs: Vec::new(),
            },
            ..expansion
        };
    }
    visiting.remove(name);
    expansions.insert(name.to_string(), expansion.clone());
    Some(expansion)
}

/// The pending constants `expression` reads by name.
fn collect_identifier_reads(
    expression: &Expr,
    pending: &HashMap<String, PendingConstant>,
    reads: &mut Vec<String>,
) {
    let mut names = NamesRead::default();
    mojito_ast::visit::walk_expr(&mut names, expression);
    reads.extend(
        names
            .0
            .into_iter()
            .filter(|name| pending.contains_key(name)),
    );
}

/// Replace each identifier of `expression` that `replacements` names by
/// its replacement, keeping the identifier's provenance.
fn replace_identifiers(expression: &mut Expr, replacements: &HashMap<String, Expr>) {
    struct Replace<'r>(&'r HashMap<String, Expr>);

    impl mojito_ast::visit::MutVisitor for Replace<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(replacement) = self.0.get(name)
            {
                *expr = located(replacement.clone(), expr);
            }
        }
    }

    if replacements.is_empty() {
        return;
    }
    mojito_ast::visit::walk_expr_mut(&mut Replace(replacements), expression);
}

/// Rewrite each value read of a pending constant in `body` to the request
/// `comptime(<expansion>)`. A read in a type, a compile-time argument, or a
/// compile-time statement is left for forcing, as is a name `shadowed` by a
/// parameter or a local the body binds.
fn request_value_reads(
    body: &mut [Stmt],
    expansions: &HashMap<String, Expr>,
    shadowed: &HashSet<String>,
    elab: &Elab<'_>,
) {
    struct CompileTimeReads<'e, 'a> {
        elab: &'e Elab<'a>,
        skipped: HashSet<usize>,
    }

    impl CompileTimeReads<'_, '_> {
        fn skip_expr(&mut self, expression: &Expr) {
            mojito_ast::visit::walk_expr(&mut ExprAddresses(&mut self.skipped), expression);
        }

        fn skip_param_args(&mut self, arguments: &[ParamArg]) {
            for argument in arguments {
                mojito_ast::visit::walk_param_arg(&mut ExprAddresses(&mut self.skipped), argument);
            }
        }
    }

    impl mojito_ast::visit::Visitor for CompileTimeReads<'_, '_> {
        fn visit_stmt(&mut self, statement: &Stmt) {
            match &statement.kind {
                StmtKind::Comptime { value, .. } => self.skip_expr(value),
                StmtKind::ComptimeIf { branches, .. } => {
                    for (condition, _) in branches {
                        self.skip_expr(condition);
                    }
                }
                StmtKind::ComptimeFor { iter, .. } => self.skip_expr(iter),
                _ => {}
            }
        }

        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Call { param_args, .. } | ExprKind::Invoke { param_args, .. } => {
                    self.skip_param_args(param_args);
                }
                ExprKind::TypeApply { args, .. } => self.skip_param_args(args),
                // A lone compile-time argument of a struct or a function
                // parses as a subscript (`G[C]`).
                ExprKind::Index { object, index }
                    if matches!(&object.kind, ExprKind::Identifier(name)
                        if self.elab.structs.contains_key(name.as_str())
                            || self.elab.fns.contains_key(name.as_str())) =>
                {
                    self.skip_expr(index);
                }
                ExprKind::TypeValue(_) => self.skip_expr(expr),
                _ => {}
            }
        }

        fn visit_type(&mut self, ty: &Type) {
            mojito_ast::visit::walk_type(&mut ExprAddresses(&mut self.skipped), ty);
        }
    }

    struct Requests<'x> {
        expansions: &'x HashMap<String, Expr>,
        shadowed: &'x HashSet<String>,
        skipped: HashSet<usize>,
    }

    impl mojito_ast::visit::MutVisitor for Requests<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            let ExprKind::Identifier(name) = &expr.kind else {
                return;
            };
            if self.shadowed.contains(name.as_str())
                || self.skipped.contains(&(std::ptr::from_ref(expr) as usize))
            {
                return;
            }
            let Some(expansion) = self.expansions.get(name) else {
                return;
            };
            let request = Expr {
                kind: ExprKind::Call {
                    name: "comptime".to_string(),
                    param_args: Vec::new(),
                    args: vec![expansion.clone()],
                    kwargs: Vec::new(),
                },
                ..expr.clone()
            };
            *expr = request;
        }
    }

    let mut reads = CompileTimeReads {
        elab,
        skipped: HashSet::new(),
    };
    mojito_ast::visit::walk_block(&mut reads, body);
    let mut requests = Requests {
        expansions,
        shadowed,
        skipped: reads.skipped,
    };
    mojito_ast::visit::walk_block_mut(&mut requests, body);
}
