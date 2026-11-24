//! The check's emitted nodes as the handoff carries them.
//!
//! Each statement and expression node the check allocated holds the op it
//! was given when it was allocated, as upstream's emitters create each op
//! when they emit it.
//!
//! An op's operands are other emitted nodes, numbered by their position in
//! [`EmittedNodes`], not by the checked tables' ids. A field the check
//! decides after allocation (a declaration's symbol and signature, a call's
//! parameter grouping, a match's plan, a destructuring's leaf bindings) is
//! left empty in the op and stands as a fact on the node, as an attribute
//! on an op: every fact the handoff carries sits on a node. The seal ([`seal`]),
//! which the check runs at its end, composes each op with its node's facts
//! into the checked tables, and rejects an occurrence the check emitted no
//! node for ([`Unsealed`]). The module's root statements and each default
//! wrapper's body are the nodes the check entered for them, as upstream's
//! module op holds its body ops and a wrapper's function op its body.

mod seal;

pub use seal::seal;

use crate::checked::{CheckedNodeId, CheckedOp, CheckedStmtId, NodeFacts};
use mojito_common::token::SourceSpan;
use std::collections::{BTreeMap, HashMap};

/// Every node the check emitted, and the index from occurrence to node.
#[derive(Debug, Clone, Default)]
pub struct EmittedNodes {
    pub expressions: Vec<EmittedExpr>,
    pub statements: Vec<EmittedStmt>,
    pub index: HashMap<SourceSpan, EmittedSlot>,
    /// The module's root statements' nodes, in source order: what the
    /// module op holds. A root no statement node holds is
    /// [`EmittedNodes::MISSING`].
    pub roots: Vec<CheckedStmtId>,
    /// Each default wrapper's body's nodes, by the wrapper's position among
    /// the check's wrappers: what its function op holds. A wrapper the
    /// check never entered has none.
    pub wrapper_bodies: BTreeMap<usize, Vec<CheckedStmtId>>,
    /// The check's desugars, keyed as it keys them: a `with` statement's
    /// and a sequence destructuring's by the statement, a loop target's by
    /// the target.
    pub desugars: HashMap<SourceSpan, EmittedDesugar>,
}

impl EmittedNodes {
    /// The slot a root or body statement takes when no statement node
    /// holds its occurrence: the seal rejects it.
    pub const MISSING: CheckedStmtId = CheckedStmtId(u32::MAX);

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

    /// Every record the check left, by occurrence: the statements' facts,
    /// then the expressions'.
    pub fn records(&self) -> impl Iterator<Item = (&SourceSpan, &NodeFacts)> {
        self.statements
            .iter()
            .filter_map(|node| Some((&node.at, node.facts.as_deref()?)))
            .chain(
                self.expressions
                    .iter()
                    .filter_map(|node| Some((&node.at, node.facts.as_deref()?))),
            )
    }
}

/// One emitted expression node.
#[derive(Debug, Clone)]
pub struct EmittedExpr {
    pub at: SourceSpan,
    /// Set when the node is allocated and never changed; `None` only where
    /// an operand had no node, which the handoff counts
    /// (`emit.opless_nodes`) and the seal rejects.
    pub op: Option<CheckedOp>,
    /// For each operand position whose lifted applications the check
    /// decides (a call's bracket argument, a subscript key, a function
    /// type), the nodes under it in walk order: the candidates the
    /// applications are chosen from.
    pub applications: Vec<Vec<CheckedNodeId>>,
    /// A comprehension's clauses' written loop targets, `None` for an `if`
    /// clause: the occurrence a target's desugar is keyed by.
    pub loop_targets: Vec<Option<SourceSpan>>,
    /// The facts the check set on the node, as attributes on an op; `None`
    /// where it set none.
    pub facts: Option<Box<NodeFacts>>,
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
    /// As [`EmittedExpr::facts`].
    pub facts: Option<Box<NodeFacts>>,
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

/// The occurrences the seal found no emitted node or op for: how many,
/// and the first whose location it knew.
#[derive(Debug, Default)]
pub struct Unsealed {
    pub(crate) count: u64,
    pub(crate) first: Option<SourceSpan>,
}

impl Unsealed {
    /// One more occurrence with no node, at `at` where it is known.
    fn miss(&mut self, at: Option<&SourceSpan>) {
        self.count += 1;
        if let Some(at) = at {
            self.locate(at);
        }
    }

    /// `at` as the first located occurrence with no node, unless one is
    /// already known.
    fn locate(&mut self, at: &SourceSpan) {
        if self.first.is_none() {
            self.first = Some(at.clone());
        }
    }
}

impl std::fmt::Display for Unsealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the check emitted no node for {} occurrence(s)",
            self.count
        )?;
        match &self.first {
            Some(at) => write!(
                f,
                ", the first at {}:{:?}",
                at.source.as_deref().unwrap_or("<main>"),
                at.span
            ),
            None => Ok(()),
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
