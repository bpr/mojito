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

/// The compiler-private liveness anchor the desugar emits at the end of a
/// block: `_mojito_keep_alive(name)` keeps `name` alive to that point without
/// copying or moving it (HIR lowers it to `KeepAlive`).
pub const KEEP_ALIVE_BUILTIN: &str = "_mojito_keep_alive";

/// Replace every checked `with` statement in `stmts` (recursively, through
/// every nested block and declaration body) by its recorded desugar. A `with`
/// without a recorded desugar — an unchecked generic template body kept
/// verbatim for monomorphization — stays as written.
pub(super) fn splice_with_desugars(
    stmts: &mut Vec<Stmt>,
    desugars: &HashMap<SourceSpan, Vec<Stmt>>,
) {
    let mut index = 0;
    while index < stmts.len() {
        if matches!(stmts[index].kind, StmtKind::With { .. })
            && let Some(replacement) = desugars.get(&stmts[index].source_span())
        {
            // The replacement may itself hold a nested `with` (a later item of
            // a multi-item statement, or one written in the body); re-examine
            // from the same index.
            stmts.splice(index..=index, replacement.iter().cloned());
            continue;
        }
        splice_nested(&mut stmts[index], desugars);
        index += 1;
    }
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
        let [item, rest @ ..] = items else {
            return Err(TypeError::InvariantViolation(
                "with statement without a context item".to_string(),
            ));
        };
        let synth = Synth {
            span: stmt.span,
            module: stmt.module.clone(),
            source: item.context.source.clone(),
            id: stmt.syntax_id.0,
        };
        // `with a as x, b as y: body` is `with a as x: with b as y: body`.
        let body = if rest.is_empty() {
            body.to_vec()
        } else {
            vec![synth.stmt(StmtKind::With {
                items: rest.to_vec(),
                body: body.to_vec(),
            })]
        };
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
    ) -> Result<Vec<Stmt>, TypeError> {
        let manager = synth.name("mgr");
        let mut desugar = vec![synth.stmt(StmtKind::VarDecl {
            name: manager.clone(),
            ty: None,
            value: item.context.clone(),
        })];
        self.check_stmt(&desugar[0], ret, in_loop)?;
        let manager_ty = self.lookup(&manager).cloned().ok_or_else(|| {
            TypeError::InvariantViolation("with manager binding was not declared".to_string())
        })?;
        let shape = self.context_manager_shape(&manager_ty)?;

        // Enter: `[var NAME =] manager[^].__enter__()`.
        let receiver = if shape.enter_consumes {
            synth.expr(ExprKind::Transfer(Box::new(synth.identifier(&manager))))
        } else {
            synth.identifier(&manager)
        };
        let enter = synth.expr(ExprKind::MethodCall {
            object: Box::new(receiver),
            method: "__enter__".to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        });
        // A consuming `__enter__`'s result stands in for the manager (the
        // pinned Mojo destroys it at the block end even when unbound).
        let bound = match &item.var {
            Some(name) => Some(name.clone()),
            None if shape.enter_consumes && !shape.enter_returns_none => Some(synth.name("enter")),
            None => None,
        };
        desugar.push(match &bound {
            Some(name) => synth.stmt(StmtKind::VarDecl {
                name: name.clone(),
                ty: None,
                value: enter,
            }),
            None => synth.stmt(StmtKind::Expr(enter)),
        });

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
                object: Box::new(synth.identifier(&manager)),
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

        if shape.enter_consumes {
            // Shape A: no `__exit__`; the stand-in lives to the block end.
            match &bound {
                Some(name) => desugar.push(protect(body, None, vec![keep_alive(name)])),
                None => desugar.extend(body),
            }
        } else if !shape.plain_exit {
            // No `__exit__`: the manager itself lives to the block end.
            desugar.push(protect(body, None, vec![keep_alive(&manager)]));
        } else if shape.error_exit && self.raising_allowed() {
            // Shape C: `var handled = False` / `try: body except err:
            // handled = True; if not manager.__exit__(err): raise err
            // finally: if not handled: manager.__exit__()`.
            let handled = synth.name("handled");
            let error = synth.name("err");
            desugar.push(synth.stmt(StmtKind::VarDecl {
                name: handled.clone(),
                ty: None,
                value: synth.expr(ExprKind::Bool(false)),
            }));
            let error_exit = synth.expr(ExprKind::MethodCall {
                object: Box::new(synth.identifier(&manager)),
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
        } else {
            // Shape B: `try: body finally: manager.__exit__()`.
            desugar.push(protect(body, None, vec![plain_exit()]));
        }
        for statement in &desugar[1..] {
            self.check_stmt(statement, ret, in_loop)?;
        }
        Ok(desugar)
    }

    /// Classify a context manager's protocol from its declared methods,
    /// reporting the pinned Mojo's diagnostics for the unsupported
    /// combinations.
    fn context_manager_shape(&self, ty: &Ty) -> Result<ManagerShape, TypeError> {
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
        let enter_returns_none = enter.ret == Ty::None;
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
        Ok(ManagerShape {
            enter_consumes,
            enter_returns_none,
            plain_exit,
            error_exit,
        })
    }
}

/// The context-manager protocol a manager type offers.
#[allow(
    clippy::struct_excessive_bools,
    reason = "TODO: group the flags into a state enum"
)]
struct ManagerShape {
    enter_consumes: bool,
    enter_returns_none: bool,
    plain_exit: bool,
    error_exit: bool,
}

/// Node factory for one `with` statement's desugar: every synthesized node
/// carries the statement's span and provenance and a fresh occurrence
/// identity, and hidden names are unique per statement.
struct Synth {
    span: mojito_common::token::Span,
    module: Option<String>,
    source: Option<String>,
    id: u64,
}

impl Synth {
    fn stmt(&self, kind: StmtKind) -> Stmt {
        let mut statement = Stmt::new(kind, self.span);
        statement.module.clone_from(&self.module);
        statement
    }

    fn expr(&self, kind: ExprKind) -> Expr {
        let mut expression = Expr::new(kind, self.span);
        expression.source.clone_from(&self.source);
        expression
    }

    fn identifier(&self, name: &str) -> Expr {
        self.expr(ExprKind::Identifier(name.to_string()))
    }

    fn name(&self, role: &str) -> String {
        format!("$with{}_{role}", self.id)
    }
}

fn splice_nested(stmt: &mut Stmt, desugars: &HashMap<SourceSpan, Vec<Stmt>>) {
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
