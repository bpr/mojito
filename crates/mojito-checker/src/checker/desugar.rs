//! Statements the checker desugars into ordinary statements: a `with`
//! statement (`with_stmt.rs`), a sequence destructuring assignment
//! (`destructure.rs`), and a destructuring loop target (`destructure.rs`).
//! Each desugar is checked in place, recorded by its statement (a loop
//! target's by the target), and spliced into the final checked tree, so
//! HIR, MIR, and the backends see only the ordinary statements. A loop
//! target's desugar names the hidden binder the loop binds instead: a `for`
//! statement starts its body with the target's statements, and a
//! comprehension clause keeps its one tuple `Destructure` as the clause's
//! `unpack`, which MIR lowers. [`Synth`] builds the synthesized nodes, for
//! these desugars and for `match`'s commands.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{ComprehensionClause, DestructureTarget, DestructureTargetKind};
use mojito_ast::visit::{MutVisitor, walk_block_mut};
use mojito_common::literal::IntLiteral;
use mojito_common::token::SyntaxId;

/// A checked statement's desugar: the statements that replace it.
#[derive(Debug, Clone)]
pub(super) struct StatementDesugar {
    pub(super) statements: Vec<Stmt>,
    /// A loop or comprehension target's desugar: the loop binds `binder`,
    /// and `statements` run first in each iteration.
    pub(super) binder: Option<String>,
}

/// Replace every desugared statement in `stmts` (recursively, through every
/// nested block, declaration body, and lambda) by its recorded desugar, and
/// every desugared loop target by its binder. A `with` or destructuring
/// assignment without a recorded desugar stays as written.
pub(super) fn splice_desugars(
    stmts: &mut Vec<Stmt>,
    desugars: &HashMap<SourceSpan, StatementDesugar>,
) {
    let mut splice = Splice(desugars);
    splice.block(stmts);
    walk_block_mut(&mut splice, stmts);
}

/// A node factory for syntax the check synthesizes on behalf of statement
/// `parent`: every node carries the statement's span and module and the
/// next identity derived from the statement's own ([`SyntaxId::derived`]),
/// so building the desugar again yields the same occurrences and its facts
/// are its own.
pub(super) struct Synth {
    span: mojito_common::token::Span,
    module: Option<String>,
    source: Option<String>,
    parent: SyntaxId,
    next: std::cell::Cell<u32>,
}

impl Synth {
    /// The factory for the nodes the check synthesizes for `stmt`, their
    /// expressions sourced as `source`.
    pub(super) fn for_statement(stmt: &Stmt, source: Option<String>) -> Self {
        Self {
            span: stmt.span,
            module: stmt.module.clone(),
            source,
            parent: stmt.syntax_id,
            next: std::cell::Cell::new(0),
        }
    }

    /// The factory for the nodes the check synthesizes for the `case`
    /// condition whose pattern is `pattern`, their expressions sourced as
    /// `source`.
    pub(super) fn for_pattern(
        pattern: &mojito_ast::ast::MatchPattern,
        source: Option<String>,
    ) -> Self {
        Self {
            span: pattern.span,
            module: pattern.source.clone(),
            source,
            parent: pattern.syntax_id,
            next: std::cell::Cell::new(0),
        }
    }

    /// The factory for the nodes the check synthesizes for the loop target
    /// `target`, their expressions sourced as `source`.
    pub(super) fn for_target(target: &DestructureTarget, source: Option<String>) -> Self {
        Self {
            span: target.span,
            module: target.source.clone(),
            source,
            parent: target.syntax_id,
            next: std::cell::Cell::new(0),
        }
    }

    /// The statement's own identity, which hidden names derive from.
    pub(super) const fn parent(&self) -> SyntaxId {
        self.parent
    }

    pub(super) fn stmt(&self, kind: StmtKind) -> Stmt {
        let mut statement = Stmt::new(kind, self.span);
        statement.module.clone_from(&self.module);
        statement.syntax_id = self.identity();
        statement
    }

    pub(super) fn expr(&self, kind: ExprKind) -> Expr {
        let mut expression = Expr::new(kind, self.span);
        expression.source.clone_from(&self.source);
        expression.syntax_id = self.identity();
        expression
    }

    pub(super) fn identifier(&self, name: &str) -> Expr {
        self.expr(ExprKind::Identifier(name.to_string()))
    }

    pub(super) fn int(&self, value: usize) -> Expr {
        self.expr(ExprKind::Int(IntLiteral::from(
            u64::try_from(value).unwrap_or(u64::MAX),
        )))
    }

    fn identity(&self) -> SyntaxId {
        let ordinal = self.next.get();
        self.next.set(ordinal + 1);
        SyntaxId::derived(self.parent, ordinal)
    }
}

/// The splice's walk: each statement's child blocks are spliced before
/// the walk descends into them.
struct Splice<'a>(&'a HashMap<SourceSpan, StatementDesugar>);

impl Splice<'_> {
    /// Splice `stmts`' own statements; a replacement may itself hold a
    /// nested desugared statement (a later item of a multi-item `with`, or
    /// a nested sequence target), so it is re-examined from the same index.
    fn block(&self, stmts: &mut Vec<Stmt>) {
        let mut index = 0;
        while index < stmts.len() {
            if matches!(
                stmts[index].kind,
                StmtKind::With { .. } | StmtKind::Destructure { .. }
            ) && let Some(replacement) = self.0.get(&stmts[index].source_span())
                && replacement.binder.is_none()
            {
                stmts.splice(index..=index, replacement.statements.iter().cloned());
                continue;
            }
            index += 1;
        }
    }

    /// The desugar of a loop target, which names its binder.
    fn loop_target(&self, target: &DestructureTarget) -> Option<(&str, &[Stmt])> {
        self.0.get(&target.source_span()).and_then(|desugar| {
            desugar
                .binder
                .as_deref()
                .map(|binder| (binder, desugar.statements.as_slice()))
        })
    }
}

impl MutVisitor for Splice<'_> {
    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        if let StmtKind::For { target, body, .. } = &mut statement.kind
            && let Some((binder, statements)) = self.loop_target(target)
        {
            body.splice(0..0, statements.iter().cloned());
            *target = binder_target(target, binder);
        }
        for block in child_blocks(statement) {
            self.block(block);
        }
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let ExprKind::Comprehension { clauses, .. } = &mut expr.kind else {
            return;
        };
        for clause in clauses {
            if let ComprehensionClause::For { target, unpack, .. } = clause
                && let Some((binder, statements)) = self.loop_target(target)
            {
                *unpack = statements.first().cloned().map(Box::new);
                **target = binder_target(target, binder);
            }
        }
    }
}

/// The one-leaf target binding `binder` in place of `target`.
fn binder_target(target: &DestructureTarget, binder: &str) -> DestructureTarget {
    let mut leaf = DestructureTarget::new(
        DestructureTargetKind::Declare {
            name: binder.to_string(),
            mode: mojito_ast::ast::BindingMode::Immutable,
        },
        target.span,
    );
    leaf.source.clone_from(&target.source);
    leaf.syntax_id = target.syntax_id;
    leaf
}

/// The statement's directly nested blocks.
fn child_blocks(stmt: &mut Stmt) -> Vec<&mut Vec<Stmt>> {
    match &mut stmt.kind {
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => branches
            .iter_mut()
            .map(|(_, block)| block)
            .chain(orelse.as_mut())
            .collect(),
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            std::iter::once(body).chain(orelse.as_mut()).collect()
        }
        StmtKind::ComptimeFor { body, .. }
        | StmtKind::With { body, .. }
        | StmtKind::Scope(body)
        | StmtKind::Def { body, .. } => vec![body],
        StmtKind::Match { cases, .. } | StmtKind::ComptimeMatch { cases, .. } => {
            cases.iter_mut().map(|case| &mut case.body).collect()
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => std::iter::once(body)
            .chain(except.as_mut().map(|(_, block)| block))
            .chain(orelse.as_mut())
            .chain(finalbody.as_mut())
            .collect(),
        StmtKind::Struct { methods, .. } => {
            methods.iter_mut().map(|method| &mut method.body).collect()
        }
        StmtKind::Trait { methods, .. } => methods
            .iter_mut()
            .filter_map(|method| method.default_body.as_mut())
            .collect(),
        _ => Vec::new(),
    }
}
