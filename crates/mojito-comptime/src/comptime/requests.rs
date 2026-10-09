//! Module constants whose initializer is a display that applies a callable
//! (`comptime XS = [1, twice(3), 5]`): a display has no parameter-expression
//! form, so a read of one is the display itself, spelled where it is read —
//! a function body's value read as the request `comptime(<display>)` the
//! elaborator below MIR serves, a compile-time position (a loop header, a
//! condition, a subscript, a method call, another constant's initializer)
//! as the display in place. See `docs/notes/ctfe-request-path.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Elab<'_> {
    /// `expression` with every applied display constant it names spelled
    /// by its display ([`Self::applied_displays`]), or `expression` as it
    /// is when it names none.
    pub(super) fn spell_applied_displays(&self, expression: &Expr) -> Expr {
        let displays = self.applied_displays.borrow();
        if displays.is_empty() {
            return expression.clone();
        }
        let mut spelled = expression.clone();
        replace_identifiers(&mut spelled, &displays);
        spelled
    }

    /// Rewrite each value read of an applied display constant in a
    /// function body of the elaborated `program` to the request
    /// `comptime(<display>)`. A read in a type, a compile-time argument, or
    /// a compile-time statement is spelled where it is read, as is a name
    /// shadowed by a parameter or a local the body binds.
    pub(super) fn request_display_reads(&self, program: &mut [Stmt]) {
        let expansions = self.applied_displays.borrow();
        if expansions.is_empty() {
            return;
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
                            || self.elab.fns.contains(name.as_str())) =>
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
        /// The address of every request this rewrite wrote.
        written: HashSet<usize>,
    }

    impl mojito_ast::visit::MutVisitor for Requests<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            // A field read of a requested value is a compile-time
            // projection, as at the pin: the request extends over it.
            if let ExprKind::Member { object, .. } = &expr.kind
                && self
                    .written
                    .contains(&(std::ptr::from_ref(&**object) as usize))
                && let ExprKind::Member { object, field } =
                    std::mem::replace(&mut expr.kind, ExprKind::None)
                && let ExprKind::Call { mut args, .. } = object.kind
                && let Some(requested) = args.pop()
            {
                let projection = Expr {
                    kind: ExprKind::Member {
                        object: Box::new(requested),
                        field,
                    },
                    syntax_id: SyntaxId::derived(expr.syntax_id, 0),
                    ..expr.clone()
                };
                *expr = request(projection, expr);
                self.written.insert(std::ptr::from_ref(expr) as usize);
                return;
            }
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
            *expr = request(expansion.clone(), expr);
            self.written.insert(std::ptr::from_ref(expr) as usize);
        }
    }

    /// The request `comptime(<requested>)` standing where `at` stands.
    fn request(requested: Expr, at: &Expr) -> Expr {
        Expr {
            kind: ExprKind::Call {
                name: "comptime".to_string(),
                param_args: Vec::new(),
                args: vec![requested],
                kwargs: Vec::new(),
            },
            ..at.clone()
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
        written: HashSet::new(),
    };
    mojito_ast::visit::walk_block_mut(&mut requests, body);
}
