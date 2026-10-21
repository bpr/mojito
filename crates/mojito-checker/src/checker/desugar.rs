//! Statements the checker desugars into ordinary statements: a `with`
//! statement (`with_stmt.rs`) and a sequence destructuring assignment
//! (`destructure.rs`). Each desugar is checked in place, recorded by its
//! statement, and spliced into the final checked tree, so HIR, MIR, and the
//! backends see only the ordinary statements. [`Synth`] builds the
//! synthesized nodes, for these desugars and for `match`'s commands.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_common::literal::IntLiteral;
use mojito_common::token::SyntaxId;

/// A checked statement's desugar: the statements that replace it.
#[derive(Debug, Clone)]
pub(super) struct StatementDesugar {
    pub(super) statements: Vec<Stmt>,
}

/// Replace every desugared statement in `stmts` (recursively, through every
/// nested block and declaration body) by its recorded desugar. A `with` or
/// destructuring assignment without a recorded desugar stays as written.
pub(super) fn splice_desugars(
    stmts: &mut Vec<Stmt>,
    desugars: &HashMap<SourceSpan, StatementDesugar>,
) {
    let mut index = 0;
    while index < stmts.len() {
        if matches!(
            stmts[index].kind,
            StmtKind::With { .. } | StmtKind::Destructure { .. }
        ) && let Some(replacement) = desugars.get(&stmts[index].source_span())
        {
            // The replacement may itself hold a nested desugared statement
            // (a later item of a multi-item `with`, a statement written in
            // its body, or a nested sequence target); re-examine from the
            // same index.
            stmts.splice(index..=index, replacement.statements.iter().cloned());
            continue;
        }
        splice_nested(&mut stmts[index], desugars);
        index += 1;
    }
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

fn splice_nested(stmt: &mut Stmt, desugars: &HashMap<SourceSpan, StatementDesugar>) {
    match &mut stmt.kind {
        StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
            for (_, block) in branches {
                splice_desugars(block, desugars);
            }
            if let Some(block) = orelse {
                splice_desugars(block, desugars);
            }
        }
        StmtKind::While { body, orelse, .. } | StmtKind::For { body, orelse, .. } => {
            splice_desugars(body, desugars);
            if let Some(block) = orelse {
                splice_desugars(block, desugars);
            }
        }
        StmtKind::ComptimeFor { body, .. }
        | StmtKind::With { body, .. }
        | StmtKind::Scope(body)
        | StmtKind::Def { body, .. } => splice_desugars(body, desugars),
        StmtKind::Match { cases, .. } | StmtKind::ComptimeMatch { cases, .. } => {
            for case in cases {
                splice_desugars(&mut case.body, desugars);
            }
        }
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => {
            splice_desugars(body, desugars);
            if let Some((_, block)) = except {
                splice_desugars(block, desugars);
            }
            if let Some(block) = orelse {
                splice_desugars(block, desugars);
            }
            if let Some(block) = finalbody {
                splice_desugars(block, desugars);
            }
        }
        StmtKind::Struct { methods, .. } => {
            for method in methods {
                splice_desugars(&mut method.body, desugars);
            }
        }
        StmtKind::Trait { methods, .. } => {
            for method in methods {
                if let Some(body) = &mut method.default_body {
                    splice_desugars(body, desugars);
                }
            }
        }
        _ => {}
    }
}
