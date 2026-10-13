//! Stage 4: lower the checked statement and expression nodes into a
//! **control-flow graph** (CFG) of basic blocks, built on `petgraph`.
//!
//! Expressions stay *nested* here (as checked nodes); Stage 5 flattens them to
//! A-Normal Form. Ownership and liveness analysis (Stages 6–7) run over the
//! flattened form, not this one.
//!
//! A [`Cfg`] is built per function body (or the top-level statement list) via
//! [`Cfg::build_checked_fn`]. Every block ends in exactly one [`Terminator`];
//! control flow (`if`/`while`/`for`/`break`/`continue`/`return`) becomes
//! blocks and edges, so later passes are pure graph traversal. A local name
//! resolves to its slot once, here, recorded per node in [`Cfg::slots`].

use mojito_checked::checked::{
    CheckedExpr, CheckedNodeId, CheckedOp, CheckedProgram, CheckedStmtId, CheckedStmtOp,
    CheckedTables, SemanticAdjustment,
};
use mojito_types::types::Ty;
use mojito_types::types::TyKind;
use petgraph::stable_graph::{NodeIndex, StableGraph};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One checked expression as the CFG holds it: its node, which carries its
/// op and attributes, with its operands' trees and the storage it denotes.
#[derive(Debug, Clone)]
pub struct HirExpr {
    pub node: CheckedNodeId,
    /// The checked operands' trees, in [`CheckedOp::operands`] order.
    /// Downstream passes use these identities rather than recovering facts by
    /// comparing source spans.
    pub children: Vec<Self>,
    /// Fully checked place shape when this expression denotes storage. This is
    /// independent of runtime `VarId` assignment and survives name shadowing.
    pub place: Option<HirPlace>,
}

#[derive(Debug, Clone)]
pub struct HirPlace {
    pub owner: mojito_types::origin::OwnerId,
    pub root_ty: Ty,
    pub projections: Vec<HirPlaceProjection>,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct HirPlaceProjection {
    pub kind: HirPlaceProjectionKind,
    pub base_ty: Ty,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum HirPlaceProjectionKind {
    Field(String),
    Index(CheckedNodeId),
    Variant(mojito_types::types::VariantIndex),
}

/// One opaque checked statement, lowered whole by Stage 5.
#[derive(Debug, Clone)]
pub struct HirStmt {
    pub node: CheckedStmtId,
    /// Checked roots directly owned by this opaque statement, in source
    /// order. Nested control-flow regions build their own CFGs.
    pub expressions: Vec<HirExpr>,
    /// The slot a `ref` declaration binds, under HIR's runtime name for it.
    pub declared: Option<VarId>,
}

pub type BlockId = NodeIndex;

pub type VarId = u32;

/// One straight-line instruction inside a basic block. Control flow lives in the
/// block's [`Terminator`], never here.
#[derive(Debug, Clone)]
pub enum HirInstr {
    /// A variable definition (`var x = e`, or a reassignment): a dataflow *def*.
    /// `binding_ty` is the resolved checked type for an initializing binding.
    /// Reassignments use `None` and retain the existing slot type.
    Bind {
        dest: VarId,
        expr: HirExpr,
        binding_ty: Option<Ty>,
        /// Stable checker identity introduced by this binding, when this is a
        /// declaration rather than a write to an existing slot.
        binding: Option<mojito_types::origin::OwnerId>,
    },
    /// Bind a concrete borrowed collection place without invoking its value-copy
    /// lifecycle. `origin` is the checker-proven interior generation retained by
    /// the iterator. MIR lowers this to an explicit place load plus
    /// `EstablishLoans`; it is not a VM type-name special case.
    BorrowIter {
        dest: VarId,
        expr: HirExpr,
        origin: mojito_types::origin::OriginPlace,
        /// Whether the loop binds the yielded references mutably (`for ref x`
        /// over a mutable source): the retained loan is then exclusive against
        /// a whole-place loan of the same source, a value loop's is shared.
        mutable: bool,
    },
    /// A bare expression evaluated for its effect/value (`f(x)`).
    Eval(HirExpr),
    /// Any other straight-line statement (assignment, member-write, declaration,
    /// …), kept whole for Stage 4 and refined when Stage 5 flattens to MIR.
    Stmt(HirStmt),
    /// An ASAP destructor, spliced in by Stage 7 liveness (never by the Stage 4
    /// lowerer — present so later stages share the type).
    Drop(VarId),
    /// A liveness anchor: `var` is read for analysis but not at runtime. Emitted at
    /// a loop exit to keep a split iteration source live through the loop (its only
    /// other use is `GetIter` before the loop), so ASAP-drop does not destroy it
    /// while a borrowing iterator still refers to its storage.
    KeepAlive(VarId),
    /// Iterator protocol: normalize the iterable in `source` to an *iterator* in
    /// `dest` — for a nominal iterable, `dest = source.__iter__()`. When
    /// `source == dest` the iterable is normalized in place (a concrete borrowed
    /// place, or compiler-private storage that iterates in place, a no-op). When
    /// they differ, `source` retains the live iterable (kept in its own slot and
    /// dropped after the loop) so a borrowing iterator does not clobber its only
    /// owner. Emitted once before the loop header.
    GetIter {
        source: VarId,
        dest: VarId,
        protocol: mojito_checked::checked::IterationProtocol,
    },
    /// Iterator protocol over compiler-private iterator storage (a runtime pack
    /// or a compile-time list): `dest = whether `iter` yields another element`
    /// (a `Bool`), a pure read of the iterator's state. `iter`/`dest` are
    /// variable slots so both IRs and the backend address them uniformly.
    HasNext { iter: VarId, dest: VarId },
    /// Iterator protocol over compiler-private iterator storage: `raw =
    /// iter.next()` — retain the element in a compiler-owned slot and advance
    /// `iter` in place. The successful body adapts `raw` to the source loop
    /// target with [`HirInstr::BindIteration`].
    Next {
        iter: VarId,
        raw: VarId,
        /// Exact checker-resolved element type.
        element_ty: Ty,
    },
    /// Nominal iterator protocol: call a typed-raising `__next__` exactly once,
    /// storing its exact result in `raw` and whether it yielded in `yielded`.
    /// Raising the checked exhaustion type sets `yielded = False`; every other
    /// runtime error propagates normally.
    TryNext {
        iter: VarId,
        raw: VarId,
        yielded: VarId,
        /// Exact checker-selected reference/value result and effect contract.
        call: mojito_checked::checked::CheckedIteratorCall,
        exhaustion: Ty,
        element_ty: Ty,
    },
    /// Adapt one raw iterator result into the user-visible target. This executes
    /// only in the yielded body, so reference copies and value moves never run on
    /// the `StopIteration` edge. `iter` is the iterator-object slot whose source
    /// loans a borrowed reference binding re-establishes on itself.
    BindIteration {
        raw: VarId,
        dest: VarId,
        iter: VarId,
        plan: mojito_checked::checked::CheckedIterationBinding,
        binding: mojito_types::origin::OwnerId,
    },
    /// A `try` statement whose sub-regions are lowered in Stage 5 as mini-CFGs.
    /// `loop_targets` snapshots the enclosing **function-level** loop stack
    /// (`(continue → header, break → exit, return cleanup)`, innermost-last)
    /// at this point, so control leaving a `try` retains both the outer target and
    /// the iterator storage that a return must destroy after `finally`. Only
    /// produced when every enclosing loop is function-level; otherwise the `try`
    /// stays a `Stmt`.
    Try {
        stmt: HirStmt,
        loop_targets: Vec<(BlockId, BlockId, Vec<VarId>)>,
    },
}

/// How a block hands control to its successor(s). Exactly one per block.
#[derive(Debug, Clone)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        cond: HirExpr,
        then_b: BlockId,
        else_b: BlockId,
    },
    /// Branch on a `Bool` slot HIR itself wrote (a `for` header's
    /// has-next flag), which no checked node computes.
    BranchOnSlot {
        cond: VarId,
        then_b: BlockId,
        else_b: BlockId,
    },
    /// A `comptime if` arm boundary: the condition is a compile-time
    /// expression MIR lowers as a parameter expression, not a value.
    ComptimeBranch {
        cond: HirExpr,
        then_b: BlockId,
        else_b: BlockId,
    },
    /// A `comptime for` header: the iterable is a compile-time range MIR
    /// lowers as parameter expressions; the loop variable `var` is the
    /// checked binding `binding`, interned at `index` here and bound to its
    /// runtime slot by MIR as a `for` variable is; `body` is the loop body
    /// (its back edge jumps here) and `exit` the block after the loop. The
    /// elaborator unrolls it.
    ComptimeLoop {
        iter: HirExpr,
        var: String,
        binding: Option<mojito_types::origin::OwnerId>,
        index: VarId,
        body: BlockId,
        exit: BlockId,
    },
    Return(Option<HirExpr>),
    /// A return nested in one or more iterator-driven loops. The value is
    /// evaluated before `cleanup` is destroyed (innermost iterator first), so
    /// moving or copying the current element into the result remains valid while
    /// residual owned iterator storage cannot leak past the function boundary.
    ReturnWithCleanup {
        value: Option<HirExpr>,
        cleanup: Vec<VarId>,
    },
    /// The normal fall-through end of a **seeded region** (a `try` sub-body). Unlike
    /// `Return(None)` (an explicit bare `return`), this means "the region completed
    /// normally" — the VM continues to the `else`/`finally`, not out of the frame.
    /// Only appears in region CFGs (`build_seeded_checked_region`), never a
    /// function body.
    FallOff,
    /// A `break`/`continue` inside a seeded region that targets a loop in the
    /// **enclosing function** CFG: `target` is that loop's exit (`break`) or header
    /// (`continue`) block, an id in the enclosing CFG, *not* this region's. No graph
    /// edge is added (the target isn't a node here); the VM propagates it out as a
    /// `Flow::Jump`. Only appears in region CFGs seeded with external loops.
    EscapeJump(BlockId),
}

#[derive(Debug, Default)]
pub struct BasicBlock {
    pub instrs: Vec<HirInstr>,
    pub term: Option<Terminator>,
}

/// A control-flow graph for one function body / statement list.
#[derive(Debug)]
pub struct Cfg {
    pub g: StableGraph<BasicBlock, ()>,
    pub entry: BlockId,
    /// The variable interner table: `vars[id as usize]` is the name that `id` was
    /// assigned. Exposed so the Stage 5 MIR flattener can seed its own interner and
    /// keep `VarId`s consistent across the two IRs.
    pub vars: Vec<String>,
    /// The number of leading `vars` that are the function's **parameters**, in
    /// declaration order (so `vars[0..n_params]` are the params). The call ABI: a
    /// caller binds its argument values to these var slots. `0` for the top-level
    /// block and for bodies built without a parameter list.
    pub n_params: usize,
    /// The program's typed semantic nodes and declarations, shared (not
    /// copied) with every other function's lowering. Kept by stable node id;
    /// a CFG is handed its body's statement ids and finds no node by location.
    pub checked: Arc<CheckedTables>,
    /// The slot each local-name node (an `Identifier`, a walrus `Named`) of
    /// this CFG's expressions reads, as resolved in its lexical scope: the
    /// value an upstream operand names.
    pub slots: HashMap<CheckedNodeId, VarId>,
    /// The statement nodes of the body this CFG lowers.
    pub body: Vec<CheckedStmtId>,
}

impl Cfg {
    /// Lower a function body whose `params` (in declaration order) seed the
    /// variable interner **first**, so `VarId`s `0..params.len()` are the
    /// parameters — the call ABI the MIR/VM rely on to bind arguments. The final
    /// fall-through block gets an implicit `return None`.
    pub fn build_checked_fn(
        checked: &CheckedProgram,
        params: &[String],
        body: &[CheckedStmtId],
    ) -> Self {
        Self::build_fn_with_context(
            params,
            HashSet::new(),
            body,
            None,
            Arc::clone(checked.tables()),
        )
    }

    /// [`Cfg::build_checked_fn`] for a nested `def` whose immutable captures
    /// a body assignment shadows rather than writes.
    pub fn build_checked_fn_with_captures(
        checked: &CheckedProgram,
        params: &[String],
        shadow_captures: HashSet<String>,
        body: &[CheckedStmtId],
    ) -> Self {
        Self::build_fn_with_context(
            params,
            shadow_captures,
            body,
            None,
            Arc::clone(checked.tables()),
        )
    }

    /// [`Cfg::build_checked_fn`] for an expression thunk: the `prologue`
    /// statements, then `return value`.
    pub fn build_checked_thunk(
        checked: &CheckedProgram,
        params: &[String],
        prologue: &[CheckedStmtId],
        value: CheckedNodeId,
    ) -> Self {
        Self::build_fn_with_context(
            params,
            HashSet::new(),
            prologue,
            Some(value),
            Arc::clone(checked.tables()),
        )
    }

    /// Lower a checked `try` region, the statement nodes `body`, into a CFG
    /// whose variable interner **starts** seeded with `seed_vars` (so the
    /// region shares the enclosing frame's `VarId`s — a name already interned
    /// keeps its id, new names append). The region's loop stack starts with the enclosing function's
    /// loops (`external_loops`, innermost-last, as `(header, exit, cleanup)`
    /// in the *enclosing* CFG): a `break`/`continue` that targets one of them
    /// lowers to a [`Terminator::EscapeJump`]. `n_params` is 0.
    pub fn build_seeded_checked_region(
        seed_vars: Vec<String>,
        body: &[CheckedStmtId],
        external_loops: &[(BlockId, BlockId, Vec<VarId>)],
        checked: Arc<CheckedTables>,
    ) -> Self {
        let mut g = StableGraph::new();
        let entry = g.add_node(BasicBlock::default());
        let loops = external_loops
            .iter()
            .map(|(header, exit, cleanup)| LoopFrame {
                header: *header,
                exit: *exit,
                escape: true,
                cleanup: cleanup.clone(),
            })
            .collect();
        let mut lower = Lower {
            g,
            cur: entry,
            loops,
            scopes: vec![seed_vars.iter().map(|n| (n.clone(), n.clone())).collect()],
            captures: HashSet::new(),
            vars: seed_vars,
            is_function: false,
            checked,
            slots: HashMap::new(),
        };
        for id in body {
            lower.stmt(*id);
        }
        lower.seal(Terminator::FallOff); // region completed normally (not a return)
        lower.finish(entry, 0, body.to_vec())
    }

    // --- Read-only accessors (so tests/analysis need not import `petgraph`) ---

    pub fn node_count(&self) -> usize {
        self.g.node_count()
    }
    pub fn edge_count(&self) -> usize {
        self.g.edge_count()
    }
    pub fn block(&self, b: BlockId) -> &BasicBlock {
        &self.g[b]
    }
    pub fn term(&self, b: BlockId) -> Option<&Terminator> {
        self.g[b].term.as_ref()
    }
    /// The successor blocks of `b`, by graph edge (order unspecified).
    pub fn successors(&self, b: BlockId) -> Vec<BlockId> {
        self.g.neighbors(b).collect()
    }
    /// Whether an edge `from → to` exists.
    pub fn has_edge(&self, from: BlockId, to: BlockId) -> bool {
        self.g.neighbors(from).any(|n| n == to)
    }

    fn build_fn_with_context(
        params: &[String],
        shadow_captures: HashSet<String>,
        body: &[CheckedStmtId],
        returned: Option<CheckedNodeId>,
        checked: Arc<CheckedTables>,
    ) -> Self {
        let mut g = StableGraph::new();
        let entry = g.add_node(BasicBlock::default());
        let mut vars = params.to_vec();
        let mut root_scope: HashMap<String, String> =
            params.iter().map(|n| (n.clone(), n.clone())).collect();
        let mut lower = Lower {
            g,
            cur: entry,
            loops: Vec::new(),
            vars: Vec::new(),
            scopes: Vec::new(),
            captures: shadow_captures,
            is_function: true,
            checked,
            slots: HashMap::new(),
        };
        let explicit = explicit_local_names(&lower.checked, body);
        let mut implicit = HashSet::new();
        collect_function_implicit_names(&lower.checked, body, &explicit, &mut implicit);
        let mut implicit: Vec<_> = implicit.into_iter().collect();
        implicit.sort();
        for name in implicit {
            if !root_scope.contains_key(&name) {
                vars.push(name.clone());
                root_scope.insert(name.clone(), name);
            }
        }
        lower.vars = vars;
        lower.scopes = vec![root_scope];
        for id in body {
            lower.stmt(*id);
        }
        if let Some(value) = returned {
            lower.ret(Some(value));
        }
        lower.seal(Terminator::Return(None)); // implicit `return None` off the end
        lower.finish(entry, params.len(), body.to_vec())
    }
}

fn collect_function_implicit_names(
    checked: &CheckedTables,
    body: &[CheckedStmtId],
    explicit: &HashSet<String>,
    names: &mut HashSet<String>,
) {
    for statement in body.iter().filter_map(|id| checked.statement(*id)) {
        let bodies: Vec<&[CheckedStmtId]> = match &statement.op {
            CheckedStmtOp::Assign { name, .. } => {
                if !explicit.contains(name) {
                    names.insert(name.clone());
                }
                Vec::new()
            }
            CheckedStmtOp::If { branches, orelse }
            | CheckedStmtOp::ComptimeIf { branches, orelse } => branches
                .iter()
                .map(|(_, body)| body.as_slice())
                .chain(orelse.as_deref())
                .collect(),
            CheckedStmtOp::While { body, orelse, .. } | CheckedStmtOp::For { body, orelse, .. } => {
                std::iter::once(body.as_slice())
                    .chain(orelse.as_deref())
                    .collect()
            }
            CheckedStmtOp::Try {
                body,
                except,
                orelse,
                finalbody,
            } => std::iter::once(body.as_slice())
                .chain(except.as_ref().map(|(_, body)| body.as_slice()))
                .chain(orelse.as_deref())
                .chain(finalbody.as_deref())
                .collect(),
            CheckedStmtOp::ComptimeFor { body, .. }
            | CheckedStmtOp::With { body, .. }
            | CheckedStmtOp::Scope(body) => vec![body.as_slice()],
            _ => Vec::new(),
        };
        for body in bodies {
            collect_function_implicit_names(checked, body, explicit, names);
        }
    }
}

/// Lowering cursor: appends to the "current" block, splitting on control flow.
/// Invariant: `cur` is the current insertion point; a block gets its terminator
/// (and out-edges) exactly once, via [`Lower::seal`].
struct Lower {
    g: StableGraph<BasicBlock, ()>,
    cur: BlockId,
    /// Innermost-last stack of enclosing loops' targets.
    loops: Vec<LoopFrame>,
    /// Interner: a variable name's first appearance assigns its `VarId`.
    vars: Vec<String>,
    scopes: Vec<HashMap<String, String>>,
    captures: HashSet<String>,
    /// Whether this is a function-body CFG (vs. a seeded `try` region). A
    /// function's own loops are function-level, so a `try` inside one can escape to
    /// them; a region's own loops are region-local and cannot be escape targets.
    is_function: bool,
    /// The program's checked tables, shared with every other lowering.
    checked: Arc<CheckedTables>,
    /// The slot each local-name node resolved to.
    slots: HashMap<CheckedNodeId, VarId>,
}

impl Lower {
    fn checked_place(&self, node: &CheckedExpr) -> Option<HirPlace> {
        match &node.op {
            CheckedOp::Identifier(_) => Some(HirPlace {
                owner: node.binding?,
                root_ty: node.place_ty.clone()?,
                projections: Vec::new(),
                ty: node.place_ty.clone()?,
            }),
            CheckedOp::Member { object, field } => {
                let base = self.checked.expression(*object)?;
                let mut place = self.checked_place(base)?;
                let ty = node.place_ty.clone()?;
                let base_ty = place.ty.clone();
                place.projections.push(HirPlaceProjection {
                    kind: HirPlaceProjectionKind::Field(field.clone()),
                    base_ty,
                    ty: ty.clone(),
                });
                place.ty = ty;
                Some(place)
            }
            CheckedOp::Index { object, index } => {
                let base = self.checked.expression(*object)?;
                let mut place = self.checked_place(base)?;
                let ty = node.place_ty.clone()?;
                let base_ty = place.ty.clone();
                // A subscript-shaped Variant projection (`storage[T]` on a
                // field, `v[String]`) is the tagged payload place.
                let variant_index =
                    node.adjustments
                        .iter()
                        .find_map(|adjustment| match adjustment {
                            SemanticAdjustment::VariantProject { index, .. } => Some(index.clone()),
                            _ => None,
                        });
                let kind = match variant_index {
                    Some(index) => HirPlaceProjectionKind::Variant(index),
                    None => HirPlaceProjectionKind::Index(*index),
                };
                place.projections.push(HirPlaceProjection {
                    kind,
                    base_ty,
                    ty: ty.clone(),
                });
                place.ty = ty;
                Some(place)
            }
            CheckedOp::TypeApply { .. } => {
                let (alternatives, index) =
                    node.adjustments
                        .iter()
                        .find_map(|adjustment| match adjustment {
                            SemanticAdjustment::VariantProject {
                                alternatives,
                                index,
                            } => Some((alternatives, index.clone())),
                            _ => None,
                        })?;
                let ty = node.place_ty.clone()?;
                Some(HirPlace {
                    owner: node.binding?,
                    root_ty: TyKind::Variant(alternatives.clone()).into(),
                    projections: vec![HirPlaceProjection {
                        kind: HirPlaceProjectionKind::Variant(index),
                        base_ty: TyKind::Variant(alternatives.clone()).into(),
                        ty: ty.clone(),
                    }],
                    ty,
                })
            }
            _ => None,
        }
    }

    fn new_block(&mut self) -> BlockId {
        self.g.add_node(BasicBlock::default())
    }

    fn push(&mut self, i: HirInstr) {
        // Dead code after a terminator (e.g. a statement following `return`) is
        // dropped rather than appended to an already-sealed block.
        if self.g[self.cur].term.is_none() {
            self.g[self.cur].instrs.push(i);
        }
    }

    /// Give `cur` its terminator and wire the implied out-edges — but only if it
    /// isn't already sealed (a `break`/`continue`/`return` inside a body may have
    /// sealed it first, in which case this is a no-op).
    fn seal(&mut self, t: Terminator) {
        if self.g[self.cur].term.is_some() {
            return;
        }
        match &t {
            Terminator::Jump(to) => {
                self.g.add_edge(self.cur, *to, ());
            }
            Terminator::Branch { then_b, else_b, .. }
            | Terminator::BranchOnSlot { then_b, else_b, .. }
            | Terminator::ComptimeBranch { then_b, else_b, .. } => {
                self.g.add_edge(self.cur, *then_b, ());
                self.g.add_edge(self.cur, *else_b, ());
            }
            Terminator::ComptimeLoop { body, exit, .. } => {
                self.g.add_edge(self.cur, *body, ());
                self.g.add_edge(self.cur, *exit, ());
            }
            // No in-graph edge: `Return`/`FallOff` leave the CFG; `EscapeJump`
            // targets a block in the *enclosing* CFG (not a node here).
            Terminator::Return(_)
            | Terminator::ReturnWithCleanup { .. }
            | Terminator::FallOff
            | Terminator::EscapeJump(_) => {}
        }
        self.g[self.cur].term = Some(t);
    }

    /// Intern a variable name to a stable `VarId`.
    fn var(&mut self, name: &str) -> VarId {
        if let Some(i) = self.vars.iter().position(|n| n == name) {
            i as VarId
        } else {
            self.vars.push(name.to_string());
            (self.vars.len() - 1) as VarId
        }
    }

    fn finish(self, entry: BlockId, n_params: usize, body: Vec<CheckedStmtId>) -> Cfg {
        Cfg {
            g: self.g,
            entry,
            vars: self.vars,
            n_params,
            checked: self.checked,
            slots: self.slots,
            body,
        }
    }

    fn resolved(&self, name: &str) -> String {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    fn declare_var(&mut self, name: &str) -> VarId {
        // A source name can denote several checker bindings even when their
        // lexical scopes are siblings rather than nested. Compile-time
        // unrolling makes that distinction observable when the cloned
        // declarations infer different types, so never recycle a slot that was
        // already assigned to an earlier declaration.
        let shadows_active_binding = self
            .scopes
            .iter()
            .rev()
            .skip(1)
            .any(|scope| scope.contains_key(name));
        let runtime =
            if shadows_active_binding || self.vars.iter().any(|candidate| candidate == name) {
                format!("{name}$shadow{}", self.vars.len())
            } else {
                name.to_string()
            };
        if self.scopes.is_empty() {
            self.scopes.push(HashMap::new());
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), runtime.clone());
        }
        self.var(&runtime)
    }

    /// Expression `id` evaluated in this frame: its names resolved to the
    /// slots in scope here, and its tree.
    fn expr(&mut self, id: CheckedNodeId) -> HirExpr {
        self.resolve_names(id);
        self.tree(id)
    }

    /// Record the slot each local name under `id` reads in this frame's
    /// scopes. Compile-time parameter arguments, t-string parts, and a
    /// lambda's `def` name no slot of this frame, so the walk skips them,
    /// as a `$contextual` leading-dot root, which names its base type.
    fn resolve_names(&mut self, id: CheckedNodeId) {
        let checked = Arc::clone(&self.checked);
        let Some(node) = checked.expression(id) else {
            return;
        };
        let name = match &node.op {
            CheckedOp::Identifier(name) | CheckedOp::Named { name, .. }
                if name != mojito_ast::ast::CONTEXTUAL_SENTINEL =>
            {
                Some(name)
            }
            _ => None,
        };
        if let Some(name) = name
            && self.scopes.iter().any(|scope| scope.contains_key(name))
        {
            let slot = self.var(&self.resolved(name));
            let recorded = *self.slots.entry(id).or_insert(slot);
            debug_assert_eq!(recorded, slot, "a node resolves to one slot per frame");
        }
        for operand in named_operands(&node.op) {
            self.resolve_names(operand);
        }
    }

    /// Expression `id`'s tree: the node, its operands' trees, and the place
    /// it denotes.
    fn tree(&self, id: CheckedNodeId) -> HirExpr {
        let node = self
            .checked
            .expression(id)
            .expect("a checked operand names a node of its tables");
        HirExpr {
            node: id,
            children: node
                .op
                .operands()
                .into_iter()
                .map(|operand| self.tree(operand))
                .collect(),
            place: self.checked_place(node),
        }
    }

    /// Opaque statement `id`, with its expression roots' trees. MIR lowers
    /// the statement whole, resolving its names by binding.
    fn statement(&self, id: CheckedStmtId) -> HirStmt {
        let expressions = self
            .checked
            .statement(id)
            .map(|node| checked_statement_roots(&node.op))
            .unwrap_or_default()
            .into_iter()
            .map(|expression| self.tree(expression))
            .collect();
        HirStmt {
            node: id,
            expressions,
            declared: None,
        }
    }

    fn scoped_block(&mut self, body: &[CheckedStmtId]) {
        self.scopes.push(HashMap::new());
        self.block(body);
        self.scopes.pop();
    }

    fn block(&mut self, body: &[CheckedStmtId]) {
        for s in body {
            self.stmt(*s);
        }
    }

    fn ret(&mut self, value: Option<CheckedNodeId>) {
        let value = value.map(|value| self.expr(value));
        let cleanup = self
            .loops
            .iter()
            .rev()
            .flat_map(|frame| frame.cleanup.iter().copied())
            .collect::<Vec<_>>();
        self.seal(if cleanup.is_empty() {
            Terminator::Return(value)
        } else {
            Terminator::ReturnWithCleanup { value, cleanup }
        });
    }

    #[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
    fn stmt(&mut self, id: CheckedStmtId) {
        let checked = Arc::clone(&self.checked);
        let s = checked
            .statement(id)
            .expect("a checked body names a statement node of its tables");
        match &s.op {
            // A `comptime if` is the same diamond with a compile-time
            // condition: the elaborator keeps one arm, after ownership has
            // decided both.
            CheckedStmtOp::If { branches, orelse }
            | CheckedStmtOp::ComptimeIf { branches, orelse } => {
                let comptime = matches!(s.op, CheckedStmtOp::ComptimeIf { .. });
                let join = self.new_block();
                for (cond, body) in branches {
                    let then_b = self.new_block();
                    let else_b = self.new_block();
                    let cond = self.expr(*cond);
                    self.seal(if comptime {
                        Terminator::ComptimeBranch {
                            cond,
                            then_b,
                            else_b,
                        }
                    } else {
                        Terminator::Branch {
                            cond,
                            then_b,
                            else_b,
                        }
                    });
                    self.cur = then_b;
                    self.scoped_block(body);
                    self.seal(Terminator::Jump(join));
                    self.cur = else_b; // the next elif/else lowers into this block
                }
                if let Some(body) = orelse {
                    self.scoped_block(body);
                }
                self.seal(Terminator::Jump(join));
                self.cur = join;
            }

            CheckedStmtOp::Scope(body) => self.scoped_block(body),

            // A `comptime for` is a loop whose header advances a compile-time
            // index: the body is analysed once as a loop body with the trip
            // count unknown, `break` and `continue` are the loop's, and the
            // elaborator unrolls it.
            CheckedStmtOp::ComptimeFor { var, iter, body } => {
                let iter = self.expr(*iter);
                let header = self.new_block();
                let body_b = self.new_block();
                let exit = self.new_block();
                self.seal(Terminator::Jump(header));
                self.cur = header;
                self.scopes.push(HashMap::new());
                let index = self.declare_var(var);
                self.seal(Terminator::ComptimeLoop {
                    iter,
                    var: var.clone(),
                    binding: s.binding,
                    index,
                    body: body_b,
                    exit,
                });
                self.cur = body_b;
                self.loops.push(LoopFrame {
                    header,
                    exit,
                    escape: false,
                    cleanup: Vec::new(),
                });
                self.block(body);
                self.loops.pop();
                self.scopes.pop();
                self.seal(Terminator::Jump(header));
                self.cur = exit;
            }

            CheckedStmtOp::While { cond, body, orelse } => {
                let header = self.new_block();
                let body_b = self.new_block();
                let exit = self.new_block();
                let normal_exit = orelse.as_ref().map_or(exit, |_| self.new_block());
                self.seal(Terminator::Jump(header));
                self.cur = header;
                let cond = self.expr(*cond);
                self.seal(Terminator::Branch {
                    cond,
                    then_b: body_b,
                    else_b: normal_exit,
                });
                self.cur = body_b;
                self.loops.push(LoopFrame {
                    header,
                    exit,
                    escape: false,
                    cleanup: Vec::new(),
                });
                self.scoped_block(body);
                self.loops.pop();
                self.seal(Terminator::Jump(header)); // back-edge (unless body sealed via break/return)
                if let Some(body) = orelse {
                    self.cur = normal_exit;
                    self.scoped_block(body);
                    self.seal(Terminator::Jump(exit));
                }
                self.cur = exit;
            }

            // `for x in it: body` — a while-shaped CFG driving the iterator
            // protocol: a pre-header evaluates the iterable once into an iterator
            // slot; the header computes `has_next` and branches on it; the body
            // binds the loop variable via `next` (which advances the iterator).
            CheckedStmtOp::For {
                var,
                binding: binding_mode,
                iter,
                body,
                orelse,
            } => {
                let checked_iter = self.expr(*iter);
                let protocol = checked
                    .expression(*iter)
                    .into_iter()
                    .flat_map(|node| &node.adjustments)
                    .find_map(|adjustment| match adjustment {
                        SemanticAdjustment::Iterate(protocol) => Some(protocol.clone()),
                        _ => None,
                    })
                    .expect("checked for-loop iterable has an iteration protocol");
                let binding_plan = *protocol
                    .binding
                    .clone()
                    .expect("checked for-loop protocol has a binding plan");
                debug_assert_eq!(binding_plan.mode, *binding_mode);
                // Evaluate the iterable once into a fresh iterator variable. `$`
                // names can't be produced by the parser, so they never collide with
                // user variables; a monotone `vars.len()` suffix keeps them unique.
                let it_name = format!("$iter{}", self.vars.len());
                let it_var = self.var(&it_name);
                // Every borrowed source is retained in its own slot — `it_var`
                // holds either a `BorrowIter` reference handle to the named
                // source or the `Bind`-bound owned temporary — and the iterator
                // object lives in a distinct slot: `GetIter` normalization must
                // not clobber the source the iterator still refers to. Only an
                // empty-`prepare` iterable normalizes in place, and owned
                // iteration (`__iter__(var self)`) consumes the source into the
                // iterator, so both keep the single slot (retaining and dropping
                // a consumed source again would double-free).
                // A `BorrowIter`'d source (borrowed origin present) is a loan of
                // storage owned elsewhere; only a `Bind`'d owned temporary that
                // the iterator may refer to is kept alive and dropped by the loop.
                let borrowed = protocol.borrowed_origin.is_some();
                let split_source = matches!(
                    protocol.mode,
                    mojito_checked::checked::IterationMode::Borrowed
                ) && (borrowed || !protocol.prepare.is_empty());
                let loop_owns_source = split_source && !borrowed && protocol.source_retained;
                if let Some(origin) = protocol.borrowed_origin.clone() {
                    self.push(HirInstr::BorrowIter {
                        dest: it_var,
                        expr: checked_iter,
                        origin,
                        mutable: binding_plan.action
                            == mojito_checked::checked::IterationBindingAction::BorrowReference
                            && binding_plan.mutable,
                    });
                } else {
                    self.push(HirInstr::Bind {
                        dest: it_var,
                        expr: checked_iter,
                        binding_ty: None,
                        binding: None,
                    });
                }
                let iter_var = if split_source {
                    self.var(&format!("$iterobj{}", self.vars.len()))
                } else {
                    it_var
                };
                // Normalize the iterable to an iterator (a user struct's `__iter__`;
                // a no-op only for compiler-private iterator storage).
                self.push(HirInstr::GetIter {
                    source: it_var,
                    dest: iter_var,
                    protocol: protocol.clone(),
                });

                let header = self.new_block();
                let body_b = self.new_block();
                let exit = self.new_block();
                let normal_exit = orelse.as_ref().map_or(exit, |_| self.new_block());
                self.scopes.push(HashMap::new());
                let v = self.declare_var(var);
                let declaration = s
                    .declaration
                    .and_then(|id| checked.declaration(id))
                    .expect("checked for-loop binding declaration");
                let binding = declaration
                    .binding
                    .expect("checked for-loop binding has a stable owner");
                debug_assert_eq!(declaration.ty.as_ref(), Some(&binding_plan.binding_ty));
                let raw_var = self.var(&format!("$yield{}", self.vars.len()));
                self.seal(Terminator::Jump(header));

                // A nominal typed-raising iterator calls `__next__` in the header
                // and branches on whether it returned an element; compiler-private
                // storage tests `HasNext` and advances with `Next` in the body.
                self.cur = header;
                let hn_name = format!("$hasnext{}", self.vars.len());
                let hn_var = self.var(&hn_name);
                if let Some(exhaustion) = protocol.exhaustion.clone() {
                    self.push(HirInstr::TryNext {
                        iter: iter_var,
                        raw: raw_var,
                        yielded: hn_var,
                        call: *protocol
                            .next
                            .clone()
                            .expect("raising iterator has checked __next__ contract"),
                        exhaustion,
                        element_ty: binding_plan.yielded_ty.clone(),
                    });
                } else {
                    self.push(HirInstr::HasNext {
                        iter: iter_var,
                        dest: hn_var,
                    });
                }
                self.seal(Terminator::BranchOnSlot {
                    cond: hn_var,
                    then_b: body_b,
                    else_b: normal_exit,
                });

                // body: the raising path already retained the element; private
                // storage advances now. Then bind x and execute the source body.
                self.cur = body_b;
                if protocol.exhaustion.is_none() {
                    self.push(HirInstr::Next {
                        iter: iter_var,
                        raw: raw_var,
                        element_ty: binding_plan.yielded_ty.clone(),
                    });
                }
                self.push(HirInstr::BindIteration {
                    raw: raw_var,
                    dest: v,
                    iter: iter_var,
                    plan: binding_plan,
                    binding,
                });
                let mut cleanup = vec![v];
                cleanup.push(raw_var);
                cleanup.push(iter_var);
                if loop_owns_source {
                    cleanup.push(it_var);
                }
                self.loops.push(LoopFrame {
                    header,
                    exit,
                    escape: false,
                    cleanup,
                });
                self.block(body);
                self.loops.pop();
                self.scopes.pop();
                self.seal(Terminator::Jump(header));
                if let Some(body) = orelse {
                    self.cur = normal_exit;
                    self.scoped_block(body);
                    self.seal(Terminator::Jump(exit));
                }
                self.cur = exit;
                // The synthetic iterator owns the normalized iterator value for
                // the lifetime of the loop.  A normal exhaustion edge is visible
                // to backward liveness, but a `break` block has no later iterator
                // use from which drop elaboration could infer that ownership dies.
                // Make the structured lifetime boundary explicit at the common
                // exit so every path (including `break`) destroys residual owned
                // iterator storage before execution continues after the loop. When
                // a split source may be referred to by its iterator, its explicit
                // exit drop also extends its liveness through the loop (no loan
                // records that dependency), so its `__deinit__` runs exactly once,
                // after the loop.
                self.push(HirInstr::Drop(iter_var));
                if loop_owns_source {
                    // An owned temporary source is used only by `GetIter` before
                    // the loop, so a liveness anchor at the exit keeps it live
                    // through the loop body; then it is destroyed exactly once,
                    // after the loop. A source the iterator cannot refer to dies
                    // right after `GetIter`, and a borrowed named source is owned
                    // by the enclosing scope (dropped there), so the loop neither
                    // anchors nor drops either.
                    self.push(HirInstr::KeepAlive(it_var));
                    self.push(HirInstr::Drop(it_var));
                }
            }

            CheckedStmtOp::Break => {
                let Some(f) = self.loops.last().cloned() else {
                    self.push(HirInstr::Stmt(self.statement(id)));
                    return;
                };
                // A loop in this CFG → a plain `Jump` to its exit; an enclosing
                // function loop (seeded into a `try` region) → an `EscapeJump`.
                self.seal(if f.escape {
                    Terminator::EscapeJump(f.exit)
                } else {
                    Terminator::Jump(f.exit)
                });
            }
            CheckedStmtOp::Continue => {
                let Some(f) = self.loops.last().cloned() else {
                    self.push(HirInstr::Stmt(self.statement(id)));
                    return;
                };
                self.seal(if f.escape {
                    Terminator::EscapeJump(f.header)
                } else {
                    Terminator::Jump(f.header)
                });
            }
            CheckedStmtOp::Return(value) => self.ret(*value),

            CheckedStmtOp::VarDecl { name, value, .. } => {
                let binding_ty = checked
                    .expression(*value)
                    .and_then(|node| node.binding_ty.clone());
                let value = self.expr(*value);
                let v = self.declare_var(name);
                self.push(HirInstr::Bind {
                    dest: v,
                    expr: value,
                    binding_ty,
                    binding: s.binding,
                });
            }
            CheckedStmtOp::RefDecl { name, value } => {
                let value = self.expr(*value);
                let runtime = self.declare_var(name);
                self.push(HirInstr::Stmt(HirStmt {
                    node: id,
                    expressions: vec![value],
                    declared: Some(runtime),
                }));
            }
            CheckedStmtOp::Assign { name, value } => {
                let value = self.expr(*value);
                let captured = self.captures.remove(name);
                if !self.scopes.iter().any(|s| s.contains_key(name))
                    && let Some(scope) = self.scopes.last_mut()
                {
                    scope.insert(name.clone(), name.clone());
                }
                let v = if captured {
                    let runtime = format!("{name}$shadow{}", self.vars.len());
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.insert(name.clone(), runtime.clone());
                    }
                    self.var(&runtime)
                } else {
                    self.var(&self.resolved(name))
                };
                self.push(HirInstr::Bind {
                    dest: v,
                    expr: value,
                    binding_ty: None,
                    binding: s.binding,
                });
            }
            CheckedStmtOp::Expr(e) => {
                let e = self.expr(*e);
                self.push(HirInstr::Eval(e));
            }

            // A `try`: snapshot the enclosing loop stack so a `break`/`continue`
            // inside it can escape to an outer loop. Supported only when every
            // enclosing loop is function-level (escapable to the function driver): a
            // function's own loops always are; a region's own loops are region-local
            // and can't be an escape target, so such a `try` falls back to the opaque
            // `Stmt` path (Stage 5 then refuses a crossing `break`/`continue`).
            CheckedStmtOp::Try { .. } => {
                let escapable = self.is_function || self.loops.iter().all(|f| f.escape);
                if escapable {
                    let loop_targets = self
                        .loops
                        .iter()
                        .map(|f| (f.header, f.exit, f.cleanup.clone()))
                        .collect();
                    self.push(HirInstr::Try {
                        stmt: self.statement(id),
                        loop_targets,
                    });
                } else {
                    self.push(HirInstr::Stmt(self.statement(id)));
                }
            }

            // Everything else (member-write, aug-assign, declarations, imports, …)
            // is kept whole for Stage 4; Stage 5 refines it.
            _ => self.push(HirInstr::Stmt(self.statement(id))),
        }
    }
}

/// One enclosing loop's `break`/`continue` targets. `escape = false` for a loop
/// in *this* CFG (a `break`/`continue` is a `Jump`); `escape = true` for a loop in
/// the enclosing **function** CFG (seeded into a `try` region — a `break`/
/// `continue` becomes an `EscapeJump` the VM propagates out).
#[derive(Clone)]
struct LoopFrame {
    header: BlockId,
    exit: BlockId,
    escape: bool,
    /// Values to destroy when a return bypasses this loop's common exit. For a
    /// `for`, this is the current element followed by the normalized iterator.
    cleanup: Vec<VarId>,
}

fn explicit_local_names(checked: &CheckedTables, body: &[CheckedStmtId]) -> HashSet<String> {
    fn walk(checked: &CheckedTables, body: &[CheckedStmtId], names: &mut HashSet<String>) {
        for statement in body.iter().filter_map(|id| checked.statement(*id)) {
            match &statement.op {
                CheckedStmtOp::VarDecl { name, .. } | CheckedStmtOp::RefDecl { name, .. } => {
                    names.insert(name.clone());
                }
                CheckedStmtOp::If { branches, orelse }
                | CheckedStmtOp::ComptimeIf { branches, orelse } => {
                    for (_, body) in branches {
                        walk(checked, body, names);
                    }
                    if let Some(body) = orelse {
                        walk(checked, body, names);
                    }
                }
                CheckedStmtOp::While { body, .. }
                | CheckedStmtOp::For { body, .. }
                | CheckedStmtOp::ComptimeFor { body, .. }
                | CheckedStmtOp::With { body, .. }
                | CheckedStmtOp::Scope(body) => walk(checked, body, names),
                CheckedStmtOp::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } => {
                    walk(checked, body, names);
                    if let Some((_, body)) = except {
                        walk(checked, body, names);
                    }
                    if let Some(body) = orelse {
                        walk(checked, body, names);
                    }
                    if let Some(body) = finalbody {
                        walk(checked, body, names);
                    }
                }
                _ => {}
            }
        }
    }

    let mut names = HashSet::new();
    walk(checked, body, &mut names);
    names
}

/// The expression roots an opaque statement owns, in source order: the
/// operands MIR lowers it against. A nested body's are its own CFG's.
fn checked_statement_roots(statement: &CheckedStmtOp) -> Vec<CheckedNodeId> {
    match statement {
        CheckedStmtOp::VarDecl { value, .. }
        | CheckedStmtOp::RefDecl { value, .. }
        | CheckedStmtOp::Assign { value, .. }
        | CheckedStmtOp::Comptime { value, .. }
        | CheckedStmtOp::Raise(value)
        | CheckedStmtOp::Expr(value) => vec![*value],
        CheckedStmtOp::SetPlace { place, value }
        | CheckedStmtOp::AugAssign { place, value, .. } => {
            vec![*place, *value]
        }
        CheckedStmtOp::Unpack { targets, value, .. } => {
            targets.iter().copied().chain([*value]).collect()
        }
        CheckedStmtOp::Return(Some(value)) => vec![*value],
        CheckedStmtOp::With { items, .. } => items.iter().map(|item| item.context).collect(),
        // Structured statements and declarations are either decomposed by HIR or
        // rebuilt as independently checked nested CFGs; their nested expressions
        // are not roots of this opaque instruction.
        _ => Vec::new(),
    }
}

/// The operands of `op` whose local names this frame resolves: every operand
/// but a compile-time parameter argument's, a t-string part, and a lambda's
/// `def`.
fn named_operands(op: &CheckedOp) -> Vec<CheckedNodeId> {
    match op {
        CheckedOp::Call { args, kwargs, .. } => args
            .iter()
            .copied()
            .chain(kwargs.iter().map(|kwarg| kwarg.value))
            .collect(),
        CheckedOp::Invoke {
            callee,
            args,
            kwargs,
            ..
        } => std::iter::once(*callee)
            .chain(args.iter().copied())
            .chain(kwargs.iter().map(|kwarg| kwarg.value))
            .collect(),
        CheckedOp::TypeApply { .. } | CheckedOp::TString { .. } | CheckedOp::Lambda { .. } => {
            Vec::new()
        }
        _ => op.operands(),
    }
}
