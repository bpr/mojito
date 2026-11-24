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
//! as a fact on the node. Under `MOJITO_CHECK_EMITTED=1` the seal
//! ([`seal`]) composes each op with its node's facts into checked tables,
//! and [`compare`] checks them against the arena built from the records.

mod seal;

pub(crate) use seal::seal;

use crate::checked::{
    CheckedArena, CheckedExpr, CheckedNodeId, CheckedOp, CheckedStmt, CheckedStmtId,
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
    /// The check's desugars, keyed as it keys them: a `with` statement's
    /// and a sequence destructuring's by the statement, a loop target's by
    /// the target.
    pub desugars: HashMap<SourceSpan, EmittedDesugar>,
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
    /// A comprehension's clauses' written loop targets, `None` for an `if`
    /// clause: the occurrence a target's desugar is keyed by.
    pub loop_targets: Vec<Option<SourceSpan>>,
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
    /// A `for` statement's written loop target: the occurrence its
    /// desugar is keyed by.
    pub loop_target: Option<SourceSpan>,
}

/// One desugar the check made: the nodes of the statements that replace
/// the statement, or, for a loop target, that run first in each iteration
/// of a loop binding `binder`.
#[derive(Debug, Clone)]
pub struct EmittedDesugar {
    pub statements: Vec<CheckedStmtId>,
    pub binder: Option<String>,
}

/// A node's arena and position in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmittedSlot {
    Statement(CheckedStmtId),
    Expression(CheckedNodeId),
}

/// Compare the tables `sealed` from the emitted nodes, `missing` of whose
/// nodes the seal found no emitted node for, with the tables `built` from
/// the check's records, whole: node for node by id, and their roots. Each
/// kind of difference is counted (`emit.seal.*`) and its first five are
/// printed; a difference shifts every later id, so the first is the one to
/// read.
pub(crate) fn compare(sealed: &CheckedArena, built: &CheckedArena, missing: u64) {
    let expressions = differences(
        "emit.seal.expression_mismatches",
        &sealed.nodes,
        &built.nodes,
        CheckedExpr::location,
    );
    let statements = differences(
        "emit.seal.statement_mismatches",
        &sealed.statements,
        &built.statements,
        CheckedStmt::location,
    );
    let roots = u64::from(sealed.roots != built.roots)
        + u64::from(sealed.wrapper_roots != built.wrapper_roots);
    if roots > 0 {
        eprintln!(
            "emit.seal.root_mismatches:\n  sealed {:?} {:?}\n  built  {:?} {:?}",
            sealed.roots, sealed.wrapper_roots, built.roots, built.wrapper_roots
        );
    }
    for (kind, count) in [
        ("emit.seal.missing", missing),
        ("emit.seal.expressions", built.nodes.len() as u64),
        ("emit.seal.statements", built.statements.len() as u64),
        ("emit.seal.expression_mismatches", expressions),
        ("emit.seal.statement_mismatches", statements),
        ("emit.seal.root_mismatches", roots),
    ] {
        timing::count(kind, count);
    }
    for (kind, count) in [
        ("emit.seal.missing", missing),
        ("emit.seal.expression_mismatches", expressions),
        ("emit.seal.statement_mismatches", statements),
        ("emit.seal.root_mismatches", roots),
    ] {
        if count > 0 {
            eprintln!("{kind}: {count}");
        }
    }
}

/// Whether a sugar whose emitted op is `op` is one the sealed tables
/// replace by the construction the check spelled for it.
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

/// How many nodes of `sealed` and `built` differ by id, a node one side
/// lacks counting as a difference; the first five are printed under
/// `kind` at their `location`.
fn differences<T: std::fmt::Debug>(
    kind: &str,
    sealed: &[T],
    built: &[T],
    location: impl Fn(&T) -> SourceSpan,
) -> u64 {
    let mut count = 0u64;
    for (id, node) in built.iter().enumerate() {
        let theirs = format!("{node:?}");
        let ours = sealed.get(id).map(|node| format!("{node:?}"));
        if ours.as_deref() != Some(theirs.as_str()) {
            count += 1;
            if count <= 5 {
                eprintln!(
                    "{kind}: id {id} at {:?}\n  sealed {}\n  built  {theirs}",
                    location(node),
                    ours.as_deref().unwrap_or("(none)")
                );
            }
        }
    }
    count + sealed.len().saturating_sub(built.len()) as u64
}
