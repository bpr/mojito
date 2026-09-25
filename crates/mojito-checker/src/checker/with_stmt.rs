//! The `with` statement: a type-aware desugar into ordinary statements
//! (`VarDecl`, `Try`, `If`, `Raise`, `Expr`) that is checked in place and
//! spliced into the final checked tree, so HIR, MIR, and the backends never
//! see a context manager. The manager type selects the shape:
//!
//! - a consuming `__enter__` (`var self`, no `__exit__` allowed): the enter
//!   result stands in for the manager and lives to the end of the block;
//! - a plain `__exit__(self)`: `try: body finally: manager.__exit__()`;
//! - both `__exit__(self)` and `__exit__(self, err: Error) -> Bool`, in a
//!   raising context: the body's error goes to the error overload, whose
//!   `False` result re-raises it, and the plain overload runs otherwise;
//! - no `__exit__` at all: the manager lives to the end of the block.
//!
//! An `as NAME` binding is scoped to the block and destroyed at its last use
//! like any local (the pinned Mojo destroys it before the block ends); only
//! the manager — or its consuming-enter stand-in — is kept alive through the
//! block, via the compiler-private `_mojito_keep_alive(name)` statement HIR
//! lowers to a liveness anchor. Extracted from `statements.rs`; see
//! `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::WithItem;
use mojito_checked::templates::WithForm;
use mojito_common::token::SyntaxId;

/// The compiler-private liveness anchor the desugar emits at the end of a
/// block: `_mojito_keep_alive(name)` keeps `name` alive to that point without
/// copying or moving it (HIR lowers it to `KeepAlive`).
pub const KEEP_ALIVE_BUILTIN: &str = "_mojito_keep_alive";

/// A checked `with` statement's desugar: the statements that replace it and
/// the form its manager's protocol decided.
#[derive(Debug, Clone)]
pub(super) struct WithDesugar {
    pub(super) form: WithForm,
    pub(super) statements: Vec<Stmt>,
}

/// Replace every checked `with` statement in `stmts` (recursively, through
/// every nested block and declaration body) by its recorded desugar. A `with`
/// without a recorded desugar — an unchecked generic template body kept
/// verbatim for monomorphization — stays as written.
pub(super) fn splice_with_desugars(
    stmts: &mut Vec<Stmt>,
    desugars: &HashMap<SourceSpan, WithDesugar>,
) {
    let mut index = 0;
    while index < stmts.len() {
        if matches!(stmts[index].kind, StmtKind::With { .. })
            && let Some(replacement) = desugars.get(&stmts[index].source_span())
        {
            // The replacement may itself hold a nested `with` (a later item of
            // a multi-item statement, or one written in the body); re-examine
            // from the same index.
            stmts.splice(index..=index, replacement.statements.iter().cloned());
            continue;
        }
        splice_nested(&mut stmts[index], desugars);
        index += 1;
    }
}

/// The desugar of the `with` statement `stmt` in `form`, built without a
/// check: the statements [`Checker::check_with`] builds for it, node for
/// node and identity for identity.
pub(super) fn with_desugar(stmt: &Stmt, form: WithForm) -> Option<Vec<Stmt>> {
    let StmtKind::With { items, body } = &stmt.kind else {
        return None;
    };
    let (item, synth, body) = first_item(stmt, items, body)?;
    let manager = synth.name("mgr");
    let mut desugar = vec![manager_declaration(item, &synth, &manager)];
    desugar.extend(desugar_tail(item, body, &synth, &manager, form));
    Some(desugar)
}

impl Checker {
    /// Check `with items: body` by desugaring it (see the module docs),
    /// checking the desugar in a block scope, and recording it for the
    /// final splice.
    pub(super) fn check_with(
        &mut self,
        stmt: &Stmt,
        items: &[WithItem],
        body: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let (item, synth, body) = first_item(stmt, items, body).ok_or_else(|| {
            TypeError::InvariantViolation("with statement without a context item".to_string())
        })?;
        self.push_scope();
        let result = self.check_with_item(item, body, &synth, ret, in_loop);
        self.pop_scope();
        let desugar = result?;
        self.with_desugars
            .borrow_mut()
            .insert(stmt.source_span(), desugar);
        Ok(())
    }

    fn check_with_item(
        &mut self,
        item: &WithItem,
        body: Vec<Stmt>,
        synth: &Synth,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<WithDesugar, TypeError> {
        let manager = synth.name("mgr");
        let mut statements = vec![manager_declaration(item, synth, &manager)];
        self.check_stmt(&statements[0], ret, in_loop)?;
        let manager_ty = self.lookup(&manager).cloned().ok_or_else(|| {
            TypeError::InvariantViolation("with manager binding was not declared".to_string())
        })?;
        let form = self.context_manager_form(&manager_ty)?;
        statements.extend(desugar_tail(item, body, synth, &manager, form));
        for statement in &statements[1..] {
            self.check_stmt(statement, ret, in_loop)?;
        }
        Ok(WithDesugar { form, statements })
    }

    /// The desugar form a manager's protocol selects, from its declared
    /// methods and whether the context may raise, reporting the pinned
    /// Mojo's diagnostics for the unsupported combinations.
    fn context_manager_form(&self, ty: &Ty) -> Result<WithForm, TypeError> {
        let no_enter = || {
            TypeError::ContextManager(format!("'{ty}' does not implement the '__enter__' method"))
        };
        let Ty::Struct(name, _) = ty else {
            return Err(no_enter());
        };
        let info = self.structs.get(name).ok_or_else(no_enter)?;
        let nullary =
            |sig: &&MethodSig| sig.has_self && sig.required.iter().all(|required| !required);
        let enter = info
            .methods
            .get("__enter__")
            .and_then(|sigs| sigs.iter().find(nullary))
            .ok_or_else(no_enter)?;
        let enter_consumes = enter.self_convention == Some(ArgConvention::Var);
        let exits = info
            .methods
            .get("__exit__")
            .map(Vec::as_slice)
            .unwrap_or_default();
        let plain_exit = exits.iter().any(|sig| nullary(&sig));
        let error_exit = exits.iter().any(|sig| {
            sig.has_self
                && sig.params.len() == 1
                && sig.params[0] == Ty::Error
                && sig.ret == Ty::Bool
        });
        if enter_consumes && (plain_exit || error_exit) {
            return Err(TypeError::ContextManager(format!(
                "context manager of type '{ty}' defines a consuming __enter__ method as well as an __exit__ method; either remove 'var' from its '__enter__' method or remove the '__exit__' method"
            )));
        }
        if error_exit && !plain_exit {
            return Err(TypeError::BadCall {
                func: "__exit__".to_string(),
                reason: "missing required argument: 'err'".to_string(),
            });
        }
        Ok(if enter_consumes {
            WithForm::ConsumingEnter {
                returns_none: enter.ret == Ty::None,
            }
        } else if !plain_exit {
            WithForm::KeptManager
        } else if error_exit && self.raising_allowed() {
            WithForm::ErrorExit
        } else {
            WithForm::PlainExit
        })
    }
}

/// Node factory for one `with` statement's desugar: every synthesized node
/// carries the statement's span and provenance, and the next identity
/// derived from the statement's own ([`SyntaxId::derived`]), so building the
/// desugar again yields the same occurrences. Hidden names are unique per
/// statement.
struct Synth {
    span: mojito_common::token::Span,
    module: Option<String>,
    source: Option<String>,
    parent: SyntaxId,
    next: std::cell::Cell<u32>,
}

impl Synth {
    fn stmt(&self, kind: StmtKind) -> Stmt {
        let mut statement = Stmt::new(kind, self.span);
        statement.module.clone_from(&self.module);
        statement.syntax_id = self.identity();
        statement
    }

    fn expr(&self, kind: ExprKind) -> Expr {
        let mut expression = Expr::new(kind, self.span);
        expression.source.clone_from(&self.source);
        expression.syntax_id = self.identity();
        expression
    }

    fn identifier(&self, name: &str) -> Expr {
        self.expr(ExprKind::Identifier(name.to_string()))
    }

    fn name(&self, role: &str) -> String {
        format!("$with{}_{role}", self.parent.0)
    }

    fn identity(&self) -> SyntaxId {
        let ordinal = self.next.get();
        self.next.set(ordinal + 1);
        SyntaxId::derived(self.parent, ordinal)
    }
}

/// The first context item of `with items: body`, the node factory for its
/// desugar, and the body it guards: `with a as x, b as y: body` is
/// `with a as x: with b as y: body`.
fn first_item<'a>(
    stmt: &Stmt,
    items: &'a [WithItem],
    body: &[Stmt],
) -> Option<(&'a WithItem, Synth, Vec<Stmt>)> {
    let [item, rest @ ..] = items else {
        return None;
    };
    let synth = Synth {
        span: stmt.span,
        module: stmt.module.clone(),
        source: item.context.source.clone(),
        parent: stmt.syntax_id,
        next: std::cell::Cell::new(0),
    };
    let body = if rest.is_empty() {
        body.to_vec()
    } else {
        vec![synth.stmt(StmtKind::With {
            items: rest.to_vec(),
            body: body.to_vec(),
        })]
    };
    Some((item, synth, body))
}

/// `var manager = context`, the desugar's first statement.
fn manager_declaration(item: &WithItem, synth: &Synth, manager: &str) -> Stmt {
    synth.stmt(StmtKind::VarDecl {
        name: manager.to_string(),
        ty: None,
        value: item.context.clone(),
    })
}

/// Everything the desugar holds after the manager's declaration: the
/// `__enter__` call and the guarded body in `form`.
fn desugar_tail(
    item: &WithItem,
    body: Vec<Stmt>,
    synth: &Synth,
    manager: &str,
    form: WithForm,
) -> Vec<Stmt> {
    let consumes = matches!(form, WithForm::ConsumingEnter { .. });
    // Enter: `[var NAME =] manager[^].__enter__()`.
    let receiver = if consumes {
        synth.expr(ExprKind::Transfer(Box::new(synth.identifier(manager))))
    } else {
        synth.identifier(manager)
    };
    let enter = synth.expr(ExprKind::MethodCall {
        object: Box::new(receiver),
        method: "__enter__".to_string(),
        args: Vec::new(),
        kwargs: Vec::new(),
    });
    // A consuming `__enter__`'s result stands in for the manager (the
    // pinned Mojo destroys it at the block end even when unbound).
    let bound = match (&item.var, form) {
        (Some(name), _) => Some(name.clone()),
        (
            None,
            WithForm::ConsumingEnter {
                returns_none: false,
            },
        ) => Some(synth.name("enter")),
        (None, _) => None,
    };
    let mut desugar = vec![match &bound {
        Some(name) => synth.stmt(StmtKind::VarDecl {
            name: name.clone(),
            ty: None,
            value: enter,
        }),
        None => synth.stmt(StmtKind::Expr(enter)),
    }];

    let keep_alive = |name: &str| {
        synth.stmt(StmtKind::Expr(synth.expr(ExprKind::Call {
            name: KEEP_ALIVE_BUILTIN.to_string(),
            param_args: Vec::new(),
            args: vec![synth.identifier(name)],
            kwargs: Vec::new(),
        })))
    };
    let plain_exit = || {
        synth.stmt(StmtKind::Expr(synth.expr(ExprKind::MethodCall {
            object: Box::new(synth.identifier(manager)),
            method: "__exit__".to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        })))
    };
    let protect =
        |body: Vec<Stmt>, except: Option<(Option<String>, Vec<Stmt>)>, finalbody: Vec<Stmt>| {
            synth.stmt(StmtKind::Try {
                body,
                except,
                orelse: None,
                finalbody: Some(finalbody),
            })
        };

    match form {
        // No `__exit__`; the stand-in lives to the block end.
        WithForm::ConsumingEnter { .. } => match &bound {
            Some(name) => desugar.push(protect(body, None, vec![keep_alive(name)])),
            None => desugar.extend(body),
        },
        // No `__exit__`: the manager itself lives to the block end.
        WithForm::KeptManager => desugar.push(protect(body, None, vec![keep_alive(manager)])),
        // `var handled = False` / `try: body except err: handled = True; if
        // not manager.__exit__(err): raise err finally: if not handled:
        // manager.__exit__()`.
        WithForm::ErrorExit => {
            let handled = synth.name("handled");
            let error = synth.name("err");
            desugar.push(synth.stmt(StmtKind::VarDecl {
                name: handled.clone(),
                ty: None,
                value: synth.expr(ExprKind::Bool(false)),
            }));
            let error_exit = synth.expr(ExprKind::MethodCall {
                object: Box::new(synth.identifier(manager)),
                method: "__exit__".to_string(),
                args: vec![synth.identifier(&error)],
                kwargs: Vec::new(),
            });
            let handler = vec![
                synth.stmt(StmtKind::Assign {
                    name: handled.clone(),
                    value: synth.expr(ExprKind::Bool(true)),
                }),
                synth.stmt(StmtKind::If {
                    branches: vec![(
                        synth.expr(ExprKind::Prefix(PrefixOp::Not, Box::new(error_exit))),
                        vec![synth.stmt(StmtKind::Raise(synth.identifier(&error)))],
                    )],
                    orelse: None,
                }),
            ];
            let cleanup = vec![synth.stmt(StmtKind::If {
                branches: vec![(
                    synth.expr(ExprKind::Prefix(
                        PrefixOp::Not,
                        Box::new(synth.identifier(&handled)),
                    )),
                    vec![plain_exit()],
                )],
                orelse: None,
            })];
            desugar.push(protect(body, Some((Some(error), handler)), cleanup));
        }
        // `try: body finally: manager.__exit__()`.
        WithForm::PlainExit => desugar.push(protect(body, None, vec![plain_exit()])),
    }
    desugar
}

fn splice_nested(stmt: &mut Stmt, desugars: &HashMap<SourceSpan, WithDesugar>) {
    match &mut stmt.kind {
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (_, block) in branches {
                splice_with_desugars(block, desugars);
            }
            if let Some(block) = orelse {
                splice_with_desugars(block, desugars);
            }
        }
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            splice_with_desugars(body, desugars);
            if let Some(block) = orelse {
                splice_with_desugars(block, desugars);
            }
        }
        StmtKind::ComptimeFor { body, .. }
        | StmtKind::With { body, .. }
        | StmtKind::Scope(body)
        | StmtKind::Def { body, .. } => splice_with_desugars(body, desugars),
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            splice_with_desugars(body, desugars);
            if let Some((_, block)) = except {
                splice_with_desugars(block, desugars);
            }
            if let Some(block) = orelse {
                splice_with_desugars(block, desugars);
            }
            if let Some(block) = finalbody {
                splice_with_desugars(block, desugars);
            }
        }
        StmtKind::Struct { methods, .. } => {
            for method in methods {
                splice_with_desugars(&mut method.body, desugars);
            }
        }
        StmtKind::Trait { methods, .. } => {
            for method in methods {
                if let Some(body) = &mut method.default_body {
                    splice_with_desugars(body, desugars);
                }
            }
        }
        _ => {}
    }
}
