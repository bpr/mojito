//! Stage 4: lower the checked statement and expression nodes into a
//! **region tree** of basic blocks, as upstream's parser and
//! `LowerSemanticCF` build `hlcf` ops.
//!
//! Expressions stay *nested* here (as checked nodes); Stage 5 flattens them to
//! A-Normal Form. Ownership and liveness analysis (Stages 6–7) run over the
//! flattened form, not this one.
//!
//! A [`Cfg`] is built per function body (or the top-level statement list) via
//! [`Cfg::build_checked_fn`]. Its [`Cfg::root`] region holds the body's
//! blocks, entry first; every block ends in exactly one [`Terminator`]. A
//! run-time `if` is an [`HirInstr::If`] (`hlcf.if`), a `while`/`for` a
//! labelled [`HirInstr::Loop`] (`hlcf.loop`), a `comptime if` an
//! [`HirInstr::ComptimeIf`] (`hlcf.comptime.if`), and a `comptime for` an
//! [`HirInstr::ComptimeFor`] (`hlcf.comptime.for`), and a `try` an
//! [`HirInstr::Try`] (`lit.try`), each holding its own regions;
//! `break`/`continue` are region terminators naming their loop, no
//! terminator jumps between blocks, and every region is one block. A local name resolves to its slot once, here, recorded per node in
//! [`Cfg::slots`].

use mojito_checked::checked::{
    CheckedCommand, CheckedCondition, CheckedExpr, CheckedMatchCase, CheckedNodeId, CheckedOp,
    CheckedProgram, CheckedStmt, CheckedStmtId, CheckedStmtOp, CheckedTables, SemanticAdjustment,
};
use mojito_types::origin::OwnerId;
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
    /// `var x: T` without an initializer: the slot exists, uninitialized,
    /// with no def. The first assignment initializes it.
    Declare {
        dest: VarId,
        ty: Ty,
        binding: Option<mojito_types::origin::OwnerId>,
    },
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
    /// variable slots so both IRs and the backend address them uniformly. A
    /// borrowed runtime pack yields element references, and MIR indexes it
    /// in place rather than advancing a consuming iterator.
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
    /// storing whether it yielded in `yielded`; its exact result is bound to
    /// `raw` by the [`HirInstr::BindIteration`] past the exhaustion test, so
    /// the exhausted edge never fills it. Raising the checked exhaustion type
    /// sets `yielded = False`; every other runtime error propagates normally.
    TryNext {
        iter: VarId,
        raw: VarId,
        yielded: VarId,
        /// Exact checker-selected reference/value result and effect contract.
        call: mojito_checked::checked::CheckedIteratorCall,
        exhaustion: Ty,
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
    /// `lit.try`: `body`, then the `except` handler when it raises, `orelse`
    /// when it completes, and `finalbody` on every exit. Every part that
    /// completes ends in [`Terminator::TryYield`]; a `break`/`continue` in a
    /// part names the enclosing loop it leaves.
    Try {
        body: HirRegion,
        except: Option<HirExcept>,
        orelse: Option<HirRegion>,
        finalbody: Option<HirRegion>,
    },
    /// `hlcf.if`: `then` when `cond` holds, else each of `elifs` in order,
    /// else `orelse`. Every arm that completes ends in [`Terminator::Yield`].
    If {
        cond: HirCond,
        then: HirRegion,
        elifs: Vec<HirElif>,
        orelse: HirRegion,
    },
    /// `hlcf.comptime.if`: one arm per condition in `conds`, then `orelse`.
    /// Conditions are compile-time expressions MIR lowers as parameter
    /// expressions, not values; every arm that completes ends in
    /// [`Terminator::ComptimeYield`]. The elaborator keeps one arm, after
    /// ownership has decided all of them.
    ComptimeIf {
        conds: Vec<HirExpr>,
        arms: Vec<HirRegion>,
        orelse: HirRegion,
    },
    /// `kgen.param.assert`: a compile-time assertion, its condition a
    /// compile-time expression MIR lowers as a parameter expression. The
    /// elaborator judges it per reached instance.
    ComptimeAssert {
        cond: HirExpr,
        message: Option<String>,
    },
    /// `hlcf.loop`, labelled `label` (unique in its function, as
    /// `LowerSemanticCF` numbers loops): `body` runs until a
    /// [`Terminator::Break`] names it, and [`Terminator::Continue`] starts
    /// the next iteration. A `while`/`for` body opens with an [`HirInstr::If`]
    /// on its condition or has-next flag whose else arm holds the loop's
    /// `else` body and then breaks.
    Loop { label: u32, body: HirRegion },
    /// `hlcf.comptime.for`: the iterable is a compile-time sequence MIR
    /// lowers as parameter expressions; the loop variable `var` is the
    /// checked binding `binding`, an alias of the element over the loop's
    /// iterator with no slot. `body` ends an iteration
    /// with [`Terminator::ComptimeContinue`] and leaves the loop with
    /// [`Terminator::ComptimeBreak`]. `orelse` is the arm taken when the
    /// iterator's `has_next` fails, ended by [`Terminator::ComptimeBreak`]:
    /// MIR opens the body with that test, as `LowerSemanticCF` leaves it.
    /// The elaborator unrolls it.
    ComptimeFor {
        iter: HirExpr,
        var: String,
        binding: Option<mojito_types::origin::OwnerId>,
        body: HirRegion,
        orelse: HirRegion,
    },
    /// `hlcf.match`: `cases` tried in order, each a region that tests its
    /// pattern and runs its body — a failed test or guard ends in
    /// [`Terminator::MatchNext`], a body that completes in
    /// [`Terminator::MatchComplete`] — then `orelse` when none matched,
    /// ended by [`Terminator::Yield`]. A `break`/`continue` in a case names
    /// the enclosing loop.
    Match {
        cases: Vec<HirRegion>,
        orelse: HirRegion,
    },
}

/// A region of a region op: its one block, a node of its [`Cfg::g`].
#[derive(Debug, Clone, Default)]
pub struct HirRegion {
    pub blocks: Vec<BlockId>,
}

/// An [`HirInstr::If`]'s operand: a checked condition, or a `Bool` slot HIR
/// itself wrote (a `for` loop's has-next flag), which no checked node
/// computes.
#[derive(Debug, Clone)]
pub enum HirCond {
    Expr(HirExpr),
    Slot(VarId),
}

/// One `elif` arm: the region computing its condition, ended by
/// [`Terminator::ElifCond`], and the arm itself.
#[derive(Debug, Clone)]
pub struct HirElif {
    pub cond: HirRegion,
    pub then: HirRegion,
}

/// An [`HirInstr::Try`]'s `except` part: its handler's checked `binding`
/// and `slot` when it names the error, and its region, whose scope the
/// handler's name is declared in.
#[derive(Debug, Clone)]
pub struct HirExcept {
    pub binding: Option<OwnerId>,
    pub slot: Option<VarId>,
    pub body: HirRegion,
}

/// The loop a `break`/`continue` leaves: a labelled [`HirInstr::Loop`] or
/// the innermost [`HirInstr::ComptimeFor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopTarget {
    Loop(u32),
    ComptimeFor,
}

/// One enclosing loop: the target its `break`/`continue` name, and the
/// values to destroy when a return bypasses its common exit (for a `for`,
/// the current element followed by the normalized iterator).
#[derive(Debug, Clone)]
pub struct EnclosingLoop {
    pub target: LoopTarget,
    pub cleanup: Vec<VarId>,
}

/// How a block hands control to its successor(s). Exactly one per block.
#[derive(Debug, Clone)]
pub enum Terminator {
    Return(Option<HirExpr>),
    /// A return nested in one or more iterator-driven loops. The value is
    /// evaluated before `cleanup` is destroyed (innermost iterator first), so
    /// moving or copying the current element into the result remains valid while
    /// residual owned iterator storage cannot leak past the function boundary.
    ReturnWithCleanup {
        value: Option<HirExpr>,
        cleanup: Vec<VarId>,
    },
    /// `lit.try.yield`: an [`HirInstr::Try`] part completed. Unlike
    /// `Return(None)` (an explicit bare `return`), the `try` continues to its
    /// `else`/`finally`, not out of the frame.
    TryYield,
    /// `hlcf.yield`: an [`HirInstr::If`] arm completed.
    Yield,
    /// `hlcf.comptime.yield`: an [`HirInstr::ComptimeIf`] arm completed.
    ComptimeYield,
    /// `hlcf.if.elifcond.yield`: an elif condition region's `Bool`.
    ElifCond(HirExpr),
    /// `hlcf.break`: leave the [`HirInstr::Loop`] labelled so.
    Break(u32),
    /// `hlcf.continue`: start the next iteration of the loop labelled so.
    Continue(u32),
    /// `hlcf.comptime.for.break`: leave the innermost
    /// [`HirInstr::ComptimeFor`].
    ComptimeBreak,
    /// `hlcf.comptime.for.continue`: end this iteration of the innermost
    /// [`HirInstr::ComptimeFor`].
    ComptimeContinue,
    /// `hlcf.match.next`: the innermost [`HirInstr::Match`] case did not
    /// match; try the next.
    MatchNext,
    /// `hlcf.match.complete`: the innermost [`HirInstr::Match`] case ran;
    /// leave the op.
    MatchComplete,
}

#[derive(Debug, Default)]
pub struct BasicBlock {
    pub instrs: Vec<HirInstr>,
    pub term: Option<Terminator>,
}

/// The region tree for one function body / statement list: [`Cfg::g`] is the
/// block arena, and holds no edges.
#[derive(Debug)]
pub struct Cfg {
    pub g: StableGraph<BasicBlock, ()>,
    /// The body's region: its blocks, entry first. Region ops nest the rest.
    pub root: HirRegion,
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
    /// The slot of each checked binding this CFG declared or was seeded
    /// with: a local name node's [`Cfg::slots`] entry is its binding's slot.
    pub bindings: HashMap<OwnerId, VarId>,
    /// The statement nodes of the body this CFG lowers.
    pub body: Vec<CheckedStmtId>,
    /// The next loop label free after this CFG's loops, from which MIR
    /// numbers the loops it adds.
    pub labels: u32,
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
            region_blocks: vec![entry],
            loops: Vec::new(),
            next_label: 0,
            vars: Vec::new(),
            scopes: Vec::new(),
            captures: shadow_captures,
            checked,
            slots: HashMap::new(),
            bindings: HashMap::new(),
        };
        let checked = Arc::clone(&lower.checked);
        let body = checked.live_statements(body);
        let explicit = explicit_local_names(&checked, body);
        let mut implicit = HashSet::new();
        collect_function_implicit_names(&checked, body, &explicit, &mut implicit);
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
        if let Some(value) = returned.filter(|_| lower.g[lower.cur].term.is_none()) {
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
    for statement in checked
        .live_statements(body)
        .iter()
        .filter_map(|id| checked.statement(*id))
    {
        let bodies: Vec<&[CheckedStmtId]> = match &statement.op {
            CheckedStmtOp::Assign { name, .. } => {
                if name != "_" && !explicit.contains(name) {
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
            CheckedStmtOp::Match { .. } => statement.op.bodies(),
            _ => Vec::new(),
        };
        for body in bodies {
            collect_function_implicit_names(checked, body, explicit, names);
        }
    }
}

/// Lowering cursor: appends to the "current" block of the region being
/// built. Invariant: `cur` is the current insertion point; a block gets its
/// terminator exactly once, via [`Lower::seal`].
struct Lower {
    g: StableGraph<BasicBlock, ()>,
    cur: BlockId,
    /// The blocks of the region being built, entry first.
    region_blocks: Vec<BlockId>,
    /// Innermost-last stack of enclosing loops.
    loops: Vec<EnclosingLoop>,
    /// The next loop label free in this function.
    next_label: u32,
    /// Interner: a variable name's first appearance assigns its `VarId`.
    vars: Vec<String>,
    scopes: Vec<HashMap<String, String>>,
    captures: HashSet<String>,
    /// The program's checked tables, shared with every other lowering.
    checked: Arc<CheckedTables>,
    /// The slot each local-name node resolved to.
    slots: HashMap<CheckedNodeId, VarId>,
    /// The slot of each checked binding declared so far.
    bindings: HashMap<OwnerId, VarId>,
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
        let block = self.g.add_node(BasicBlock::default());
        self.region_blocks.push(block);
        block
    }

    /// A region of its own: a fresh entry block that `build` lowers into,
    /// its last block sealed with the terminator `build` returns unless a
    /// `return`, `break`, or `continue` sealed it already.
    fn region(&mut self, build: impl FnOnce(&mut Self) -> Terminator) -> HirRegion {
        let outer = std::mem::take(&mut self.region_blocks);
        let cur = self.cur;
        self.cur = self.new_block();
        let end = build(self);
        self.seal(end);
        self.cur = cur;
        HirRegion {
            blocks: std::mem::replace(&mut self.region_blocks, outer),
        }
    }

    /// A fresh loop label.
    const fn label(&mut self) -> u32 {
        self.next_label += 1;
        self.next_label - 1
    }

    /// Append `i` to the current block. Dead statements are erased before
    /// they lower ([`CheckedTables::live_statements`]), and `push` still
    /// never appends to a sealed block: every region stays one block.
    fn push(&mut self, i: HirInstr) {
        if self.g[self.cur].term.is_none() {
            self.g[self.cur].instrs.push(i);
        }
    }

    /// Give `cur` its terminator — but only if it isn't already sealed (a
    /// `break`/`continue`/`return` inside a body may have sealed it first, in
    /// which case this is a no-op).
    fn seal(&mut self, t: Terminator) {
        if self.g[self.cur].term.is_none() {
            self.g[self.cur].term = Some(t);
        }
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
            root: HirRegion {
                blocks: self.region_blocks,
            },
            entry,
            vars: self.vars,
            n_params,
            checked: self.checked,
            slots: self.slots,
            bindings: self.bindings,
            body,
            labels: self.next_label,
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

    /// [`Lower::declare_var`] for a declaration of checked `binding`, whose
    /// slot later reads of the binding resolve to.
    fn declare_binding(&mut self, name: &str, binding: Option<OwnerId>) -> VarId {
        let slot = self.declare_var(name);
        if let Some(binding) = binding {
            self.bindings.insert(binding, slot);
        }
        slot
    }

    /// Declare an `except` name's slot in the innermost scope: the slot the
    /// frame already gave its binding, else its own spelling unless a slot
    /// of the frame already has it, then one suffixed by its binding.
    fn declare_handler(&mut self, binding: Option<OwnerId>, name: &str) -> VarId {
        let declared = binding.and_then(|binding| self.bindings.get(&binding).copied());
        let runtime = match (declared, binding) {
            (Some(slot), _) => self.vars[slot as usize].clone(),
            (None, Some(binding)) if self.vars.iter().any(|candidate| candidate == name) => {
                format!("{name}$binding{}", binding.0)
            }
            _ => name.to_string(),
        };
        let slot = self.var(&runtime);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), runtime);
        }
        if let Some(binding) = binding {
            self.bindings.insert(binding, slot);
        }
        slot
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
            let slot = match node.binding.and_then(|binding| self.bindings.get(&binding)) {
                Some(slot) => *slot,
                None => self.var(&self.resolved(name)),
            };
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
        let checked = Arc::clone(&self.checked);
        for s in checked.live_statements(body) {
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

    fn stmt(&mut self, id: CheckedStmtId) {
        let checked = Arc::clone(&self.checked);
        let s = checked
            .statement(id)
            .expect("a checked body names a statement node of its tables");
        match &s.op {
            CheckedStmtOp::If { branches, orelse } => {
                self.if_statement(id, branches, orelse.as_deref());
            }
            CheckedStmtOp::ComptimeIf { branches, orelse } => {
                self.comptime_if(id, branches, orelse.as_deref());
            }
            CheckedStmtOp::Scope(body) => self.scoped_block(body),
            CheckedStmtOp::ComptimeAssert { cond, message } => {
                let cond = self.expr(*cond);
                self.push(HirInstr::ComptimeAssert {
                    cond,
                    message: message.clone(),
                });
            }
            CheckedStmtOp::ComptimeFor { var, iter, body } => {
                self.comptime_for(var, *iter, s.binding, body);
            }
            CheckedStmtOp::While { cond, body, orelse } => {
                self.while_loop(cond, body, orelse.as_deref());
            }
            CheckedStmtOp::Match {
                anchor: Some(anchor),
                cases,
            } => self.match_statement(*anchor, cases),
            CheckedStmtOp::For { .. } => self.for_loop(s),
            CheckedStmtOp::Break => self.leave(id, true),
            CheckedStmtOp::Continue => self.leave(id, false),
            CheckedStmtOp::Return(value) => self.ret(*value),

            CheckedStmtOp::VarDecl { name, value, .. }
                if checked
                    .expression(*value)
                    .is_some_and(|node| matches!(node.op, CheckedOp::Uninitialized)) =>
            {
                let Some(ty) = s.binding_ty.clone() else {
                    self.push(HirInstr::Stmt(self.statement(id)));
                    return;
                };
                let dest = self.declare_binding(name, s.binding);
                self.push(HirInstr::Declare {
                    dest,
                    ty,
                    binding: s.binding,
                });
            }
            CheckedStmtOp::VarDecl { name, value, .. } => {
                let binding_ty = checked
                    .expression(*value)
                    .and_then(|node| node.binding_ty.clone());
                let value = self.expr(*value);
                let v = self.declare_binding(name, s.binding);
                self.push(HirInstr::Bind {
                    dest: v,
                    expr: value,
                    binding_ty,
                    binding: s.binding,
                });
            }
            CheckedStmtOp::RefDecl { name, value } => {
                let value = self.expr(*value);
                let runtime = self.declare_binding(name, s.binding);
                self.push(HirInstr::Stmt(HirStmt {
                    node: id,
                    expressions: vec![value],
                    declared: Some(runtime),
                }));
            }
            // `_` discards: each statement binds a fresh unnamed slot.
            CheckedStmtOp::Assign { name, value } if name == "_" => {
                let value = self.expr(*value);
                let dest = self.var(&format!("$discard_s{}", self.vars.len()));
                self.push(HirInstr::Bind {
                    dest,
                    expr: value,
                    binding_ty: None,
                    binding: s.binding,
                });
            }
            CheckedStmtOp::Assign { value, .. }
                if let Some(dest) = s
                    .binding
                    .and_then(|binding| self.bindings.get(&binding).copied()) =>
            {
                let value = self.expr(*value);
                self.push(HirInstr::Bind {
                    dest,
                    expr: value,
                    binding_ty: None,
                    binding: s.binding,
                });
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
                if let Some(binding) = s.binding {
                    self.bindings.insert(binding, v);
                }
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

            CheckedStmtOp::Try {
                body,
                except,
                orelse,
                finalbody,
            } => self.try_statement(
                body,
                except
                    .as_ref()
                    .map(|(name, body)| (name.as_deref(), body.as_slice())),
                orelse.as_deref(),
                finalbody.as_deref(),
                s.binding,
            ),

            // Everything else (member-write, aug-assign, declarations, imports, …)
            // is kept whole for Stage 4; Stage 5 refines it.
            _ => self.push(HirInstr::Stmt(self.statement(id))),
        }
    }
    /// `if`/`elif`/`else` as `hlcf.if`: the first condition is the op's
    /// operand, evaluated here; each later one is an elif condition region.
    fn if_statement(
        &mut self,
        id: CheckedStmtId,
        branches: &[(CheckedCondition, Vec<CheckedStmtId>)],
        orelse: Option<&[CheckedStmtId]>,
    ) {
        let Some(((first, then), rest)) = branches.split_first() else {
            return;
        };
        // A chain holding a `case` condition is a chain of one-armed
        // conditionals, each branch's test in the previous one's else.
        if branches.iter().any(|(cond, _)| cond.boolean().is_none()) {
            self.conditional_chain(branches, orelse);
            return;
        }
        let Some(first) = first.boolean() else {
            self.push(HirInstr::Stmt(self.statement(id)));
            return;
        };
        let cond = HirCond::Expr(self.expr(first));
        let then = self.arm(Some(then));
        let mut elifs = Vec::new();
        for (cond, body) in rest {
            let Some(cond) = cond.boolean() else {
                self.push(HirInstr::Stmt(self.statement(id)));
                return;
            };
            let cond = self.region(|this| Terminator::ElifCond(this.expr(cond)));
            elifs.push(HirElif {
                cond,
                then: self.arm(Some(body)),
            });
        }
        let orelse = self.arm(orelse);
        self.push(HirInstr::If {
            cond,
            then,
            elifs,
            orelse,
        });
    }

    /// An `if`/`elif`/`else` chain with a `case` condition, one branch at
    /// a time: a boolean branch is an `hlcf.if` whose else region holds
    /// the rest of the chain; a `case pattern = value` branch anchors the
    /// value — evaluated only when the branch is reached — and matches it
    /// as a one-case `hlcf.match` whose body is the branch's and whose else
    /// region holds the rest of the chain, so the bindings exist in the
    /// body alone.
    fn conditional_chain(
        &mut self,
        branches: &[(CheckedCondition, Vec<CheckedStmtId>)],
        orelse: Option<&[CheckedStmtId]>,
    ) {
        let Some(((cond, body), rest)) = branches.split_first() else {
            if let Some(orelse) = orelse {
                self.scoped_block(orelse);
            }
            return;
        };
        match cond {
            CheckedCondition::Expr(node) => {
                let cond = HirCond::Expr(self.expr(*node));
                let then = self.arm(Some(body));
                let orelse = self.region(|this| {
                    this.conditional_chain(rest, orelse);
                    Terminator::Yield
                });
                self.push(HirInstr::If {
                    cond,
                    then,
                    elifs: Vec::new(),
                    orelse,
                });
            }
            CheckedCondition::Case {
                anchor, commands, ..
            } => {
                self.scopes.push(HashMap::new());
                if let Some(anchor) = anchor {
                    self.stmt(*anchor);
                }
                let case = self.region(|this| {
                    this.scopes.push(HashMap::new());
                    this.case_commands(commands);
                    this.block(body);
                    this.scopes.pop();
                    Terminator::MatchComplete
                });
                let orelse = self.region(|this| {
                    this.conditional_chain(rest, orelse);
                    Terminator::Yield
                });
                self.push(HirInstr::Match {
                    cases: vec![case],
                    orelse,
                });
                self.scopes.pop();
            }
        }
    }

    /// `match` as `hlcf.match`: the anchor declares the subject's slot in the
    /// enclosing scope, then each case is a region of its own scope — every
    /// test and the guard an `hlcf.if` whose else arm is `match.next`, every
    /// binding its declaration, then the body, ended by `match.complete` —
    /// and the else region yields the fall-through when no case matched.
    fn match_statement(&mut self, anchor: CheckedStmtId, cases: &[CheckedMatchCase]) {
        self.stmt(anchor);
        let regions = cases
            .iter()
            .map(|case| {
                self.region(|this| {
                    this.scopes.push(HashMap::new());
                    this.case_commands(&case.commands);
                    if let Some(guard) = case.guard {
                        this.case_test(guard);
                    }
                    this.block(&case.body);
                    this.scopes.pop();
                    Terminator::MatchComplete
                })
            })
            .collect();
        let orelse = self.region(|_| Terminator::Yield);
        self.push(HirInstr::Match {
            cases: regions,
            orelse,
        });
    }

    /// A case's commands in order: a test (a comparison or a sequence
    /// length test) is a branch whose failure advances, an element read or
    /// a binding a statement of the case's scope. A `|` pattern's alternatives are an
    /// inner `hlcf.match` in the case's own scope: each alternative a case
    /// region whose failed test advances to the next alternative and whose
    /// success completes to the commands after the `|`, and whose else
    /// region advances the enclosing case; a binding the first alternative
    /// declares is the slot the later ones assign.
    fn case_commands(&mut self, commands: &[CheckedCommand]) {
        for command in commands {
            match command {
                CheckedCommand::Equal { test } | CheckedCommand::ListLength { test } => {
                    self.case_test(*test);
                }
                CheckedCommand::Bind { declaration }
                | CheckedCommand::PathValue { declaration } => self.stmt(*declaration),
                CheckedCommand::Alternatives(alternatives) => {
                    let cases = alternatives
                        .iter()
                        .map(|commands| {
                            self.region(|this| {
                                this.case_commands(commands);
                                Terminator::MatchComplete
                            })
                        })
                        .collect();
                    let orelse = self.region(|_| Terminator::MatchNext);
                    self.push(HirInstr::Match { cases, orelse });
                }
            }
        }
    }

    /// One test of a case: `hlcf.if` over `test`, whose else arm advances to
    /// the next case.
    fn case_test(&mut self, test: CheckedNodeId) {
        let cond = HirCond::Expr(self.expr(test));
        let then = self.region(|_| Terminator::Yield);
        let orelse = self.region(|_| Terminator::MatchNext);
        self.push(HirInstr::If {
            cond,
            then,
            elifs: Vec::new(),
            orelse,
        });
    }

    /// `try`/`except`/`else`/`finally` as `lit.try`, each part a region of
    /// this walk; a named handler, the checked `binding`, is declared in its
    /// `except` region's scope.
    fn try_statement(
        &mut self,
        body: &[CheckedStmtId],
        except: Option<(Option<&str>, &[CheckedStmtId])>,
        orelse: Option<&[CheckedStmtId]>,
        finalbody: Option<&[CheckedStmtId]>,
        binding: Option<OwnerId>,
    ) {
        let body = self.try_part(body);
        let except = except.map(|(name, body)| {
            let mut slot = None;
            let body = self.region(|this| {
                this.scopes.push(HashMap::new());
                slot = name.map(|name| this.declare_handler(binding, name));
                this.block(body);
                this.scopes.pop();
                Terminator::TryYield
            });
            HirExcept {
                binding: name.and(binding),
                slot,
                body,
            }
        });
        let orelse = orelse.map(|body| self.try_part(body));
        let finalbody = finalbody.map(|body| self.try_part(body));
        self.push(HirInstr::Try {
            body,
            except,
            orelse,
            finalbody,
        });
    }

    /// A `try` part: `body` in its own scope, then `try.yield`.
    fn try_part(&mut self, body: &[CheckedStmtId]) -> HirRegion {
        self.region(|this| {
            this.scoped_block(body);
            Terminator::TryYield
        })
    }

    /// An `if` arm: `body` in its own scope, then `yield`.
    fn arm(&mut self, body: Option<&[CheckedStmtId]>) -> HirRegion {
        self.region(|this| {
            if let Some(body) = body {
                this.scoped_block(body);
            }
            Terminator::Yield
        })
    }

    /// `comptime if`/`elif`/`else` as `hlcf.comptime.if`: every condition
    /// first, as the op's attributes, then one region per arm.
    fn comptime_if(
        &mut self,
        id: CheckedStmtId,
        branches: &[(CheckedCondition, Vec<CheckedStmtId>)],
        orelse: Option<&[CheckedStmtId]>,
    ) {
        let Some(conds) = branches
            .iter()
            .map(|(cond, _)| cond.boolean())
            .collect::<Option<Vec<_>>>()
        else {
            self.push(HirInstr::Stmt(self.statement(id)));
            return;
        };
        let conds = conds.into_iter().map(|cond| self.expr(cond)).collect();
        let arms = branches
            .iter()
            .map(|(_, body)| self.comptime_arm(Some(body)))
            .collect();
        let orelse = self.comptime_arm(orelse);
        self.push(HirInstr::ComptimeIf {
            conds,
            arms,
            orelse,
        });
    }

    /// A `comptime if` arm: `body` in its own scope, then `comptime.yield`.
    fn comptime_arm(&mut self, body: Option<&[CheckedStmtId]>) -> HirRegion {
        self.region(|this| {
            if let Some(body) = body {
                this.scoped_block(body);
            }
            Terminator::ComptimeYield
        })
    }

    /// A `comptime for` as `hlcf.comptime.for`: the body is analysed once
    /// with the trip count unknown, `break` and `continue` are the loop's,
    /// and the elaborator unrolls it.
    fn comptime_for(
        &mut self,
        var: &str,
        iter: CheckedNodeId,
        binding: Option<OwnerId>,
        body: &[CheckedStmtId],
    ) {
        let iter = self.expr(iter);
        self.scopes.push(HashMap::new());
        self.loops.push(EnclosingLoop {
            target: LoopTarget::ComptimeFor,
            cleanup: Vec::new(),
        });
        let body = self.region(|this| {
            this.block(body);
            Terminator::ComptimeContinue
        });
        self.loops.pop();
        self.scopes.pop();
        let orelse = self.region(|_| Terminator::ComptimeBreak);
        self.push(HirInstr::ComptimeFor {
            iter,
            var: var.to_string(),
            binding,
            body,
            orelse,
        });
    }

    /// `while cond: body else: orelse` as a labelled `hlcf.loop` whose body
    /// tests `cond` first (`lowerLITLoop`).
    fn while_loop(
        &mut self,
        cond: &CheckedCondition,
        body: &[CheckedStmtId],
        orelse: Option<&[CheckedStmtId]>,
    ) {
        let label = self.label();
        let body = match cond {
            CheckedCondition::Expr(cond) => self.region(|this| {
                let cond = HirCond::Expr(this.expr(*cond));
                this.loop_test(cond, label, orelse);
                this.loops.push(EnclosingLoop {
                    target: LoopTarget::Loop(label),
                    cleanup: Vec::new(),
                });
                this.scoped_block(body);
                this.loops.pop();
                Terminator::Continue(label)
            }),
            // `while case`: each visit of the header anchors the value
            // again and matches it; a failed match runs the loop's `else`
            // and leaves, as a false condition does, and the bindings exist
            // in the body alone.
            CheckedCondition::Case {
                anchor, commands, ..
            } => self.region(|this| {
                this.scopes.push(HashMap::new());
                if let Some(anchor) = anchor {
                    this.stmt(*anchor);
                }
                let orelse = this.region(|this| {
                    if let Some(body) = orelse {
                        this.scoped_block(body);
                    }
                    Terminator::Break(label)
                });
                let case = this.region(|this| {
                    this.scopes.push(HashMap::new());
                    this.case_commands(commands);
                    this.loops.push(EnclosingLoop {
                        target: LoopTarget::Loop(label),
                        cleanup: Vec::new(),
                    });
                    this.block(body);
                    this.loops.pop();
                    this.scopes.pop();
                    Terminator::MatchComplete
                });
                this.push(HirInstr::Match {
                    cases: vec![case],
                    orelse,
                });
                this.scopes.pop();
                Terminator::Continue(label)
            }),
        };
        self.push(HirInstr::Loop { label, body });
    }

    /// A loop body's opening test: go on when `cond` holds, else run the
    /// loop's `orelse` body and break. The `else` body lowers outside this
    /// loop's frame, so its own `break`/`continue` name the enclosing loop,
    /// as `LowerSemanticCF` lowers it.
    fn loop_test(&mut self, cond: HirCond, label: u32, orelse: Option<&[CheckedStmtId]>) {
        let then = self.region(|_| Terminator::Yield);
        let orelse = self.region(|this| {
            if let Some(body) = orelse {
                this.scoped_block(body);
            }
            Terminator::Break(label)
        });
        self.push(HirInstr::If {
            cond,
            then,
            elifs: Vec::new(),
            orelse,
        });
    }

    /// `for x in it: body else: orelse` — a while-shaped loop driving the
    /// iterator protocol: the iterable is evaluated once into an iterator
    /// slot before the loop; the loop body computes `has_next` and tests
    /// it, then binds the loop variable via `next` (which advances the
    /// iterator).
    fn for_loop(&mut self, s: &CheckedStmt) {
        let CheckedStmtOp::For {
            var,
            binding: binding_mode,
            iter,
            body,
            orelse,
        } = &s.op
        else {
            return;
        };
        let checked = Arc::clone(&self.checked);
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
        // The iterator `__iter__` returns is a value of its own type in a
        // slot of its own, apart from `it_var`, which holds either a
        // `BorrowIter` reference handle to the named source or the
        // `Bind`-bound owned temporary: a borrowing `__iter__` must not
        // clobber the source the iterator still refers to, and an owned
        // one (`__iter__(var self)`) consumes the source, whose slot is
        // empty from then on. Only an empty-`prepare` iterable, already
        // compiler-private iterator storage, normalizes in place.
        // A `BorrowIter`'d source (borrowed origin present) is a loan of
        // storage owned elsewhere; only a `Bind`'d borrowed temporary that
        // the iterator may refer to is kept alive and dropped by the loop.
        let borrowed = protocol.borrowed_origin.is_some();
        let split_source = borrowed || !protocol.prepare.is_empty();
        let loop_owns_source = split_source
            && !borrowed
            && protocol.source_retained
            && protocol.mode == mojito_checked::checked::IterationMode::Borrowed;
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

        self.scopes.push(HashMap::new());
        let declaration = s
            .declaration
            .and_then(|id| checked.declaration(id))
            .expect("checked for-loop binding declaration");
        let binding = declaration
            .binding
            .expect("checked for-loop binding has a stable owner");
        let v = self.declare_binding(var, Some(binding));
        debug_assert_eq!(declaration.ty.as_ref(), Some(&binding_plan.binding_ty));
        let raw_var = self.var(&format!("$yield{}", self.vars.len()));
        let hn_var = self.var(&format!("$hasnext{}", self.vars.len()));
        let label = self.label();
        // The yielded slot is not listed: the binding always consumes it.
        let mut cleanup = vec![v, iter_var];
        if loop_owns_source {
            cleanup.push(it_var);
        }
        let body = self.region(|this| {
            // A nominal typed-raising iterator calls `__next__` first and
            // tests whether it returned an element; compiler-private
            // storage tests `HasNext` and advances with `Next` after.
            let exhaustion = protocol.exhaustion.clone();
            let raising = exhaustion.is_some();
            if let Some(exhaustion) = exhaustion {
                this.push(HirInstr::TryNext {
                    iter: iter_var,
                    raw: raw_var,
                    yielded: hn_var,
                    call: *protocol
                        .next
                        .clone()
                        .expect("raising iterator has checked __next__ contract"),
                    exhaustion,
                });
            } else {
                this.push(HirInstr::HasNext {
                    iter: iter_var,
                    dest: hn_var,
                });
            }
            // The loop variable is out of scope in the `else` body.
            let scope = this.scopes.pop();
            this.loop_test(HirCond::Slot(hn_var), label, orelse.as_deref());
            this.scopes.extend(scope);
            if !raising {
                this.push(HirInstr::Next {
                    iter: iter_var,
                    raw: raw_var,
                    element_ty: binding_plan.yielded_ty.clone(),
                });
            }
            this.push(HirInstr::BindIteration {
                raw: raw_var,
                dest: v,
                iter: iter_var,
                plan: binding_plan,
                binding,
            });
            this.loops.push(EnclosingLoop {
                target: LoopTarget::Loop(label),
                cleanup,
            });
            this.block(body);
            this.loops.pop();
            Terminator::Continue(label)
        });
        self.scopes.pop();
        self.push(HirInstr::Loop { label, body });
        // Lowering emits no drops: the synthetic iterator, defined by
        // `GetIter`, dies on every loop exit edge (`break` included),
        // where drop elaboration destroys it.
        if loop_owns_source {
            // An owned temporary source is used only by `GetIter` before
            // the loop, so a liveness anchor after it keeps it live
            // through the loop body, and elaboration destroys it exactly
            // once, after the loop. A source the iterator cannot refer to
            // dies right after `GetIter`, and a borrowed named source is
            // owned by the enclosing scope (dropped there), so the loop
            // anchors neither.
            self.push(HirInstr::KeepAlive(it_var));
        }
    }

    /// `break` (`exits`) or `continue`: a terminator naming the innermost
    /// enclosing loop; outside every loop, the statement MIR rejects.
    fn leave(&mut self, id: CheckedStmtId, exits: bool) {
        let Some(frame) = self.loops.last() else {
            self.push(HirInstr::Stmt(self.statement(id)));
            return;
        };
        self.seal(match (frame.target, exits) {
            (LoopTarget::Loop(label), true) => Terminator::Break(label),
            (LoopTarget::Loop(label), false) => Terminator::Continue(label),
            (LoopTarget::ComptimeFor, true) => Terminator::ComptimeBreak,
            (LoopTarget::ComptimeFor, false) => Terminator::ComptimeContinue,
        });
    }
}

fn explicit_local_names(checked: &CheckedTables, body: &[CheckedStmtId]) -> HashSet<String> {
    fn walk(checked: &CheckedTables, body: &[CheckedStmtId], names: &mut HashSet<String>) {
        for statement in checked
            .live_statements(body)
            .iter()
            .filter_map(|id| checked.statement(*id))
        {
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
                CheckedStmtOp::Match { .. } => {
                    for body in statement.op.bodies() {
                        walk(checked, body, names);
                    }
                }
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
        CheckedStmtOp::Destructure { target, value } => {
            target.expressions().into_iter().chain([*value]).collect()
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
