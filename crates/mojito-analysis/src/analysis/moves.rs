//! The ownership move checker: per-place init/move lattice (`Own`,
//! `Key`, `Node`) and the flow-sensitive `analyze_moves` driver.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// A place's move/init state. A three-point lattice ordered by how "moved" a
/// place might be; the merge of disagreeing paths is `MaybeMoved`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Own {
    /// Initialized and not transferred — safe to use.
    Owned,
    /// Transferred (`^`) on every path to here — using it is a use-after-move.
    Moved,
    /// Transferred on some paths but not others — using it is a conditional move.
    MaybeMoved,
}

/// The dataflow join (least upper bound): equal states are preserved; any
/// disagreement between `Owned` and `Moved`, or anything involving `MaybeMoved`,
/// becomes `MaybeMoved`.
pub(super) const fn join(a: Own, b: Own) -> Own {
    match (a, b) {
        (Own::Owned, Own::Owned) => Own::Owned,
        (Own::Moved, Own::Moved) => Own::Moved,
        _ => Own::MaybeMoved,
    }
}

/// A total order on the lattice: `Moved(2) > MaybeMoved(1) > Owned(0)`.
pub(super) const fn severity(o: Own) -> u8 {
    match o {
        Own::Owned => 0,
        Own::MaybeMoved => 1,
        Own::Moved => 2,
    }
}

// --- Place-tree ownership lattice (field-sensitive partial moves) -----------

/// One projection step in a place path. Dynamic indices collapse to a wildcard
/// (`Index`) and overlap every constant index. Constant indices exist only for
/// compiler-private heterogeneous Tuple storage, where they let independently
/// owned elements retain distinct move state.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Key {
    Field(String),
    Index,
    ConstIndex(usize),
    Variant(usize),
    /// Payload of compiler-private inline uninit storage. The payload is
    /// opaque to ownership tracking (all access is explicitly unsafe), so the
    /// key exists only to keep place paths total; it overlaps itself.
    UninitPayload,
}

pub(super) fn keys_overlap(left: &Key, right: &Key) -> bool {
    left == right
        || matches!(
            (left, right),
            (Key::Index, Key::ConstIndex(_)) | (Key::ConstIndex(_), Key::Index)
        )
}

/// Map a MIR place's projection chain to a path of lattice keys.
pub(super) fn place_path(place: &MirPlace) -> Vec<Key> {
    place
        .proj
        .iter()
        .map(|p| match p {
            Proj::Field(f) => Key::Field(f.clone()),
            Proj::Index(_) => Key::Index,
            Proj::ConstIndex(index) => Key::ConstIndex(*index),
            Proj::Variant(index) => Key::Variant(*index),
            Proj::UninitPayload => Key::UninitPayload,
        })
        .collect()
}

/// A human-readable place name (`p`, `p.a`, `p.items[…]`) for diagnostics.
pub(super) fn place_display(root: &str, path: &[Key]) -> String {
    let mut s = root.to_string();
    for k in path {
        match k {
            Key::Field(f) => {
                s.push('.');
                s.push_str(f);
            }
            Key::Index => s.push_str("[…]"),
            Key::ConstIndex(index) => {
                s.push('[');
                s.push_str(&index.to_string());
                s.push(']');
            }
            Key::Variant(index) => {
                s.push_str("[alternative#");
                s.push_str(&index.to_string());
                s.push(']');
            }
            Key::UninitPayload => s.push_str("[payload]"),
        }
    }
    s
}

/// The move/init state of a place *and everything under it*, as a tree. `base`
/// is the state of this node's own value and of any child not present in
/// `children`; `children` refine specific sub-places (fields / the wildcard
/// index). A partial move is `base = Owned` with a `Moved` child. Invariant: a
/// `base == Moved` node has no children (moving the whole clears sub-state); a
/// control-flow join may produce `base == MaybeMoved` with children.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Node {
    base: Own,
    children: BTreeMap<Key, Self>,
}

impl Node {
    pub(super) const fn owned() -> Self {
        Self {
            base: Own::Owned,
            children: BTreeMap::new(),
        }
    }

    /// Severity of *reading the whole subtree* at this node, paired with the
    /// relative path of the worst offender (for a precise diagnostic): the worst
    /// of its own base (path `[]`) and every descendant's whole severity — a
    /// moved child taints a whole read of the parent, and is named as the blame.
    pub(super) fn whole(&self) -> (Own, Vec<Key>) {
        let mut worst = (self.base, Vec::new());
        for (k, c) in &self.children {
            let (sev, sub) = c.whole();
            if severity(sev) > severity(worst.0) {
                let mut full = vec![k.clone()];
                full.extend(sub);
                worst = (sev, full);
            }
        }
        worst
    }

    /// The state of *reading* the place reached by `path` (its whole subtree),
    /// combined with any moved ancestor passed through along the way. Returns the
    /// severity and the blamed sub-path: a moved *ancestor* blames the ancestor,
    /// a moved *descendant* of a whole read blames the descendant.
    pub(super) fn read(&self, path: &[Key]) -> (Own, Vec<Key>) {
        match path.split_first() {
            None => self.whole(),
            // A moved ancestor on the way down blames the ancestor itself.
            Some(_) if self.base != Own::Owned => (self.base, Vec::new()),
            Some((key, rest)) => {
                let mut worst = (Own::Owned, Vec::new());
                for (candidate, child) in self
                    .children
                    .iter()
                    .filter(|(candidate, _)| keys_overlap(key, candidate))
                {
                    let (sev, subpath) = child.read(rest);
                    if severity(sev) > severity(worst.0) {
                        let mut full = vec![candidate.clone()];
                        full.extend(subpath);
                        worst = (sev, full);
                    }
                }
                worst
            }
        }
    }

    /// The base state of the *node itself* reached by `path` — only ancestor
    /// bases matter, not sibling/descendant moves. Used to check a field write,
    /// whose parent must merely be initialized (not wholly moved), so writing
    /// `p.a` is legal even when the sibling `p.b` has been moved out. Blames the
    /// nearest moved ancestor.
    pub(super) fn base_at(&self, path: &[Key]) -> (Own, Vec<Key>) {
        match path.split_first() {
            None => (self.base, Vec::new()),
            Some((_, _)) if self.base != Own::Owned => (self.base, Vec::new()),
            Some((key, rest)) => {
                let mut worst = (Own::Owned, Vec::new());
                for (candidate, child) in self
                    .children
                    .iter()
                    .filter(|(candidate, _)| keys_overlap(key, candidate))
                {
                    let (sev, subpath) = child.base_at(rest);
                    if severity(sev) > severity(worst.0) {
                        let mut full = vec![candidate.clone()];
                        full.extend(subpath);
                        worst = (sev, full);
                    }
                }
                worst
            }
        }
    }

    /// Mark the place at `path` as wholly moved (clearing its sub-state).
    pub(super) fn do_move(&mut self, path: &[Key]) {
        match path.split_first() {
            None => {
                *self = Self {
                    base: Own::Moved,
                    children: BTreeMap::new(),
                }
            }
            Some((k, rest)) => {
                let base = self.base;
                self.children
                    .entry(k.clone())
                    .or_insert_with(|| Self {
                        base,
                        children: BTreeMap::new(),
                    })
                    .do_move(rest);
            }
        }
    }

    /// Re-initialize the place at `path` to `Owned` (a def / field store).
    pub(super) fn do_def(&mut self, path: &[Key]) {
        match path.split_first() {
            None => *self = Self::owned(),
            Some((k, rest)) => {
                // Reinitializing a field of a wholly-moved value is itself invalid
                // (caught as a write through a moved parent); don't corrupt state.
                if self.base == Own::Moved {
                    return;
                }
                let base = self.base;
                self.children
                    .entry(k.clone())
                    .or_insert_with(|| Self {
                        base,
                        children: BTreeMap::new(),
                    })
                    .do_def(rest);
            }
        }
    }
}

/// Join two place-trees at a control-flow merge (a per-node dataflow lub). A key
/// present on only one side inherits that side's `base` for the missing child.
pub(super) fn join_node(a: &Node, b: &Node) -> Node {
    let base = join(a.base, b.base);
    let mut children = BTreeMap::new();
    let mut keys: Vec<&Key> = a.children.keys().chain(b.children.keys()).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let ca = a.children.get(k).cloned().unwrap_or(Node {
            base: a.base,
            children: BTreeMap::new(),
        });
        let cb = b.children.get(k).cloned().unwrap_or(Node {
            base: b.base,
            children: BTreeMap::new(),
        });
        children.insert(k.clone(), join_node(&ca, &cb));
    }
    Node { base, children }
}

/// A basic block's successors (by terminator).
pub(super) fn successors(term: &MirTerm) -> Vec<usize> {
    match term {
        MirTerm::Jump(t) => vec![*t],
        MirTerm::Branch { then_b, else_b, .. } => vec![*then_b, *else_b],
        // `EscapeJump` only appears inside a `try` region (never a function body),
        // so this — which walks function-body successors — never sees it; it leaves
        // this CFG like a `Return`.
        MirTerm::Return(_)
        | MirTerm::ReturnWithCleanup { .. }
        | MirTerm::FallOff
        | MirTerm::EscapeJump { .. } => vec![],
    }
}

/// How an instruction touches a place: a whole-value *read* (using the subtree),
/// or the *structural* parent-check of a field write (the parent must merely be
/// initialized, not wholly moved — so writing `p.a` is fine when `p.b` is moved).
pub(super) enum Touch {
    Read,
    WriteParent,
}

/// The places an instruction *reads* or structurally touches (for reporting),
/// each with the register whose span points at the offending source. Moves and
/// definitions are applied separately by [`apply_effects`].
pub(super) fn place_uses(i: &MirInstr) -> Vec<(VarId, Vec<Key>, Touch, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .map(|loan| {
                (
                    loan.place.root,
                    place_path(&loan.place),
                    Touch::Read,
                    *marker,
                )
            })
            .collect(),
        // A whole-variable read/borrow (a bare `x`) or move (`x^`): reads the
        // whole variable first.
        MirInstr::UseVar { dest, var, .. } => vec![(*var, Vec::new(), Touch::Read, *dest)],
        // A place read (`p.a`, a read-modify-write load) or a partial move
        // (`p.a^`): reads that specific sub-place.
        MirInstr::LoadPlace { dest, place } | MirInstr::MovePlace { dest, place } => {
            vec![(place.root, place_path(place), Touch::Read, *dest)]
        }
        MirInstr::ConsumePlace { place, marker } => {
            vec![(place.root, place_path(place), Touch::Read, *marker)]
        }
        MirInstr::DropPlace { place } => {
            vec![(place.root, place_path(place), Touch::Read, Reg(0))]
        }
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .map(|capture| {
                (
                    capture.place.root,
                    place_path(&capture.place),
                    Touch::Read,
                    *dest,
                )
            })
            .collect(),
        // A place write `p…​.f = e`: the *parent* place must be initialized (the
        // field itself is being overwritten, so it need not be). A statically
        // selected private Tuple element has the same independent-place
        // semantics. A dynamic-index write keeps the whole chain as the parent.
        MirInstr::Store { place, src } => {
            let mut path = place_path(place);
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                path.pop(); // drop the final sub-place — check its parent
            }
            vec![(place.root, path, Touch::WriteParent, *src)]
        }
        MirInstr::StoreRef { place, reference } => {
            let mut path = place_path(place);
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *reference)]
        }
        MirInstr::MultiSet {
            receiver_place,
            value,
            ..
        } => receiver_place
            .iter()
            .map(|place| (place.root, place_path(place), Touch::Read, *value))
            .collect(),
        MirInstr::VariantSet { place, value, .. } => {
            let mut path = place_path(place);
            if matches!(place.proj.last(), Some(Proj::Field(_))) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *value)]
        }
        MirInstr::VariantSetInitWith { place, factory, .. } => {
            let mut path = place_path(place);
            if matches!(place.proj.last(), Some(Proj::Field(_))) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *factory)]
        }
        MirInstr::VariantReplace { place, value, .. } => {
            let mut path = place_path(place);
            if matches!(place.proj.last(), Some(Proj::Field(_))) {
                path.pop();
            }
            vec![(place.root, path, Touch::WriteParent, *value)]
        }
        // The `for` iterator variable is read (and advanced) — treat as a whole read.
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            vec![(*iter, Vec::new(), Touch::Read, *dest)]
        }
        _ => Vec::new(),
    }
}

/// Apply an instruction's move/def effects to a place-tree state (no reporting):
/// a `DefVar` (re)initializes a whole variable, a `^` transfer moves one, a
/// partial move `p.a^` moves that sub-place, and a field store reinitializes the
/// written field.
pub(super) fn apply_effects(state: &mut [Node], i: &MirInstr) {
    match i {
        MirInstr::DefVar { var, .. } => state[*var as usize].do_def(&[]),
        MirInstr::UseVar {
            var,
            mode: UseMode::Move,
            ..
        } => state[*var as usize].do_move(&[]),
        MirInstr::MakeClosure { captures, .. } => {
            for capture in captures {
                if capture.mode == MirCaptureMode::Move {
                    state[capture.place.root as usize].do_move(&place_path(&capture.place));
                }
            }
        }
        MirInstr::MovePlace { place, .. } => {
            state[place.root as usize].do_move(&place_path(place));
        }
        // A named destructor's receiver: consumed by the call it follows.
        MirInstr::ConsumeVar { var } => state[*var as usize].do_move(&[]),
        MirInstr::ConsumePlace { place, .. } | MirInstr::DropPlace { place } => {
            state[place.root as usize].do_move(&place_path(place));
        }
        // A field or statically selected private Tuple-element store
        // reinitializes exactly that sub-place. A dynamic-index store cannot
        // precisely reinitialize one element, so it remains conservative.
        MirInstr::Store { place, .. }
        | MirInstr::StoreRef { place, .. }
        | MirInstr::VariantSet { place, .. }
        | MirInstr::VariantSetInitWith { place, .. }
        | MirInstr::VariantReplace { place, .. }
            if matches!(
                place.proj.last(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) =>
        {
            state[place.root as usize].do_def(&place_path(place));
        }
        _ => {}
    }
}

/// Join two per-variable place-tree states (control-flow merge).
pub(super) fn join_states(a: &[Node], b: &[Node]) -> Vec<Node> {
    a.iter().zip(b).map(|(x, y)| join_node(x, y)).collect()
}

/// A region's move states per exit channel: the normal fall-off, every
/// potentially-raising point, and `return`/escape exits. `None` marks an
/// unreachable channel.
struct MoveFlow {
    normal: Option<Vec<Node>>,
    raises: Option<Vec<Node>>,
    exits: Option<Vec<Node>>,
}

impl MoveFlow {
    const fn unreachable() -> Self {
        Self {
            normal: None,
            raises: None,
            exits: None,
        }
    }
}

/// Join a channel state into `target` (unreachable contributes nothing).
#[allow(clippy::ref_option, reason = "TODO: take Option<&T>")]
fn add_state(target: &mut Option<Vec<Node>>, source: &Option<Vec<Node>>) {
    match (target.as_mut(), source) {
        (_, None) => {}
        (None, Some(source)) => *target = Some(source.clone()),
        (Some(target), Some(source)) => *target = join_states(target, source),
    }
}

/// Analyze one function body for move violations, field-sensitively (partial
/// moves): a value transferred with `^` — whole (`x^`) or a field (`p.a^`) — may
/// not be read again on that path, but a disjoint sibling (`p.b`) stays usable.
/// `try` regions are walked with the precise raise-point rule: an `except` arm
/// starts from the join of the states at every potentially-raising
/// instruction of the body, so a value consumed before a raising call is
/// uninitialized there while one consumed after the last raise is not.
pub(super) fn analyze_moves(f: &MirFunction) -> Result<(), OwnershipError> {
    // The entry starts every variable `Owned` — the checker guarantees definite
    // assignment before use, so this never causes a false negative for our
    // purpose (tracking transfers) and avoids a spurious "uninitialized" lattice.
    let entry: Vec<Node> = vec![Node::owned(); f.n_vars];
    walk_region(Some(entry), &f.blocks, f, true).map(|_| ())
}

/// Walk one region's mini-CFG (a function body or a `try` region) from its
/// entry state: reach the per-block fixpoint, then replay each reachable
/// block, reporting every place use against the current state when `report`
/// is set. Nested `try`s recurse through [`walk_try`].
fn walk_region(
    entry: Option<Vec<Node>>,
    blocks: &[MirBlock],
    f: &MirFunction,
    report: bool,
) -> Result<MoveFlow, OwnershipError> {
    let Some(entry) = entry else {
        return Ok(MoveFlow::unreachable());
    };
    if blocks.is_empty() {
        return Ok(MoveFlow {
            normal: Some(entry),
            raises: None,
            exits: None,
        });
    }
    let in_states = region_in_states(&entry, blocks, f);
    let mut flow = MoveFlow::unreachable();
    for (b, block) in blocks.iter().enumerate() {
        let Some(mut state) = in_states[b].clone() else {
            continue;
        };
        let mut reachable = true;
        for (i, instr) in block.instrs.iter().enumerate() {
            if let MirInstr::Try { .. } = instr {
                let nested = walk_try(std::mem::take(&mut state), instr, f, report)?;
                add_state(&mut flow.raises, &nested.raises);
                add_state(&mut flow.exits, &nested.exits);
                if let Some(normal) = nested.normal {
                    state = normal;
                } else {
                    reachable = false;
                    break;
                }
                continue;
            }
            if report {
                check_instruction_uses(&state, instr, f)?;
            }
            apply_effects(&mut state, instr);
            if interior_instruction_directly_raises(instr) {
                add_state(
                    &mut flow.raises,
                    &Some(raise_seed(&state, block.instrs.get(i + 1))),
                );
            }
        }
        if !reachable {
            continue;
        }
        match block.term {
            MirTerm::FallOff => add_state(&mut flow.normal, &Some(state)),
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } | MirTerm::EscapeJump { .. } => {
                add_state(&mut flow.exits, &Some(state));
            }
            MirTerm::Jump(_) | MirTerm::Branch { .. } => {}
        }
    }
    Ok(flow)
}

/// The state a raise at this instruction hands its observer. A named
/// destructor's receiver is consumed at the call itself: the MIR emits the
/// receiver's `ConsumeVar` right after the call, so fold it into the seed.
fn raise_seed(state: &[Node], next: Option<&MirInstr>) -> Vec<Node> {
    let mut seed = state.to_vec();
    if let Some(MirInstr::ConsumeVar { var }) = next {
        seed[*var as usize].do_move(&[]);
    }
    seed
}

/// Walk the four regions of one `try`: the body from `entry`, `else` from the
/// body's normal completion, the handler from the body's raise points, and
/// `finally` from every channel (checked once from their join; its own
/// channels compose per entering channel, as the runtime runs it).
fn walk_try(
    entry: Vec<Node>,
    instr: &MirInstr,
    f: &MirFunction,
    report: bool,
) -> Result<MoveFlow, OwnershipError> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return Ok(MoveFlow {
            normal: Some(entry),
            raises: None,
            exits: None,
        });
    };
    let body_flow = walk_region(Some(entry), body, f, report)?;
    let mut normal = None;
    let mut raises = None;
    let mut exits = body_flow.exits.clone();
    if let Some(orelse) = orelse {
        let else_flow = walk_region(body_flow.normal.clone(), orelse, f, report)?;
        add_state(&mut normal, &else_flow.normal);
        add_state(&mut raises, &else_flow.raises);
        add_state(&mut exits, &else_flow.exits);
    } else {
        add_state(&mut normal, &body_flow.normal);
    }
    if let Some((_, handler)) = handler {
        let handler_flow = walk_region(body_flow.raises.clone(), handler, f, report)?;
        add_state(&mut normal, &handler_flow.normal);
        add_state(&mut raises, &handler_flow.raises);
        add_state(&mut exits, &handler_flow.exits);
    } else {
        add_state(&mut raises, &body_flow.raises);
    }
    let Some(finalbody) = finalbody else {
        return Ok(MoveFlow {
            normal,
            raises,
            exits,
        });
    };
    if report {
        let mut all = normal.clone();
        add_state(&mut all, &raises);
        add_state(&mut all, &exits);
        walk_region(all, finalbody, f, true)?;
    }
    let normal_final = walk_region(normal, finalbody, f, false)?;
    let raising_final = walk_region(raises, finalbody, f, false)?;
    let exiting_final = walk_region(exits, finalbody, f, false)?;
    let mut raises = raising_final.normal;
    add_state(&mut raises, &normal_final.raises);
    add_state(&mut raises, &raising_final.raises);
    add_state(&mut raises, &exiting_final.raises);
    let mut exits = exiting_final.normal;
    add_state(&mut exits, &normal_final.exits);
    add_state(&mut exits, &raising_final.exits);
    add_state(&mut exits, &exiting_final.exits);
    Ok(MoveFlow {
        normal: normal_final.normal,
        raises,
        exits,
    })
}

/// Per-block in-states of a region at the dataflow fixpoint:
/// `in[b] = ⨆ out[pred]`, `out[b] = transfer(in[b])`. `Owned` is a real
/// program state, not the lattice bottom, so unvisited blocks stay absent
/// (a loop header must not join a definite preheader move with a seeded
/// backedge and report `MaybeMoved` forever).
fn region_in_states(
    entry: &[Node],
    blocks: &[MirBlock],
    f: &MirFunction,
) -> Vec<Option<Vec<Node>>> {
    let nb = blocks.len();
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (b, blk) in blocks.iter().enumerate() {
        for s in successors(&blk.term) {
            if s < nb {
                preds[s].push(b);
            }
        }
    }
    let mut in_states: Vec<Option<Vec<Node>>> = vec![None; nb];
    let mut out_states: Vec<Option<Vec<Node>>> = vec![None; nb];
    let mut changed = true;
    while changed {
        changed = false;
        #[allow(clippy::needless_range_loop)]
        for b in 0..nb {
            let new_in = if b == 0 || preds[b].is_empty() {
                entry.to_vec()
            } else {
                let mut predecessors = preds[b]
                    .iter()
                    .filter_map(|predecessor| out_states[*predecessor].as_ref());
                let Some(first) = predecessors.next() else {
                    continue;
                };
                let mut acc = first.clone();
                for predecessor in predecessors {
                    acc = join_states(&acc, predecessor);
                }
                acc
            };
            let new_out = transfer_block(new_in.clone(), &blocks[b].instrs, f);
            if in_states[b].as_ref() != Some(&new_in) || out_states[b] != new_out {
                in_states[b] = Some(new_in);
                out_states[b] = new_out;
                changed = true;
            }
        }
    }
    in_states
}

/// Apply a block's instructions to a state without reporting; a nested `try`
/// continues from its normal completion, and an always-raising or
/// always-exiting one leaves the rest of the block unreachable.
fn transfer_block(mut state: Vec<Node>, instrs: &[MirInstr], f: &MirFunction) -> Option<Vec<Node>> {
    for instr in instrs {
        if let MirInstr::Try { .. } = instr {
            let nested = walk_try(state, instr, f, false).ok()?;
            state = nested.normal?;
        } else {
            apply_effects(&mut state, instr);
        }
    }
    Some(state)
}

/// Check one instruction's place uses against the current state, returning
/// the first violation.
fn check_instruction_uses(
    state: &[Node],
    instr: &MirInstr,
    f: &MirFunction,
) -> Result<(), OwnershipError> {
    for (root, path, touch, reg) in place_uses(instr) {
        let node = &state[root as usize];
        let (sev, blame) = match touch {
            Touch::Read => node.read(&path),
            Touch::WriteParent => node.base_at(&path),
        };
        if sev != Own::Owned {
            let span = f.spans.0.get(&reg.0).map_or_else(
                || mojito_common::token::SourceSpan::new(None, (0, 0)),
                |(s, _)| s.clone(),
            );
            let var = place_display(&f.var_names[root as usize], &blame);
            return Err(match sev {
                Own::Moved => OwnershipError::UseAfterMove { var, span },
                _ => OwnershipError::ConditionallyMoved { var, span },
            });
        }
    }
    Ok(())
}
