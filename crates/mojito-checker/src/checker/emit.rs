//! The check's emitted nodes: one per statement and expression occurrence,
//! allocated when the check enters the statement that holds it, each
//! carrying the facts the check decides there, as upstream's emitters set
//! each decided fact as an attribute on an op they already made.
//!
//! An index finds an occurrence's node. An occurrence no node holds (a
//! pattern, a destructuring target, an occurrence the check synthesizes, or
//! one written before its statement was entered) keeps a detached record,
//! which moves onto the node when one is allocated for it. A span is never
//! in both.
//!
//! The emitter logs and journals as the fact stores do (`fact_store`): a
//! body site's writes are carried by log range, and a probe's writes and
//! allocations are undone when it rolls back.

use mojito_ast::ast::{Expr, Stmt};
use mojito_ast::visit::{self, Visitor};
use mojito_checked::checked::NodeFacts;
use mojito_checked::fact_store::{FactMap, ProbeMark};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// The statement and expression nodes the check has allocated, the index
/// from occurrence to node, and the detached records.
#[derive(Debug, Default)]
pub struct Emitter {
    statements: Vec<EmittedNode>,
    expressions: Vec<EmittedNode>,
    index: HashMap<SourceSpan, EmittedSlot>,
    detached: FactMap<SourceSpan, NodeFacts>,
    /// Every node write, in order.
    log: Vec<EmittedSlot>,
    /// Each probed node write's slot and the record it displaced, oldest
    /// first.
    undo: Vec<(EmittedSlot, Option<Box<NodeFacts>>)>,
    probes: u32,
}

impl Emitter {
    /// The record at `at`: its node's when one holds the occurrence, its
    /// detached record otherwise.
    pub fn get(&self, at: &SourceSpan) -> Option<&NodeFacts> {
        match self.index.get(at) {
            Some(slot) => self.node(*slot).facts.as_deref(),
            None => self.detached.get(at),
        }
    }

    /// Every record: the nodes' in allocation order, then the detached.
    pub fn iter(&self) -> impl Iterator<Item = (&SourceSpan, &NodeFacts)> {
        self.statements
            .iter()
            .chain(&self.expressions)
            .filter_map(|node| Some((&node.at, node.facts.as_deref()?)))
            .chain(self.detached.iter())
    }

    /// Allocate `statement` and every statement and expression under it,
    /// unless the check entered it before.
    pub(super) fn enter_statement(&mut self, statement: &Stmt) {
        if !self.index.contains_key(&statement.source_span()) {
            visit::walk_stmt(&mut Allocation(self), statement);
        }
    }

    /// The record at `at`, created if absent: the write path.
    pub(super) fn record(&mut self, at: SourceSpan) -> &mut NodeFacts {
        match self.index.get(&at).copied() {
            Some(slot) => self.write(slot).get_or_insert_default(),
            None => self.detached.entry(at).or_default(),
        }
    }

    /// The record at `at` handed out for mutation; only an existing one
    /// counts as written.
    pub(super) fn get_mut(&mut self, at: &SourceSpan) -> Option<&mut NodeFacts> {
        match self.index.get(at).copied() {
            Some(slot) if self.node(slot).facts.is_some() => self.write(slot).as_deref_mut(),
            Some(_) => None,
            None => self.detached.get_mut(at),
        }
    }

    /// Take the record at `at` out; a removal is journaled, not logged.
    pub(super) fn remove(&mut self, at: &SourceSpan) -> Option<NodeFacts> {
        let Some(slot) = self.index.get(at).copied() else {
            return self.detached.remove(at);
        };
        let removed = self.node_mut(slot).facts.take();
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
                self.detached.insert(at, facts);
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
            detached: self.detached.mark(),
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
            .map(|slot| &self.node(*slot).at)
            .chain(self.detached.logged(from.detached..to.detached))
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
            detached: self.detached.open_probe(),
        }
    }

    /// Undo every write since `mark` was opened, newest first, then drop
    /// the nodes allocated since, with their index entries, and the slots
    /// logged since. A detached record a dropped node took is restored by
    /// the detached store's own roll-back.
    pub(super) fn roll_back(&mut self, mark: EmitProbe) {
        for (slot, previous) in self.undo.split_off(mark.undo).into_iter().rev() {
            self.node_mut(slot).facts = previous;
        }
        for node in self
            .statements
            .drain(mark.statements..)
            .chain(self.expressions.drain(mark.expressions..))
        {
            self.index.remove(&node.at);
        }
        self.log.truncate(mark.log);
        self.detached.roll_back(mark.detached);
        self.probes -= 1;
    }

    /// Copy the window `from..to` of `previous`, another pass over the same
    /// program: allocate every node it allocated there that this emitter
    /// lacks, then set the final record of every occurrence it wrote there.
    /// A record removed after the window is not copied.
    pub(super) fn copy_window(&mut self, previous: &Self, from: &EmitMark, to: &EmitMark) {
        for node in &previous.statements[from.statements..to.statements] {
            self.allocate(node.at.clone(), ArenaKind::Statement);
        }
        for node in &previous.expressions[from.expressions..to.expressions] {
            self.allocate(node.at.clone(), ArenaKind::Expression);
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
            .chain(&mut self.expressions)
            .filter_map(|node| node.facts.as_deref_mut())
            .for_each(&mut rewrite);
        self.detached.retain(|_, facts| {
            rewrite(facts);
            true
        });
    }

    /// The span-keyed handoff read out of the nodes and the detached
    /// records, less any record a cleared fact left empty.
    pub(super) fn into_records(self) -> HashMap<SourceSpan, NodeFacts> {
        let mut records: HashMap<SourceSpan, NodeFacts> = self
            .statements
            .into_iter()
            .chain(self.expressions)
            .filter_map(|node| Some((node.at, *node.facts?)))
            .filter(|(_, facts)| !facts.is_empty())
            .collect();
        let on_nodes = records.len();
        let mut detached = self.detached.into_inner();
        detached.retain(|_, facts| !facts.is_empty());
        timing::count("emit.node_records", on_nodes as u64);
        timing::count("emit.detached_records", detached.len() as u64);
        debug_assert!(
            detached.keys().all(|at| !records.contains_key(at)),
            "an occurrence held both a node record and a detached one"
        );
        records.extend(detached);
        records
    }

    fn node(&self, slot: EmittedSlot) -> &EmittedNode {
        match slot {
            EmittedSlot::Statement(at) => &self.statements[at as usize],
            EmittedSlot::Expression(at) => &self.expressions[at as usize],
        }
    }

    fn node_mut(&mut self, slot: EmittedSlot) -> &mut EmittedNode {
        match slot {
            EmittedSlot::Statement(at) => &mut self.statements[at as usize],
            EmittedSlot::Expression(at) => &mut self.expressions[at as usize],
        }
    }

    /// A node's record slot handed out for a write: logged, and journaled
    /// under a probe.
    fn write(&mut self, slot: EmittedSlot) -> &mut Option<Box<NodeFacts>> {
        self.log.push(slot);
        if self.probes > 0 {
            let previous = self.node(slot).facts.clone();
            self.undo.push((slot, previous));
        }
        &mut self.node_mut(slot).facts
    }

    /// Allocate a node for `at` unless one holds it, moving its detached
    /// record onto it.
    fn allocate(&mut self, at: SourceSpan, kind: ArenaKind) {
        let Entry::Vacant(vacant) = self.index.entry(at) else {
            return;
        };
        let facts = self.detached.remove(vacant.key()).map(Box::new);
        let at = vacant.key().clone();
        let arena = match kind {
            ArenaKind::Statement => &mut self.statements,
            ArenaKind::Expression => &mut self.expressions,
        };
        let position = u32::try_from(arena.len()).expect("an emitter arena fits u32 slots");
        arena.push(EmittedNode { at, facts });
        vacant.insert(match kind {
            ArenaKind::Statement => EmittedSlot::Statement(position),
            ArenaKind::Expression => EmittedSlot::Expression(position),
        });
    }
}

/// An emitter's arena and log positions at one moment, for a body site's
/// window.
#[derive(Debug, Clone, Copy)]
pub(super) struct EmitMark {
    statements: usize,
    expressions: usize,
    log: usize,
    detached: usize,
}

/// An emitter's positions when a probe opened.
#[derive(Debug, Clone, Copy)]
pub(super) struct EmitProbe {
    statements: usize,
    expressions: usize,
    log: usize,
    undo: usize,
    detached: ProbeMark,
}

/// One allocated node: its occurrence and the facts set on it.
#[derive(Debug)]
struct EmittedNode {
    at: SourceSpan,
    facts: Option<Box<NodeFacts>>,
}

/// A node's arena and position in it.
#[derive(Debug, Clone, Copy)]
enum EmittedSlot {
    Statement(u32),
    Expression(u32),
}

#[derive(Debug, Clone, Copy)]
enum ArenaKind {
    Statement,
    Expression,
}

/// The walk that allocates a statement's subtree.
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

#[cfg(test)]
mod tests {
    use super::Emitter;
    use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
    use mojito_common::token::{SourceSpan, Span};
    use std::collections::HashMap;

    fn statement() -> Stmt {
        Stmt::new(
            StmtKind::Expr(Expr::new(ExprKind::Bool(true), Span::default())),
            Span::default(),
        )
    }

    fn value(statement: &Stmt) -> &Expr {
        match &statement.kind {
            StmtKind::Expr(value) => value,
            _ => unreachable!("the test statement is an expression statement"),
        }
    }

    fn records(emitter: Emitter) -> HashMap<SourceSpan, bool> {
        emitter
            .into_records()
            .into_iter()
            .map(|(at, facts)| (at, facts.truthiness_condition))
            .collect()
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
        assert_eq!(records(emitter), records(before));
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
        assert_eq!(records(fresh), records(previous));
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
}
