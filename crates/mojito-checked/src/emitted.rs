//! The check's emitted nodes as the handoff carries them.
//!
//! Each statement and expression node the check allocated holds the op it
//! was given when it was allocated, as upstream's emitters create each op
//! when they emit it.
//!
//! An op's operands are other emitted nodes, numbered by their position in
//! [`EmittedNodes`], not by the checked arena's ids. A field the check
//! decides after allocation (a declaration's symbol and signature, a call's
//! parameter grouping, a match's plan) is left empty in the op and stands
//! as a fact on the node. Nothing reads these ops yet but
//! `MOJITO_CHECK_EMITTED=1`'s comparison with the checked arena
//! ([`cross_check`]).

use crate::checked::{
    CheckedComprehensionClause, CheckedExpr, CheckedNodeId, CheckedOp, CheckedParamArg,
    CheckedStmt, CheckedStmtId, NodeFacts, NodeMap, SemanticAdjustment,
};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use std::collections::HashMap;

/// Every node the check emitted, and the index from occurrence to node.
#[derive(Debug, Clone, Default)]
pub struct EmittedNodes {
    pub expressions: Vec<EmittedExpr>,
    pub statements: Vec<EmittedStmt>,
    pub index: HashMap<SourceSpan, EmittedSlot>,
}

impl EmittedNodes {
    /// The expression node emitted for the occurrence `at`.
    pub fn expression(&self, at: &SourceSpan) -> Option<&EmittedExpr> {
        match self.index.get(at)? {
            EmittedSlot::Expression(id) => self.expressions.get(id.0 as usize),
            EmittedSlot::Statement(_) => None,
        }
    }

    /// The statement node emitted for the occurrence `at`.
    pub fn statement(&self, at: &SourceSpan) -> Option<&EmittedStmt> {
        match self.index.get(at)? {
            EmittedSlot::Statement(id) => self.statements.get(id.0 as usize),
            EmittedSlot::Expression(_) => None,
        }
    }
}

/// One emitted expression node.
#[derive(Debug, Clone)]
pub struct EmittedExpr {
    pub at: SourceSpan,
    /// Set when the node is allocated and never changed; `None` only where
    /// an operand had no node, which the handoff counts
    /// (`emit.opless_nodes`).
    pub op: Option<CheckedOp>,
    /// For each operand position whose lifted applications the check
    /// decides (a call's bracket argument, a subscript key, a function
    /// type), the nodes under it in walk order: the candidates the
    /// applications are chosen from.
    pub applications: Vec<Vec<CheckedNodeId>>,
}

/// One emitted statement node.
#[derive(Debug, Clone)]
pub struct EmittedStmt {
    pub at: SourceSpan,
    /// As [`EmittedExpr::op`].
    pub op: Option<crate::checked::CheckedStmtOp>,
    /// A variable declaration's annotation's nodes in walk order, the
    /// candidates its lifted applications are chosen from.
    pub applications: Vec<CheckedNodeId>,
}

/// A node's arena and position in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmittedSlot {
    Statement(CheckedStmtId),
    Expression(CheckedNodeId),
}

/// Compare the emitted nodes with the checked arena's `expressions` and
/// `statements`, built from the check's `facts`: every arena node has an
/// emitted node of its kind at its occurrence, holding an op of its
/// variant, and an expression op the syntax alone decides equals the
/// arena's once both name their operands by occurrence. Each kind of
/// mismatch is counted (`emit.check.*`) and its first five are printed.
pub(crate) fn cross_check(
    emitted: &EmittedNodes,
    expressions: &[CheckedExpr],
    statements: &[CheckedStmt],
    facts: &HashMap<SourceSpan, NodeFacts>,
) {
    let mut report = Report::default();
    let mut locations = Locations::default();
    for statement in statements {
        let at = statement.location();
        match emitted.statement(&at).map(|node| node.op.as_ref()) {
            None => report.note("emit.check.unemitted", &at, String::new),
            Some(None) => report.note("emit.check.opless", &at, String::new),
            Some(Some(op))
                if std::mem::discriminant(op) != std::mem::discriminant(&statement.op) =>
            {
                report.note("emit.check.variant_mismatches", &at, || {
                    format!("emitted {op:?}\n  arena {:?}", statement.op)
                });
            }
            Some(Some(_)) => {}
        }
    }
    for expression in expressions {
        let at = expression.location();
        let op = match emitted.expression(&at).map(|node| node.op.as_ref()) {
            None => {
                report.note("emit.check.unemitted", &at, String::new);
                continue;
            }
            Some(None) => {
                report.note("emit.check.opless", &at, String::new);
                continue;
            }
            Some(Some(op)) => op,
        };
        if std::mem::discriminant(op) != std::mem::discriminant(&expression.op) {
            report.note("emit.check.variant_mismatches", &at, || {
                format!("emitted {op:?}\n  arena {:?}", expression.op)
            });
            continue;
        }
        if !syntactic(op) {
            continue;
        }
        let ours = comparable(op.map_operands(&mut EmittedOperands {
            emitted,
            facts,
            locations: &mut locations,
        }));
        let theirs = comparable(expression.op.map_operands(&mut ArenaOperands {
            expressions,
            statements,
            locations: &mut locations,
        }));
        let (ours, theirs) = (format!("{ours:?}"), format!("{theirs:?}"));
        report.compared += 1;
        if ours != theirs {
            report.note("emit.check.op_mismatches", &at, || {
                format!("emitted {ours}\n  arena {theirs}")
            });
        }
    }
    report.finish();
}

/// Whether the syntax alone decides the emitted op `op`, once a call's
/// parameter grouping and a subscript's key are set aside: not a type
/// value, whose type and applications are the check's, nor a comprehension
/// whose written target is no one name, which the arena reads after its
/// desugar is spliced.
fn syntactic(op: &CheckedOp) -> bool {
    match op {
        CheckedOp::TypeValue { .. } => false,
        CheckedOp::Comprehension { clauses, .. } => clauses.iter().all(|clause| match clause {
            CheckedComprehensionClause::For { var, .. } => !var.is_empty(),
            CheckedComprehensionClause::If(_) => true,
        }),
        _ => true,
    }
}

/// `op` with the fields the check decides after allocation cleared.
fn comparable(op: CheckedOp) -> CheckedOp {
    match op {
        CheckedOp::Call {
            name, args, kwargs, ..
        } => CheckedOp::Call {
            name,
            param_args: Vec::new(),
            args,
            kwargs,
        },
        CheckedOp::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => CheckedOp::Invoke {
            callee,
            param_args: Vec::new(),
            args,
            kwargs,
        },
        CheckedOp::Index { object, .. } => CheckedOp::Index {
            object,
            index: CheckedParamArg::Attribute {
                value: None,
                applications: Vec::new(),
            },
        },
        op => op,
    }
}

/// Whether a sugar whose emitted op is `op` is one the arena replaces by
/// the construction the check spelled for it.
const fn substituted(op: &CheckedOp) -> bool {
    matches!(
        op,
        CheckedOp::BraceLit(_)
            | CheckedOp::InitializerList { .. }
            | CheckedOp::ListLit(_)
            | CheckedOp::TString { .. }
            | CheckedOp::Identifier(_)
            | CheckedOp::Member { .. }
            | CheckedOp::Call { .. }
            | CheckedOp::MethodCall { .. }
            | CheckedOp::Invoke { .. }
            | CheckedOp::Index { .. }
            | CheckedOp::MultiIndex { .. }
            | CheckedOp::Int(_)
            | CheckedOp::Float(_)
            | CheckedOp::Bool(_)
            | CheckedOp::Str(_)
    )
}

/// Occurrences numbered in the order they are first named: an operand of
/// either side becomes its occurrence's number.
#[derive(Default)]
struct Locations {
    expressions: HashMap<SourceSpan, u32>,
    statements: HashMap<SourceSpan, u32>,
}

impl Locations {
    fn expression(&mut self, at: SourceSpan) -> CheckedNodeId {
        let next = self.expressions.len() as u32;
        CheckedNodeId(*self.expressions.entry(at).or_insert(next))
    }

    fn statement(&mut self, at: SourceSpan) -> CheckedStmtId {
        let next = self.statements.len() as u32;
        CheckedStmtId(*self.statements.entry(at).or_insert(next))
    }
}

/// An emitted op's operands named by occurrence: a spelled sugar's by its
/// construction's, which the arena holds in its place.
struct EmittedOperands<'a> {
    emitted: &'a EmittedNodes,
    facts: &'a HashMap<SourceSpan, NodeFacts>,
    locations: &'a mut Locations,
}

impl EmittedOperands<'_> {
    /// The occurrence the arena holds for the emitted expression at `at`.
    fn substitute(&self, mut at: SourceSpan) -> SourceSpan {
        while let Some(node) = self.emitted.expression(&at)
            && node.op.as_ref().is_some_and(substituted)
            && let Some(construction) = self
                .facts
                .get(&at)
                .and_then(|facts| facts.adjustment.as_deref())
                .and_then(spelled_construction)
            && construction != at
        {
            at = construction;
        }
        at
    }
}

impl NodeMap for EmittedOperands<'_> {
    fn expression(&mut self, id: CheckedNodeId) -> CheckedNodeId {
        let Some(node) = self.emitted.expressions.get(id.0 as usize) else {
            return CheckedNodeId(u32::MAX);
        };
        let at = self.substitute(node.at.clone());
        self.locations.expression(at)
    }

    fn statement(&mut self, id: CheckedStmtId) -> CheckedStmtId {
        match self.emitted.statements.get(id.0 as usize) {
            Some(node) => self.locations.statement(node.at.clone()),
            None => CheckedStmtId(u32::MAX),
        }
    }
}

/// The occurrence of the construction a sugar's `adjustment` spelled.
fn spelled_construction(adjustment: &SemanticAdjustment) -> Option<SourceSpan> {
    match adjustment {
        SemanticAdjustment::SpelledConstruction { construction } => {
            Some(construction.0.source_span())
        }
        SemanticAdjustment::ComptimeApplication {
            operation: Some(operation),
            ..
        } => match &**operation {
            SemanticAdjustment::SpelledConstruction { construction } => {
                Some(construction.0.source_span())
            }
            _ => None,
        },
        _ => None,
    }
}

/// An arena op's operands named by occurrence.
struct ArenaOperands<'a> {
    expressions: &'a [CheckedExpr],
    statements: &'a [CheckedStmt],
    locations: &'a mut Locations,
}

impl NodeMap for ArenaOperands<'_> {
    fn expression(&mut self, id: CheckedNodeId) -> CheckedNodeId {
        match self.expressions.get(id.0 as usize) {
            Some(node) => self.locations.expression(node.location()),
            None => CheckedNodeId(u32::MAX),
        }
    }

    fn statement(&mut self, id: CheckedStmtId) -> CheckedStmtId {
        match self.statements.get(id.0 as usize) {
            Some(node) => self.locations.statement(node.location()),
            None => CheckedStmtId(u32::MAX),
        }
    }
}

/// The mismatches found, by kind.
#[derive(Default)]
struct Report {
    counts: HashMap<&'static str, u64>,
    /// How many expression ops were compared whole.
    compared: u64,
}

impl Report {
    /// Count one mismatch of `kind` at `at`, printing it with `detail`
    /// while it is among its kind's first five.
    fn note(&mut self, kind: &'static str, at: &SourceSpan, detail: impl FnOnce() -> String) {
        let count = self.counts.entry(kind).or_default();
        *count += 1;
        if *count <= 5 {
            eprintln!("{kind}: {at:?}\n  {}", detail());
        }
    }

    /// Count every kind, a kind with no mismatch as zero.
    fn finish(self) {
        timing::count("emit.check.compared", self.compared);
        for kind in [
            "emit.check.unemitted",
            "emit.check.opless",
            "emit.check.variant_mismatches",
            "emit.check.op_mismatches",
        ] {
            let count = self.counts.get(kind).copied().unwrap_or(0);
            timing::count(kind, count);
            if count > 0 {
                eprintln!("{kind}: {count}");
            }
        }
    }
}
