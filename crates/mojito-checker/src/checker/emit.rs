//! The check's emitted nodes: one per statement and expression occurrence,
//! allocated when the check enters the statement that holds it, or the
//! construction it spells, or the match tests it plans, each holding its op
//! and carrying the facts the check decides there, as upstream's emitters
//! create each op when they emit it and set each decided fact on it as an
//! attribute.
//!
//! A node's op is set when the node is allocated, from its syntax, with its
//! operands the nodes allocated for its operands (`ops`), and never changes:
//! what the check decides later is a fact on the node. So a probe's
//! roll-back drops the nodes it allocated, ops and all, and restores only
//! records.
//!
//! An index finds an occurrence's node. An occurrence no node holds yet
//! keeps a pending record, which moves onto the node when one is allocated
//! for it; a span is never in both. What is still pending when the check
//! ends is syntax the check typed only to evaluate it, whose result it
//! folded into an attribute of the op that holds it, and the handoff
//! discards it: every fact handed off is an attribute on an emitted op.
//!
//! The emitter logs and journals as the fact stores do (`fact_store`): a
//! body site's writes are carried by log range, and a probe's writes and
//! allocations are undone when it rolls back.

mod ops;

use mojito_ast::ast::{Expr, Stmt};
use mojito_ast::visit::{self, Visitor};
use mojito_checked::checked::{
    CheckedNodeId, CheckedStmtId, MatchPlan, NodeFacts, NodeMap, PlannedCommand,
};
use mojito_checked::emitted::{EmittedExpr, EmittedNodes, EmittedSlot, EmittedStmt};
use mojito_checked::fact_store::{FactMap, ProbeMark};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// The statement and expression nodes the check has allocated, the index
/// from occurrence to node, and the pending records.
#[derive(Debug, Default)]
pub struct Emitter {
    statements: Vec<EmittedStmt>,
    expressions: Vec<EmittedExpr>,
    index: HashMap<SourceSpan, EmittedSlot>,
    pending: FactMap<SourceSpan, NodeFacts>,
    /// Every node write, in order.
    log: Vec<EmittedSlot>,
    /// Each probed node write's slot and the record it displaced, oldest
    /// first.
    undo: Vec<(EmittedSlot, Option<Box<NodeFacts>>)>,
    probes: u32,
}

impl Emitter {
    /// The record at `at`: its node's when one holds the occurrence, its
    /// pending record otherwise.
    pub fn get(&self, at: &SourceSpan) -> Option<&NodeFacts> {
        match self.index.get(at) {
            Some(slot) => self.facts(*slot),
            None => self.pending.get(at),
        }
    }

    /// Every record: the nodes' in allocation order, then the pending.
    pub fn iter(&self) -> impl Iterator<Item = (&SourceSpan, &NodeFacts)> {
        self.statements
            .iter()
            .map(|node| (&node.at, &node.facts))
            .chain(self.expressions.iter().map(|node| (&node.at, &node.facts)))
            .filter_map(|(at, facts)| Some((at, facts.as_deref()?)))
            .chain(self.pending.iter())
    }

    /// Emit `statement` and every statement and expression under it,
    /// unless the check entered it before.
    pub(super) fn enter_statement(&mut self, statement: &Stmt) {
        if !self.index.contains_key(&statement.source_span()) {
            let start = self.allocated();
            visit::walk_stmt(&mut Allocation(self), statement);
            visit::walk_stmt(
                &mut Operations {
                    emitter: self,
                    start,
                },
                statement,
            );
        }
    }

    /// Emit `expression` and every statement and expression under it that
    /// no node holds yet: a construction the check spelled.
    pub(super) fn enter_expression(&mut self, expression: &Expr) {
        if !self.index.contains_key(&expression.source_span()) {
            let start = self.allocated();
            visit::walk_expr(&mut Allocation(self), expression);
            visit::walk_expr(
                &mut Operations {
                    emitter: self,
                    start,
                },
                expression,
            );
        }
    }

    /// Emit the syntax match `plan` synthesized: its anchor, each command's
    /// test and declaration, each path read, and each `comptime match`
    /// case's condition.
    pub(super) fn enter_plan(&mut self, plan: &MatchPlan) {
        if let Some(anchor) = &plan.anchor {
            self.enter_statement(anchor);
        }
        for command in plan.cases.iter().flatten() {
            self.enter_command(command);
        }
        for read in plan.reads.iter().flatten() {
            self.enter_statement(&read.declaration);
        }
        for condition in plan.conditions.iter().flatten() {
            self.enter_expression(condition);
        }
    }

    /// The record at `at`, created if absent: the write path.
    pub(super) fn record(&mut self, at: SourceSpan) -> &mut NodeFacts {
        match self.index.get(&at).copied() {
            Some(slot) => self.write(slot).get_or_insert_default(),
            None => self.pending.entry(at).or_default(),
        }
    }

    /// The record at `at` handed out for mutation; only an existing one
    /// counts as written.
    pub(super) fn get_mut(&mut self, at: &SourceSpan) -> Option<&mut NodeFacts> {
        match self.index.get(at).copied() {
            Some(slot) if self.facts(slot).is_some() => self.write(slot).as_deref_mut(),
            Some(_) => None,
            None => self.pending.get_mut(at),
        }
    }

    /// Take the record at `at` out; a removal is journaled, not logged.
    pub(super) fn remove(&mut self, at: &SourceSpan) -> Option<NodeFacts> {
        let Some(slot) = self.index.get(at).copied() else {
            return self.pending.remove(at);
        };
        let removed = self.facts_mut(slot).take();
        if self.probes > 0 && removed.is_some() {
            self.undo.push((slot, removed.clone()));
        }
        removed.map(|facts| *facts)
    }

    /// Replace the record at `at` wholesale.
    pub(super) fn insert(&mut self, at: SourceSpan, facts: NodeFacts) {
        match self.index.get(&at).copied() {
            Some(slot) => *self.write(slot) = Some(Box::new(facts)),
            None => {
                self.pending.insert(at, facts);
            }
        }
    }

    /// The arena and log positions: what is allocated and written from here
    /// on is `written(mark, ..)`.
    pub(super) const fn mark(&self) -> EmitMark {
        EmitMark {
            statements: self.statements.len(),
            expressions: self.expressions.len(),
            log: self.log.len(),
            pending: self.pending.mark(),
        }
    }

    /// The occurrence of every record written between `from` and `to`,
    /// repeats included.
    pub(super) fn written<'a>(
        &'a self,
        from: &EmitMark,
        to: &EmitMark,
    ) -> impl Iterator<Item = &'a SourceSpan> {
        self.log[from.log..to.log]
            .iter()
            .map(|slot| self.at(*slot))
            .chain(self.pending.logged(from.pending..to.pending))
    }

    /// Start journaling writes and allocations; `roll_back(mark)` undoes
    /// them.
    pub(super) const fn open_probe(&mut self) -> EmitProbe {
        self.probes += 1;
        EmitProbe {
            statements: self.statements.len(),
            expressions: self.expressions.len(),
            log: self.log.len(),
            undo: self.undo.len(),
            pending: self.pending.open_probe(),
        }
    }

    /// Undo every write since `mark` was opened, newest first, then drop
    /// the nodes allocated since, with their ops and index entries, and the
    /// slots logged since. A pending record a dropped node took is
    /// restored by the pending store's own roll-back.
    pub(super) fn roll_back(&mut self, mark: EmitProbe) {
        for (slot, previous) in self.undo.split_off(mark.undo).into_iter().rev() {
            *self.facts_mut(slot) = previous;
        }
        for at in self
            .statements
            .drain(mark.statements..)
            .map(|node| node.at)
            .chain(
                self.expressions
                    .drain(mark.expressions..)
                    .map(|node| node.at),
            )
        {
            self.index.remove(&at);
        }
        self.log.truncate(mark.log);
        self.pending.roll_back(mark.pending);
        self.probes -= 1;
    }

    /// Copy the window `from..to` of `previous`, another pass over the same
    /// program: allocate every node it allocated there that this emitter
    /// lacks, with its op relocated onto this emitter's nodes, then set the
    /// final record of every occurrence it wrote there. A record removed
    /// after the window is not copied.
    pub(super) fn copy_window(&mut self, previous: &Self, from: &EmitMark, to: &EmitMark) {
        let start = self.allocated();
        for node in &previous.statements[from.statements..to.statements] {
            self.allocate(node.at.clone(), ArenaKind::Statement);
        }
        for node in &previous.expressions[from.expressions..to.expressions] {
            self.allocate(node.at.clone(), ArenaKind::Expression);
        }
        let mut relocation = Relocation {
            previous,
            this: self,
            start,
        };
        for position in from.statements..to.statements {
            relocation.statement(CheckedStmtId(slot_position(position)));
        }
        for position in from.expressions..to.expressions {
            relocation.expression(CheckedNodeId(slot_position(position)));
        }
        for at in previous.written(from, to) {
            if let Some(facts) = previous.get(at) {
                self.insert(at.clone(), facts.clone());
            }
        }
    }

    /// Rewrite every record in place, unlogged: for after the check.
    pub(super) fn for_each_record_mut(&mut self, mut rewrite: impl FnMut(&mut NodeFacts)) {
        self.statements
            .iter_mut()
            .map(|node| &mut node.facts)
            .chain(self.expressions.iter_mut().map(|node| &mut node.facts))
            .filter_map(|facts| facts.as_deref_mut())
            .for_each(&mut rewrite);
        self.pending.retain(|_, facts| {
            rewrite(facts);
            true
        });
    }

    /// The emitted nodes, each holding the facts set on it, less any
    /// record a cleared fact left empty. A record still pending is on
    /// syntax the check typed only to evaluate it, whose result it folded
    /// into an attribute of the op that holds it: the handoff discards it.
    pub(super) fn into_nodes(self) -> EmittedNodes {
        let (mut on_nodes, mut opless) = (0u64, 0u64);
        let mut seal = |op: bool, facts: &mut Option<Box<NodeFacts>>| {
            opless += u64::from(!op);
            if facts.as_ref().is_some_and(|facts| facts.is_empty()) {
                *facts = None;
            }
            on_nodes += u64::from(facts.is_some());
        };
        let mut statements = self.statements;
        for node in &mut statements {
            seal(node.op.is_some(), &mut node.facts);
        }
        let mut expressions = self.expressions;
        for node in &mut expressions {
            seal(node.op.is_some(), &mut node.facts);
        }
        let pending = self.pending.into_inner();
        debug_assert!(
            pending.values().all(|facts| facts.match_plan.is_none()
                && facts.match_warnings.is_none()
                && facts.declared_leaves.is_none()),
            "a statement's record was left on no node"
        );
        timing::count("emit.node_records", on_nodes);
        timing::count(
            "emit.discarded_records",
            pending.values().filter(|facts| !facts.is_empty()).count() as u64,
        );
        timing::count("emit.opless_nodes", opless);
        EmittedNodes {
            expressions,
            statements,
            index: self.index,
            desugars: HashMap::new(),
        }
    }

    fn facts(&self, slot: EmittedSlot) -> Option<&NodeFacts> {
        match slot {
            EmittedSlot::Statement(id) => self.statements[id.0 as usize].facts.as_deref(),
            EmittedSlot::Expression(id) => self.expressions[id.0 as usize].facts.as_deref(),
        }
    }

    fn facts_mut(&mut self, slot: EmittedSlot) -> &mut Option<Box<NodeFacts>> {
        match slot {
            EmittedSlot::Statement(id) => &mut self.statements[id.0 as usize].facts,
            EmittedSlot::Expression(id) => &mut self.expressions[id.0 as usize].facts,
        }
    }

    /// The node counts of both arenas: a node at or past them was
    /// allocated since.
    const fn allocated(&self) -> Allocated {
        Allocated {
            statements: self.statements.len(),
            expressions: self.expressions.len(),
        }
    }

    /// Allocate a node for `at` unless one holds it, moving its pending
    /// record onto it; the node's slot either way.
    fn allocate(&mut self, at: SourceSpan, kind: ArenaKind) -> EmittedSlot {
        let vacant = match self.index.entry(at) {
            Entry::Occupied(occupied) => return *occupied.get(),
            Entry::Vacant(vacant) => vacant,
        };
        let facts = self.pending.remove(vacant.key()).map(Box::new);
        let at = vacant.key().clone();
        let slot = match kind {
            ArenaKind::Statement => {
                let id = CheckedStmtId(slot_position(self.statements.len()));
                self.statements.push(EmittedStmt {
                    at,
                    op: None,
                    applications: Vec::new(),
                    loop_target: None,
                    facts,
                });
                EmittedSlot::Statement(id)
            }
            ArenaKind::Expression => {
                let id = CheckedNodeId(slot_position(self.expressions.len()));
                self.expressions.push(EmittedExpr {
                    at,
                    op: None,
                    applications: Vec::new(),
                    loop_targets: Vec::new(),
                    facts,
                });
                EmittedSlot::Expression(id)
            }
        };
        *vacant.insert(slot)
    }

    fn at(&self, slot: EmittedSlot) -> &SourceSpan {
        match slot {
            EmittedSlot::Statement(id) => &self.statements[id.0 as usize].at,
            EmittedSlot::Expression(id) => &self.expressions[id.0 as usize].at,
        }
    }

    /// A node's record slot handed out for a write: logged, and journaled
    /// under a probe.
    fn write(&mut self, slot: EmittedSlot) -> &mut Option<Box<NodeFacts>> {
        self.log.push(slot);
        if self.probes > 0 {
            let previous = self.facts(slot).cloned().map(Box::new);
            self.undo.push((slot, previous));
        }
        self.facts_mut(slot)
    }

    fn enter_command(&mut self, command: &PlannedCommand) {
        match command {
            PlannedCommand::Equal { test, .. } | PlannedCommand::ListLength { test, .. } => {
                self.enter_expression(test);
            }
            PlannedCommand::EnumTag { tag, test, .. } => {
                self.enter_statement(tag);
                self.enter_expression(test);
            }
            PlannedCommand::Bind { declaration, .. } => self.enter_statement(declaration),
            PlannedCommand::Alternatives(alternatives) => {
                for command in alternatives.iter().flatten() {
                    self.enter_command(command);
                }
            }
        }
    }
}

/// An emitter's arena and log positions at one moment, for a body site's
/// window.
#[derive(Debug, Clone, Copy)]
pub(super) struct EmitMark {
    statements: usize,
    expressions: usize,
    log: usize,
    pending: usize,
}

/// An emitter's positions when a probe opened.
#[derive(Debug, Clone, Copy)]
pub(super) struct EmitProbe {
    statements: usize,
    expressions: usize,
    log: usize,
    undo: usize,
    pending: ProbeMark,
}

#[derive(Debug, Clone, Copy)]
enum ArenaKind {
    Statement,
    Expression,
}

/// Both arenas' node counts at one moment.
#[derive(Debug, Clone, Copy)]
struct Allocated {
    statements: usize,
    expressions: usize,
}

/// The walk that allocates a subtree.
struct Allocation<'a>(&'a mut Emitter);

impl Visitor for Allocation<'_> {
    fn visit_stmt(&mut self, statement: &Stmt) {
        self.0
            .allocate(statement.source_span(), ArenaKind::Statement);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        self.0.allocate(expr.source_span(), ArenaKind::Expression);
    }
}

/// The walk that sets the op of every node of a subtree allocated since
/// `start`, once the subtree is allocated.
struct Operations<'a> {
    emitter: &'a mut Emitter,
    start: Allocated,
}

impl Visitor for Operations<'_> {
    fn visit_stmt(&mut self, statement: &Stmt) {
        let Some(&EmittedSlot::Statement(id)) = self.emitter.index.get(&statement.source_span())
        else {
            return;
        };
        let position = id.0 as usize;
        if position < self.start.statements {
            return;
        }
        let ops = ops::Ops {
            index: &self.emitter.index,
        };
        if let Ok(op) = ops.statement(statement) {
            let node = &mut self.emitter.statements[position];
            node.op = Some(op.op);
            node.applications = op.applications;
            node.loop_target = op.loop_target;
        }
    }

    fn visit_expr(&mut self, expression: &Expr) {
        let Some(&EmittedSlot::Expression(id)) = self.emitter.index.get(&expression.source_span())
        else {
            return;
        };
        let position = id.0 as usize;
        if position < self.start.expressions {
            return;
        }
        let ops = ops::Ops {
            index: &self.emitter.index,
        };
        if let Ok(op) = ops.expression(expression) {
            let node = &mut self.emitter.expressions[position];
            node.op = Some(op.op);
            node.applications = op.applications;
            node.loop_targets = op.loop_targets;
        }
    }
}

/// A previous pass's nodes mapped onto this emitter's: each operand to the
/// node this emitter holds for its occurrence, allocated if it holds none,
/// and a node allocated since `start` given the previous op relocated.
struct Relocation<'a> {
    previous: &'a Emitter,
    this: &'a mut Emitter,
    start: Allocated,
}

impl NodeMap for Relocation<'_> {
    fn expression(&mut self, id: CheckedNodeId) -> CheckedNodeId {
        let pass = self.previous;
        let previous = &pass.expressions[id.0 as usize];
        let EmittedSlot::Expression(here) = self
            .this
            .allocate(previous.at.clone(), ArenaKind::Expression)
        else {
            return id;
        };
        let position = here.0 as usize;
        if position >= self.start.expressions && self.this.expressions[position].op.is_none() {
            let op = previous.op.as_ref().map(|op| op.map_operands(self));
            let applications = previous
                .applications
                .iter()
                .map(|candidates| {
                    candidates
                        .iter()
                        .map(|candidate| self.expression(*candidate))
                        .collect()
                })
                .collect();
            let node = &mut self.this.expressions[position];
            node.op = op;
            node.applications = applications;
            node.loop_targets.clone_from(&previous.loop_targets);
        }
        here
    }

    fn statement(&mut self, id: CheckedStmtId) -> CheckedStmtId {
        let pass = self.previous;
        let previous = &pass.statements[id.0 as usize];
        let EmittedSlot::Statement(here) = self
            .this
            .allocate(previous.at.clone(), ArenaKind::Statement)
        else {
            return id;
        };
        let position = here.0 as usize;
        if position >= self.start.statements && self.this.statements[position].op.is_none() {
            let op = previous.op.as_ref().map(|op| op.map_operands(self));
            let applications = previous
                .applications
                .iter()
                .map(|candidate| self.expression(*candidate))
                .collect();
            let node = &mut self.this.statements[position];
            node.op = op;
            node.applications = applications;
            node.loop_target.clone_from(&previous.loop_target);
        }
        here
    }
}

fn slot_position(position: usize) -> u32 {
    u32::try_from(position).expect("an emitter arena fits u32 slots")
}

#[cfg(test)]
mod tests {
    use super::Emitter;
    use mojito_ast::ast::{
        BindingMode, DestructureTarget, DestructureTargetKind, Expr, ExprKind, InfixOp, ParamArg,
        Stmt, StmtKind,
    };
    use mojito_checked::checked::{CheckedOp, CheckedParamArg};
    use mojito_checked::emitted::{EmittedNodes, EmittedSlot};
    use mojito_common::token::{SourceSpan, Span};
    use std::collections::HashMap;

    fn truth() -> Expr {
        Expr::new(ExprKind::Bool(true), Span::default())
    }

    fn statement() -> Stmt {
        expression_statement(truth())
    }

    fn expression_statement(value: Expr) -> Stmt {
        Stmt::new(StmtKind::Expr(value), Span::default())
    }

    fn value(statement: &Stmt) -> &Expr {
        match &statement.kind {
            StmtKind::Expr(value) => value,
            _ => unreachable!("the test statement is an expression statement"),
        }
    }

    /// Every record the check can read, pending ones included.
    fn records(emitter: &Emitter) -> HashMap<SourceSpan, bool> {
        emitter
            .iter()
            .map(|(at, facts)| (at.clone(), facts.truthiness_condition))
            .collect()
    }

    /// The op at `at`, its operands named by their occurrences.
    fn op_at(nodes: &EmittedNodes, at: &Expr) -> (String, Vec<SourceSpan>) {
        let node = nodes
            .expression(&at.source_span())
            .expect("the expression was emitted");
        let op = node.op.as_ref().expect("the node holds its op");
        let operands = match op {
            CheckedOp::Infix(_, left, right) => vec![*left, *right],
            _ => Vec::new(),
        };
        (
            format!("{:?}", std::mem::discriminant(op)),
            operands
                .into_iter()
                .map(|id| nodes.expressions[id.0 as usize].at.clone())
                .collect(),
        )
    }

    #[test]
    fn a_probe_rolls_back_writes_allocations_and_moved_records() {
        let (entered, synthesized) = (statement(), statement());
        let mut emitter = Emitter::default();
        emitter.enter_statement(&entered);
        emitter
            .record(value(&entered).source_span())
            .truthiness_condition = true;
        emitter
            .record(synthesized.source_span())
            .truthiness_condition = true;
        let mut before = Emitter::default();
        before.enter_statement(&entered);
        before
            .record(value(&entered).source_span())
            .truthiness_condition = true;
        before
            .record(synthesized.source_span())
            .truthiness_condition = true;

        let probe = emitter.open_probe();
        emitter
            .record(value(&entered).source_span())
            .truthiness_condition = false;
        emitter.enter_statement(&synthesized);
        emitter
            .record(value(&synthesized).source_span())
            .truthiness_condition = true;
        assert!(emitter.index.contains_key(&synthesized.source_span()));
        emitter.roll_back(probe);

        assert!(!emitter.index.contains_key(&synthesized.source_span()));
        assert!(
            !emitter
                .index
                .contains_key(&value(&synthesized).source_span())
        );
        assert_eq!(records(&emitter), records(&before));
    }

    #[test]
    fn nested_probes_roll_back_in_order() {
        let entered = statement();
        let at = value(&entered).source_span();
        let mut emitter = Emitter::default();
        emitter.enter_statement(&entered);
        let outer = emitter.open_probe();
        emitter.record(at.clone()).truthiness_condition = true;
        let inner = emitter.open_probe();
        emitter.record(at.clone()).truthiness_condition = false;
        emitter.remove(&at);
        emitter.roll_back(inner);
        assert_eq!(
            emitter.get(&at).map(|facts| facts.truthiness_condition),
            Some(true)
        );
        emitter.roll_back(outer);
        assert!(emitter.get(&at).is_none());
    }

    #[test]
    fn a_copied_window_gives_the_same_records() {
        let (entered, synthesized) = (statement(), statement());
        let mut previous = Emitter::default();
        previous.enter_statement(&entered);
        let from = previous.mark();
        previous
            .record(value(&entered).source_span())
            .truthiness_condition = true;
        previous.enter_statement(&synthesized);
        previous
            .record(synthesized.source_span())
            .truthiness_condition = true;
        let to = previous.mark();

        let mut fresh = Emitter::default();
        fresh.enter_statement(&entered);
        fresh.copy_window(&previous, &from, &to);
        assert!(fresh.index.contains_key(&synthesized.source_span()));
        assert_eq!(records(&fresh), records(&previous));
    }

    #[test]
    fn a_record_removed_after_the_window_is_not_copied() {
        let entered = statement();
        let at = value(&entered).source_span();
        let mut previous = Emitter::default();
        previous.enter_statement(&entered);
        let from = previous.mark();
        previous.record(at.clone()).truthiness_condition = true;
        let to = previous.mark();
        previous.remove(&at);

        let mut fresh = Emitter::default();
        fresh.enter_statement(&entered);
        fresh.copy_window(&previous, &from, &to);
        assert!(fresh.get(&at).is_none());
    }

    #[test]
    fn an_entered_expression_takes_its_pending_record() {
        let construction = truth();
        let mut emitter = Emitter::default();
        emitter
            .record(construction.source_span())
            .truthiness_condition = true;
        emitter.enter_expression(&construction);
        assert!(matches!(
            emitter.index.get(&construction.source_span()),
            Some(EmittedSlot::Expression(_))
        ));
        let nodes = emitter.into_nodes();
        assert!(
            nodes
                .expression(&construction.source_span())
                .and_then(|node| node.facts.as_deref())
                .is_some_and(|facts| facts.truthiness_condition)
        );
        assert!(matches!(
            nodes.expressions[0].op,
            Some(CheckedOp::Bool(true))
        ));
    }

    #[test]
    fn a_record_no_node_takes_is_discarded_at_the_handoff() {
        let (entered, evaluated) = (statement(), truth());
        let mut emitter = Emitter::default();
        emitter.enter_statement(&entered);
        emitter
            .record(value(&entered).source_span())
            .truthiness_condition = true;
        emitter.record(evaluated.source_span()).truthiness_condition = true;
        assert!(emitter.get(&evaluated.source_span()).is_some());
        let nodes = emitter.into_nodes();
        assert_eq!(
            nodes
                .records()
                .map(|(at, _)| at.clone())
                .collect::<Vec<_>>(),
            vec![value(&entered).source_span()]
        );
    }

    #[test]
    fn a_copied_window_relocates_its_ops_onto_this_emitters_nodes() {
        let (left, right) = (truth(), truth());
        let sum = Expr::new(
            ExprKind::Infix(
                InfixOp::Add,
                Box::new(left.clone()),
                Box::new(right.clone()),
            ),
            Span::default(),
        );
        let (entered, carried) = (statement(), expression_statement(sum.clone()));
        let mut previous = Emitter::default();
        previous.enter_statement(&entered);
        let from = previous.mark();
        previous.enter_statement(&carried);
        let to = previous.mark();

        let mut fresh = Emitter::default();
        fresh.enter_statement(&statement());
        fresh.enter_statement(&entered);
        fresh.copy_window(&previous, &from, &to);
        let previous = previous.into_nodes();
        let fresh = fresh.into_nodes();
        let at = |expression: &Expr| fresh.index.get(&expression.source_span()).copied();
        assert_ne!(at(&left), previous.index.get(&left.source_span()).copied());
        assert_eq!(op_at(&fresh, &sum), op_at(&previous, &sum));
        assert_eq!(
            op_at(&fresh, &sum).1,
            vec![left.source_span(), right.source_span()]
        );
    }

    #[test]
    fn an_entered_call_holds_one_parameter_per_bracket_argument() {
        let (parameter, argument) = (
            Expr::new(ExprKind::Identifier("T".to_string()), Span::default()),
            truth(),
        );
        let call = Expr::new(
            ExprKind::Call {
                name: "x".to_string(),
                param_args: vec![ParamArg::Value(parameter.clone())],
                args: vec![argument.clone()],
                kwargs: Vec::new(),
            },
            Span::default(),
        );
        let mut emitter = Emitter::default();
        emitter.enter_statement(&expression_statement(call.clone()));
        let nodes = emitter.into_nodes();
        let node = nodes
            .expression(&call.source_span())
            .expect("the call was emitted");
        let slot = |expression: &Expr| match nodes.index.get(&expression.source_span()) {
            Some(EmittedSlot::Expression(id)) => *id,
            _ => unreachable!("every operand was emitted"),
        };
        let Some(CheckedOp::Call {
            param_args, args, ..
        }) = &node.op
        else {
            unreachable!("a call's op is a call")
        };
        assert_eq!(param_args.len(), 1);
        assert!(param_args[0].declaration.is_none());
        assert!(matches!(
            param_args[0].arguments.as_slice(),
            [CheckedParamArg::Value(id)] if *id == slot(&parameter)
        ));
        assert_eq!(args, &vec![slot(&argument)]);
        assert_eq!(node.applications, vec![vec![slot(&parameter)]]);
    }

    fn for_statement(target: DestructureTarget) -> Stmt {
        Stmt::new(
            StmtKind::For {
                target,
                iter: Expr::new(ExprKind::Identifier("xs".to_string()), Span::default()),
                body: vec![statement()],
                orelse: None,
            },
            Span::default(),
        )
    }

    fn tuple_target() -> DestructureTarget {
        let leaf = |name: &str| {
            DestructureTarget::new(
                DestructureTargetKind::Declare {
                    name: name.to_string(),
                    mode: BindingMode::Immutable,
                },
                Span::default(),
            )
        };
        DestructureTarget::new(
            DestructureTargetKind::Tuple(vec![leaf("a"), leaf("b")]),
            Span::default(),
        )
    }

    #[test]
    fn an_entered_for_statement_holds_its_loop_targets_occurrence() {
        let target = tuple_target();
        let looped = for_statement(target.clone());
        let mut emitter = Emitter::default();
        emitter.enter_statement(&looped);
        let nodes = emitter.into_nodes();
        let node = nodes
            .statement(&looped.source_span())
            .expect("the loop was emitted");
        assert_eq!(node.loop_target, Some(target.source_span()));
    }

    #[test]
    fn a_copied_window_keeps_a_relocated_loops_target() {
        let target = tuple_target();
        let (entered, looped) = (statement(), for_statement(target.clone()));
        let mut previous = Emitter::default();
        previous.enter_statement(&entered);
        let from = previous.mark();
        previous.enter_statement(&looped);
        let to = previous.mark();

        let mut fresh = Emitter::default();
        fresh.enter_statement(&entered);
        fresh.copy_window(&previous, &from, &to);
        let nodes = fresh.into_nodes();
        let node = nodes
            .statement(&looped.source_span())
            .expect("the loop was copied");
        assert_eq!(node.loop_target, Some(target.source_span()));
        assert!(matches!(
            node.op,
            Some(mojito_checked::checked::CheckedStmtOp::For { .. })
        ));
    }

    #[test]
    fn a_probe_drops_the_nodes_and_ops_it_allocated() {
        let construction = truth();
        let mut emitter = Emitter::default();
        let probe = emitter.open_probe();
        emitter.enter_expression(&construction);
        assert!(emitter.index.contains_key(&construction.source_span()));
        emitter.roll_back(probe);
        assert!(!emitter.index.contains_key(&construction.source_span()));
        assert!(emitter.into_nodes().expressions.is_empty());
    }
}
