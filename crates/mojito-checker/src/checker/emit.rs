//! The check's emitted nodes: one per statement and expression occurrence,
//! allocated when the check enters the statement that holds it, or the
//! construction it spells, or a match test or condition it plans, each
//! holding its op and carrying the facts the check decides there, as
//! upstream's emitters create each op when they emit it and set each
//! decided fact on it as an attribute. A fact that names another operation
//! (a spelled sugar's construction, a match plan's commands) holds that
//! operation's node, as an op names its operands' values.
//!
//! A node's op is set when the node is allocated, from its syntax, with its
//! operands the nodes allocated for its operands (`ops`), and never changes:
//! what the check decides later is a fact on the node. So a probe's
//! roll-back drops the nodes it allocated, ops and all, and restores only
//! records.
//!
//! An index finds an occurrence's node. An occurrence no node holds yet
//! keeps a pending record, which moves onto the node when one is allocated
//! for it; a span is never in both.
//!
//! Syntax the check types only to evaluate it (a compile-time operator's
//! dunder call, an origin alias's construction, a `rebind` target) is typed
//! inside an evaluation (`open_evaluation`/`close_evaluation`), as
//! upstream's parameter evaluator emits speculatively, reads the callee and
//! type off the ops it emitted, folds them into an attribute, and erases
//! them: closing it erases every record the evaluation created on syntax no
//! node holds. A record still pending when the check ends is on syntax a
//! site typed only to evaluate it outside an evaluation; the handoff
//! discards it, and `emit.discarded_records` counts it, so every fact
//! handed off is an attribute on an emitted op.
//!
//! The emitter logs and journals as the fact stores do (`fact_store`): a
//! body site's writes are carried by log range, and a probe's writes and
//! allocations are undone when it rolls back.
//!
//! The syntax of each construction the check spelled stays here, beside
//! its node, for the check's own reads of it, as upstream's parser keeps
//! its tree while the ops name values: it never crosses the handoff.

mod ops;

use mojito_ast::ast::{Expr, Stmt};
use mojito_ast::visit::{self, Visitor};
use mojito_checked::checked::{
    CheckedNodeId, CheckedStmtId, NodeFacts, NodeMap, SemanticAdjustment,
};
use mojito_checked::emitted::{EmittedExpr, EmittedNodes, EmittedSlot, EmittedStmt};
use mojito_checked::fact_store::{FactMap, ProbeMark};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};

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
    /// The syntax of each spelled construction, by its root's node.
    constructions: BTreeMap<CheckedNodeId, Expr>,
    /// The module's root statements' nodes, in source order.
    roots: Vec<CheckedStmtId>,
    /// Each default wrapper's body's nodes, by the wrapper's position.
    wrapper_bodies: BTreeMap<usize, Vec<CheckedStmtId>>,
    /// The evaluations open.
    evaluations: u32,
    /// Each pending record an open evaluation created, oldest first.
    evaluated: Vec<SourceSpan>,
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
    /// unless the check entered it before: the statement's node either way,
    /// none only when an expression holds its occurrence.
    pub(super) fn enter_statement(&mut self, statement: &Stmt) -> Option<CheckedStmtId> {
        let at = statement.source_span();
        if !self.index.contains_key(&at) {
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
        match self.index.get(&at)? {
            EmittedSlot::Statement(id) => Some(*id),
            EmittedSlot::Expression(_) => None,
        }
    }

    /// Emit the module's root statement `statement` as [`Self::enter_statement`]
    /// does, and hold its node as the module's next root.
    pub(super) fn enter_root(&mut self, statement: &Stmt) {
        let slot = self.enter_statement(statement);
        self.roots.push(slot.unwrap_or(EmittedNodes::MISSING));
    }

    /// Emit the body statements of default wrapper `wrapper`, the
    /// wrapper's position among the check's, and hold their nodes as its
    /// body.
    pub(super) fn enter_wrapper_body(&mut self, wrapper: usize, body: &[Stmt]) {
        let slots = body
            .iter()
            .map(|statement| {
                self.enter_statement(statement)
                    .unwrap_or(EmittedNodes::MISSING)
            })
            .collect();
        self.wrapper_bodies.insert(wrapper, slots);
    }

    /// Emit `expression` and every statement and expression under it that
    /// no node holds yet (a synthesized test or condition, a construction
    /// the check spelled): the expression's node, none only when a
    /// statement holds its occurrence.
    pub(super) fn enter_expression(&mut self, expression: &Expr) -> Option<CheckedNodeId> {
        let at = expression.source_span();
        if !self.index.contains_key(&at) {
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
        match self.index.get(&at)? {
            EmittedSlot::Expression(id) => Some(*id),
            EmittedSlot::Statement(_) => None,
        }
    }

    /// Emit the construction the check spelled for a sugar, keeping its
    /// syntax beside its node: the node the sugar's record names.
    pub(super) fn enter_construction(&mut self, construction: Expr) -> Option<CheckedNodeId> {
        let id = self.enter_expression(&construction)?;
        self.constructions.entry(id).or_insert(construction);
        Some(id)
    }

    /// The syntax of the construction the check spelled for the sugar at
    /// `sugar`, when it spelled one.
    pub(super) fn construction(&self, sugar: &SourceSpan) -> Option<&Expr> {
        match self.get(sugar)?.adjustment.as_deref()? {
            SemanticAdjustment::SpelledConstruction { construction } => {
                self.constructions.get(construction)
            }
            _ => None,
        }
    }

    /// The record at `at`, created if absent: the write path.
    pub(super) fn record(&mut self, at: SourceSpan) -> &mut NodeFacts {
        if let Some(slot) = self.index.get(&at).copied() {
            return self.write(slot).get_or_insert_default();
        }
        self.note_evaluated(&at);
        self.pending.entry(at).or_default()
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
        if let Some(slot) = self.index.get(&at).copied() {
            *self.write(slot) = Some(Box::new(facts));
        } else {
            self.note_evaluated(&at);
            self.pending.insert(at, facts);
        }
    }

    /// Whether a node holds the occurrence at `at`.
    pub(super) fn holds(&self, at: &SourceSpan) -> bool {
        self.index.contains_key(at)
    }

    /// Start an evaluation: a pending record created from here until
    /// `close_evaluation(mark)` is erased then, unless a node holds its
    /// occurrence by that time. Evaluations nest.
    pub(super) const fn open_evaluation(&mut self) -> EvaluationMark {
        self.evaluations += 1;
        EvaluationMark(self.evaluated.len())
    }

    /// End the evaluation `mark` opened, erasing every record it created
    /// on syntax no node holds; the erasure is journaled, so a probe
    /// around the evaluation restores it when it rolls back.
    pub(super) fn close_evaluation(&mut self, mark: EvaluationMark) {
        for at in self.evaluated.split_off(mark.0) {
            if !self.index.contains_key(&at) {
                self.pending.remove(&at);
            }
        }
        self.evaluations -= 1;
    }

    /// Erase the record of every occurrence in `expression` no node holds,
    /// and of each construction such an occurrence spelled: syntax that
    /// turned out, once typed, to have been typed only to evaluate it.
    pub(super) fn discard_unheld(&mut self, expression: &Expr) {
        let mut unheld = Unheld {
            emitter: self,
            constructions: Vec::new(),
        };
        visit::walk_expr(&mut unheld, expression);
        while let Some(construction) = unheld.constructions.pop() {
            visit::walk_expr(&mut unheld, &construction);
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
        self.constructions
            .split_off(&CheckedNodeId(slot_position(mark.expressions)));
        self.log.truncate(mark.log);
        self.pending.roll_back(mark.pending);
        self.probes -= 1;
    }

    /// Copy the window `from..to` of `previous`, another pass over the same
    /// program: allocate every node it allocated there that this emitter
    /// lacks, with its op relocated onto this emitter's nodes, then set the
    /// final record of every occurrence it wrote there, with the nodes it
    /// names relocated too. A record removed after the window is not
    /// copied.
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
        let records: Vec<(SourceSpan, NodeFacts)> = previous
            .written(from, to)
            .filter_map(|at| {
                let mut facts = previous.get(at)?.clone();
                facts.map_operands(&mut relocation);
                Some((at.clone(), facts))
            })
            .collect();
        for (at, facts) in records {
            self.insert(at, facts);
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
    /// syntax a site typed only to evaluate it outside an evaluation: the
    /// handoff discards it, as it does the constructions' syntax.
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
            roots: self.roots,
            wrapper_bodies: self.wrapper_bodies,
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

    /// Note a pending record an open evaluation is about to create at `at`.
    fn note_evaluated(&mut self, at: &SourceSpan) {
        if self.evaluations > 0 && !self.pending.contains_key(at) {
            self.evaluated.push(at.clone());
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

/// An emitter's evaluated-record position when an evaluation opened.
#[derive(Debug, Clone, Copy)]
pub(super) struct EvaluationMark(usize);

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

/// The walk that erases the pending record of every occurrence no node
/// holds, collecting the constructions those occurrences spelled.
struct Unheld<'a> {
    emitter: &'a mut Emitter,
    constructions: Vec<Expr>,
}

impl Visitor for Unheld<'_> {
    fn visit_expr(&mut self, expression: &Expr) {
        let at = expression.source_span();
        if let Some(construction) = self.emitter.construction(&at) {
            self.constructions.push(construction.clone());
        }
        if !self.emitter.holds(&at) {
            self.emitter.pending.remove(&at);
        }
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
        if let Some(construction) = pass.constructions.get(&id) {
            self.this
                .constructions
                .entry(here)
                .or_insert_with(|| construction.clone());
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
    use mojito_checked::checked::{
        CheckedCommand, CheckedNodeId, CheckedOp, CheckedParamArg, CheckedStmtId, MatchEmission,
        MatchPlan, SemanticAdjustment,
    };
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
        let slot = emitter.enter_expression(&construction);
        assert!(matches!(
            emitter.index.get(&construction.source_span()),
            Some(EmittedSlot::Expression(id)) if Some(*id) == slot
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

    /// Record at `sugar` that it spells the construction at `slot`.
    fn spell(emitter: &mut Emitter, sugar: &Expr, slot: CheckedNodeId) {
        emitter.record(sugar.source_span()).adjustment =
            Some(Box::new(SemanticAdjustment::SpelledConstruction {
                construction: slot,
            }));
    }

    fn expression_node_of(emitter: &Emitter, expression: &Expr) -> Option<CheckedNodeId> {
        match emitter.index.get(&expression.source_span())? {
            EmittedSlot::Expression(id) => Some(*id),
            EmittedSlot::Statement(_) => None,
        }
    }

    fn statement_node_of(emitter: &Emitter, statement: &Stmt) -> Option<CheckedStmtId> {
        match emitter.index.get(&statement.source_span())? {
            EmittedSlot::Statement(id) => Some(*id),
            EmittedSlot::Expression(_) => None,
        }
    }

    #[test]
    fn a_rolled_back_construction_leaves_no_syntax_for_its_sugar() {
        let (entered, sugar, construction) = (statement(), truth(), truth());
        let mut emitter = Emitter::default();
        emitter.enter_statement(&entered);
        let probe = emitter.open_probe();
        let slot = emitter
            .enter_construction(construction.clone())
            .expect("a construction is an expression");
        spell(&mut emitter, &sugar, slot);
        assert_eq!(
            emitter.construction(&sugar.source_span()),
            Some(&construction)
        );
        emitter.roll_back(probe);
        assert!(emitter.construction(&sugar.source_span()).is_none());
        assert!(emitter.constructions.is_empty());
    }

    #[test]
    fn a_copied_window_relocates_the_nodes_its_records_name() {
        let (entered, matched, bound) = (statement(), statement(), statement());
        let (sugar, construction, test) = (truth(), truth(), truth());
        let mut previous = Emitter::default();
        previous.enter_statement(&entered);
        let from = previous.mark();
        let slot = previous
            .enter_construction(construction.clone())
            .expect("a construction is an expression");
        spell(&mut previous, &sugar, slot);
        previous.enter_statement(&matched);
        let anchor = previous.enter_statement(&bound);
        let tested = previous
            .enter_expression(&test)
            .expect("a test is an expression");
        previous.record(matched.source_span()).match_plan = Some(MatchPlan {
            anchor,
            cases: vec![vec![CheckedCommand::Equal {
                test: tested,
                reads: Vec::new(),
            }]],
            reads: vec![Vec::new()],
            emission: MatchEmission::default(),
            conditions: Vec::new(),
            concluded: false,
            unreachable: Vec::new(),
        });
        let to = previous.mark();

        let mut fresh = Emitter::default();
        fresh.enter_statement(&statement());
        fresh.enter_statement(&entered);
        fresh.copy_window(&previous, &from, &to);
        let spelled = match fresh
            .get(&sugar.source_span())
            .and_then(|facts| facts.adjustment.as_deref())
        {
            Some(SemanticAdjustment::SpelledConstruction { construction }) => Some(*construction),
            _ => None,
        };
        assert_eq!(spelled, expression_node_of(&fresh, &construction));
        assert_ne!(spelled, Some(slot));
        assert_eq!(
            fresh.construction(&sugar.source_span()),
            Some(&construction)
        );
        let plan = fresh
            .get(&matched.source_span())
            .and_then(|facts| facts.match_plan.as_ref())
            .expect("the plan was copied");
        assert_eq!(plan.anchor, statement_node_of(&fresh, &bound));
        assert_ne!(plan.anchor, anchor);
        assert!(matches!(
            plan.cases[0].as_slice(),
            [CheckedCommand::Equal { test: relocated, .. }]
                if Some(*relocated) == expression_node_of(&fresh, &test) && *relocated != tested
        ));
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
    fn an_evaluation_erases_only_the_unheld_records_it_created() {
        let (entered, existing, evaluated) = (statement(), truth(), truth());
        let mut emitter = Emitter::default();
        emitter.enter_statement(&entered);
        emitter.record(existing.source_span()).truthiness_condition = true;
        let evaluation = emitter.open_evaluation();
        emitter
            .record(value(&entered).source_span())
            .truthiness_condition = true;
        emitter.record(existing.source_span()).truthiness_condition = false;
        emitter.record(evaluated.source_span()).truthiness_condition = true;
        emitter.close_evaluation(evaluation);
        assert!(emitter.get(&evaluated.source_span()).is_none());
        assert_eq!(
            emitter
                .get(&existing.source_span())
                .map(|facts| facts.truthiness_condition),
            Some(false)
        );
        assert_eq!(
            emitter
                .get(&value(&entered).source_span())
                .map(|facts| facts.truthiness_condition),
            Some(true)
        );
    }

    #[test]
    fn a_probe_around_an_evaluation_rolls_its_erasure_back() {
        let (existing, evaluated) = (truth(), truth());
        let mut emitter = Emitter::default();
        emitter.record(existing.source_span()).truthiness_condition = true;
        let before = records(&emitter);
        let probe = emitter.open_probe();
        let evaluation = emitter.open_evaluation();
        emitter.record(evaluated.source_span()).truthiness_condition = true;
        emitter.close_evaluation(evaluation);
        emitter.discard_unheld(&existing);
        assert!(records(&emitter).is_empty());
        emitter.roll_back(probe);
        assert_eq!(records(&emitter), before);
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
