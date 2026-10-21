//! The lifetime scan: the per-place init/move lattice (`Own`, `Key`,
//! `Node`) and the flow-sensitive `analyze_lifetimes` driver, which checks
//! moves and interior origins in one walk.

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
    /// A variant payload, by its known alternative; `None` is a template's
    /// symbolic alternative, which may fold to any position.
    Variant(Option<usize>),
    /// Payload of compiler-private inline uninit storage. The payload is
    /// opaque to ownership tracking (all access is explicitly unsafe), so the
    /// key exists only to keep place paths total; it overlaps itself.
    UninitPayload,
}

pub(super) fn keys_overlap(left: &Key, right: &Key) -> bool {
    left == right
        || matches!(
            (left, right),
            (Key::Index, Key::ConstIndex(_))
                | (Key::ConstIndex(_), Key::Index)
                | (Key::Variant(None), Key::Variant(_))
                | (Key::Variant(_), Key::Variant(None))
        )
}

/// Map a MIR place's projection chain to a path of lattice keys.
pub(super) fn place_path(place: &MirPlace) -> Vec<Key> {
    place
        .proj
        .iter()
        .filter_map(|p| match p {
            Proj::Field(f) => Some(Key::Field(f.clone())),
            Proj::Index(_) => Some(Key::Index),
            Proj::ConstIndex(index) => Some(Key::ConstIndex(*index)),
            Proj::Variant(index) => Some(Key::Variant(index.known())),
            Proj::UninitPayload => Some(Key::UninitPayload),
            Proj::Rebind => None,
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
                s.push_str(&index.map_or_else(|| "?".to_string(), |index| index.to_string()));
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

    /// An uninitialized place: one not yet defined, or wholly moved out.
    pub(super) const fn moved() -> Self {
        Self {
            base: Own::Moved,
            children: BTreeMap::new(),
        }
    }

    /// A value whose storage exists but whose named fields are all
    /// uninitialized, as an initializer's `out self` receiver enters its body.
    pub(super) fn with_uninitialized_fields(fields: impl IntoIterator<Item = String>) -> Self {
        Self {
            base: Own::Owned,
            children: fields
                .into_iter()
                .map(|field| (Key::Field(field), Self::moved()))
                .collect(),
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

    /// The first field-only sub-place below this node whose value is moved
    /// (or maybe moved) while the node's own value is not: the hole that a
    /// whole destruction or redefinition of this value would trip over.
    /// Sub-places below an index, alternative, or payload key are
    /// compiler-private storage and are not holes.
    pub(super) fn field_hole(&self) -> Option<Vec<Key>> {
        self.moved_below(self.base, None)
    }

    /// A moved field-only sub-place disjoint from the place at `path`: at
    /// every node along the path, a field child off the path that is more
    /// moved than that node. `skip_root` leaves the node itself out, for a
    /// `deinit` receiver whose direct fields are independent values.
    pub(super) fn disjoint_moved(&self, path: &[Key], skip_root: bool) -> Option<Vec<Key>> {
        let (next, rest) = path.split_first()?;
        if !skip_root && let Some(hole) = self.moved_below(self.base, Some(next)) {
            return Some(hole);
        }
        let child = self.children.get(next)?;
        child.disjoint_moved(rest, false).map(|sub| {
            let mut full = vec![next.clone()];
            full.extend(sub);
            full
        })
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

    /// The first field child (other than one overlapping `skip`) moved beyond
    /// `base`, or such a hole deeper under an intact field child.
    fn moved_below(&self, base: Own, skip: Option<&Key>) -> Option<Vec<Key>> {
        self.children
            .iter()
            .filter(|(key, _)| {
                matches!(key, Key::Field(_)) && !skip.is_some_and(|skip| keys_overlap(key, skip))
            })
            .find_map(|(key, child)| {
                let sub = if severity(child.base) > severity(base) {
                    Vec::new()
                } else {
                    child.moved_below(child.base, None)?
                };
                let mut full = vec![key.clone()];
                full.extend(sub);
                Some(full)
            })
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

/// Each block's predecessors within `blocks` (from terminator successors).
pub(super) fn block_predecessors(blocks: &[MirBlock]) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); blocks.len()];
    for (block, body) in blocks.iter().enumerate() {
        for successor in terminator_targets(&body.term) {
            if let Some(list) = predecessors.get_mut(successor) {
                list.push(block);
            }
        }
    }
    predecessors
}

/// The blocks a dataflow over one mini-CFG still has to visit. A forward
/// dataflow takes them in reverse postorder from block 0 (then any block
/// unreachable from it, in index order), so each block is visited after its
/// forward predecessors; a backward one takes them in the opposite order.
pub(super) struct Worklist {
    order: Vec<usize>,
    /// Each block's position in `order`.
    rank: Vec<usize>,
    pending: BTreeSet<usize>,
}

impl Worklist {
    /// A forward worklist holding every block.
    pub(super) fn forward(blocks: &[MirBlock]) -> Self {
        let nb = blocks.len();
        let mut visited = vec![false; nb];
        let mut order = Vec::with_capacity(nb);
        if nb > 0 {
            let mut stack = vec![(0, terminator_targets(&blocks[0].term), 0)];
            visited[0] = true;
            while let Some((block, block_successors, next)) = stack.last_mut() {
                if let Some(successor) = block_successors.get(*next).copied() {
                    *next += 1;
                    if successor < nb && !visited[successor] {
                        visited[successor] = true;
                        stack.push((successor, terminator_targets(&blocks[successor].term), 0));
                    }
                } else {
                    order.push(*block);
                    stack.pop();
                }
            }
        }
        order.reverse();
        order.extend((0..nb).filter(|block| !visited[*block]));
        Self::over(order)
    }

    /// A backward worklist holding every block.
    pub(super) fn backward(blocks: &[MirBlock]) -> Self {
        let mut order = Self::forward(blocks).order;
        order.reverse();
        Self::over(order)
    }

    /// The pending block first in the worklist's order.
    pub(super) fn pop(&mut self) -> Option<usize> {
        self.pending
            .pop_first()
            .map(|position| self.order[position])
    }

    /// Revisit `block`.
    pub(super) fn push(&mut self, block: usize) {
        if let Some(rank) = self.rank.get(block) {
            self.pending.insert(*rank);
        }
    }

    /// Revisit the successors of a block whose outgoing state changed.
    pub(super) fn push_successors(&mut self, term: &MirTerm) {
        for successor in terminator_targets(term) {
            self.push(successor);
        }
    }

    fn over(order: Vec<usize>) -> Self {
        let mut rank = vec![0; order.len()];
        for (position, block) in order.iter().enumerate() {
            rank[*block] = position;
        }
        Self {
            pending: (0..order.len()).collect(),
            order,
            rank,
        }
    }
}

/// The per-function facts the move walk reads beside the instructions.
pub(super) struct MoveContext {
    /// Each `MakeRef` handle naming a reference parameter's whole referent,
    /// by register: a `WriteRef` or `ReadRef` carries only the register, so
    /// the parameter it writes back or reads is recovered here.
    refills: HashMap<u32, VarId>,
    /// Each reference slot holding a call operand awaiting its setup read,
    /// with the owner places its setup loans name: the read through the slot
    /// reads them, so an owner moved between the operand and its call's setup
    /// is used after its move.
    setup_owners: HashMap<VarId, Vec<MirPlace>>,
}

/// How an instruction touches a place: a whole-value *read* (using the subtree),
/// or a write, whose *structural* check is on the parent (the parent must merely
/// be initialized, not wholly moved — so reinitializing a moved `p.a` is fine).
pub(super) enum Touch {
    Read,
    Write { parent: Vec<Key> },
}

/// The places an instruction *reads* or structurally touches (for reporting),
/// each with the register whose span points at the offending source. Moves and
/// definitions are applied separately by [`apply_effects`].
pub(super) fn place_uses(
    i: &MirInstr,
    context: &MoveContext,
) -> Vec<(VarId, Vec<Key>, Touch, Reg)> {
    match i {
        // A read through a reference parameter's own handle reads the whole
        // referent.
        MirInstr::ReadRef { dest, reference } => context
            .refills
            .get(&reference.0)
            .map(|var| (*var, Vec::new(), Touch::Read, *dest))
            .into_iter()
            .collect(),
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
            let owners = place
                .through
                .and_then(|through| context.setup_owners.get(&through))
                .into_iter()
                .flatten()
                .map(|owner| (owner.root, place_path(owner), Touch::Read, *dest));
            std::iter::once((place.root, place_path(place), Touch::Read, *dest))
                .chain(owners)
                .collect()
        }
        MirInstr::ConsumePlace { place, marker } => {
            vec![(place.root, place_path(place), Touch::Read, *marker)]
        }
        MirInstr::DropPlace { place, .. } | MirInstr::MarkDestroyed { place } => {
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
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(
                place.steps().next_back(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                parent.pop(); // drop the final sub-place — check its parent
            }
            vec![(place.root, path, Touch::Write { parent }, *src)]
        }
        MirInstr::StoreRef { place, reference } => {
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(
                place.steps().next_back(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                parent.pop();
            }
            vec![(place.root, path, Touch::Write { parent }, *reference)]
        }
        // Marking a place initialized writes it without a value: its parent
        // must hold a value, as for a store.
        MirInstr::MarkInitialized { place } => {
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(
                place.steps().next_back(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) {
                parent.pop();
            }
            vec![(place.root, path, Touch::Write { parent }, Reg(0))]
        }
        MirInstr::MultiSet {
            receiver_place,
            value,
            ..
        } => receiver_place
            .as_deref()
            .into_iter()
            .map(|place| (place.root, place_path(place), Touch::Read, *value))
            .collect(),
        MirInstr::VariantSet { place, value, .. } => {
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(place.steps().next_back(), Some(Proj::Field(_))) {
                parent.pop();
            }
            vec![(place.root, path, Touch::Write { parent }, *value)]
        }
        MirInstr::VariantSetInitWith { place, factory, .. } => {
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(place.steps().next_back(), Some(Proj::Field(_))) {
                parent.pop();
            }
            vec![(place.root, path, Touch::Write { parent }, *factory)]
        }
        MirInstr::VariantReplace { place, value, .. } => {
            let path = place_path(place);
            let mut parent = path.clone();
            if matches!(place.steps().next_back(), Some(Proj::Field(_))) {
                parent.pop();
            }
            vec![(place.root, path, Touch::Write { parent }, *value)]
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
pub(super) fn apply_effects(state: &mut [Node], i: &MirInstr, context: &MoveContext) {
    match i {
        // A whole write through a reference parameter's own handle puts a
        // value back into the caller's storage.
        MirInstr::WriteRef { reference, .. } => {
            if let Some(var) = context.refills.get(&reference.0) {
                state[*var as usize].do_def(&[]);
            }
        }
        MirInstr::DefVar { var, .. } => state[*var as usize].do_def(&[]),
        MirInstr::UseVar { var, mode, .. } if mode.moves() => state[*var as usize].do_move(&[]),
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
        MirInstr::GetIter { .. } => {
            if let Some(source) = i.consumed_iteration_source() {
                state[source as usize].do_move(&[]);
            }
        }
        MirInstr::ConsumePlace { place, .. }
        | MirInstr::DropPlace { place, .. }
        | MirInstr::MarkDestroyed { place } => {
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
                place.steps().next_back(),
                Some(Proj::Field(_) | Proj::ConstIndex(_))
            ) =>
        {
            state[place.root as usize].do_def(&place_path(place));
        }
        // A place marked initialized holds a value from here on, whatever
        // storage lies behind it (upstream's `lit.ownership.mark_initialized`).
        MirInstr::MarkInitialized { place } => {
            state[place.root as usize].do_def(&place_path(place));
        }
        _ => {}
    }
}

/// Join two per-variable place-tree states (control-flow merge).
pub(super) fn join_states(a: &[Node], b: &[Node]) -> Vec<Node> {
    a.iter().zip(b).map(|(x, y)| join_node(x, y)).collect()
}

/// Scan one function body's lifetimes as upstream's `UninitializedValueScan`
/// scans them, carrying `TrackedAndInteriorLiveness`: each variable's
/// move/init place-tree and the interior-origin generations its references
/// carry, checked together in walk order.
///
/// Moves are field-sensitive (partial moves): a value transferred with `^`
/// — whole (`x^`) or a field (`p.a^`) — may not be read again on that path,
/// but a disjoint sibling (`p.b`) stays usable. A use through a reference
/// whose interior origin a container mutation invalidated on any path to
/// it is rejected.
///
/// A structured body is walked by the region walk (`region_walk.rs`): an
/// `if` joins its arms, a `match` case starts from the states its
/// predecessors' `match.next` left, and a loop iterates its body until the
/// state its `continue`s carry back stops changing, the states at its
/// `break`s being its live-out. A `break`/`continue` reaches the loop it
/// names through any `finally` it crosses. `try` regions are walked with
/// the precise raise-point rule: an `except` arm starts from the join of
/// the states at every potentially-raising instruction of the body, so a
/// value consumed before a raising call is uninitialized there while one
/// consumed after the last raise is not.
///
/// A field moved out of a value must be written back before any other part
/// of the value is used, before the variable is redefined, and before the
/// function exits; a `deinit` parameter's direct fields are exempt because
/// each is destroyed on its own.
pub(super) fn analyze_lifetimes(f: &MirFunction) -> Result<(), OwnershipError> {
    let scan = LifetimeScan {
        f,
        context: move_context(f),
        interior: collect_interior_generations(&f.blocks),
    };
    // The entry starts every variable `Owned` — the checker guarantees definite
    // assignment before use, so this never causes a false negative for our
    // purpose (tracking transfers) and avoids a spurious "uninitialized" lattice.
    let entry = LifetimeState {
        moves: vec![Node::owned(); f.n_vars],
        interior: InteriorState::default(),
    };
    let mut check = |state: &LifetimeState, instr: &MirInstr| -> Result<(), OwnershipError> {
        check_instruction_uses(&state.moves, instr, f, &scan.context)?;
        if let MirInstr::DefVar { var, .. } = instr {
            check_whole_destruction(&state.moves[*var as usize], *var, f)?;
        }
        if !scan.interior.is_empty() {
            check_interior_instruction_uses(&state.interior, instr, f)?;
        }
        Ok(())
    };
    let flow = walk_forward(&scan, entry, &f.blocks, &mut check)?;
    // A reference parameter is the caller's storage, so it holds a whole
    // value again on each channel that leaves the function.
    for (state, raising) in [
        (&flow.normal, false),
        (&flow.exits, false),
        (&flow.raises, true),
    ] {
        for (var, node) in (0..).zip(state.iter().flat_map(|state| &state.moves)) {
            check_reference_parameter_refilled(node, var, f, raising)?;
        }
    }
    // Every variable dies at the function's exit, on each channel that
    // leaves it: a value that gets there with a field moved out cannot be
    // destroyed as a whole.
    for state in [&flow.normal, &flow.exits, &flow.raises]
        .into_iter()
        .flatten()
    {
        for (var, node) in (0..).zip(&state.moves) {
            check_whole_destruction(node, var, f)?;
        }
    }
    Ok(())
}

/// Replay one function body's move states from `entry`, handing `observer`
/// the state that reaches each instruction, before its effects, exactly once.
/// A `finally` body is observed from the join of every channel entering it.
pub(super) fn observe_move_states(
    f: &MirFunction,
    entry: Vec<Node>,
    observer: &mut impl FnMut(&[Node], &MirInstr),
) -> Result<(), OwnershipError> {
    let scan = LifetimeScan {
        f,
        context: move_context(f),
        interior: BTreeMap::new(),
    };
    let entry = LifetimeState {
        moves: entry,
        interior: InteriorState::default(),
    };
    let mut observe = |state: &LifetimeState, instr: &MirInstr| -> Result<(), OwnershipError> {
        observer(&state.moves, instr);
        Ok(())
    };
    walk_forward(&scan, entry, &f.blocks, &mut observe).map(|_| ())
}

/// The state one program point carries in the lifetime scan.
#[derive(Clone, PartialEq)]
struct LifetimeState {
    /// Each variable's move/init place-tree.
    moves: Vec<Node>,
    /// The interior-origin generations each reference may carry.
    interior: InteriorState,
}

/// The lifetime scan of one function: the facts its walk reads beside the
/// instructions, and the interior generations it tracks (none when the
/// body establishes no interior reference).
struct LifetimeScan<'f> {
    f: &'f MirFunction,
    context: MoveContext,
    interior: BTreeMap<u32, InteriorGeneration>,
}

impl ForwardScan for LifetimeScan<'_> {
    type State = LifetimeState;

    fn join(&self, state: &LifetimeState, other: &LifetimeState) -> LifetimeState {
        LifetimeState {
            moves: join_states(&state.moves, &other.moves),
            interior: join_interior_states(state.interior.clone(), &other.interior),
        }
    }

    fn transfer(&self, state: &mut LifetimeState, instr: &MirInstr) {
        apply_effects(&mut state.moves, instr, &self.context);
        if !self.interior.is_empty() {
            transfer_interior_instruction(&mut state.interior, instr, &self.interior, self.f);
        }
    }

    /// A named destructor's receiver is consumed at the call itself: the
    /// MIR emits the receiver's `ConsumeVar` right after the call, so fold
    /// it into the seed.
    fn raise_seed(&self, state: &LifetimeState, next: Option<&MirInstr>) -> LifetimeState {
        let mut seed = state.clone();
        if let Some(MirInstr::ConsumeVar { var }) = next {
            seed.moves[*var as usize].do_move(&[]);
        }
        seed
    }

    fn enters(&self, _instr: &MirInstr) -> bool {
        true
    }
}

/// Check one instruction's place uses against the current state, returning
/// the first violation: a read or write through a moved place, then a use of
/// a place beside a hole the same value carries. A further move of a sibling
/// is not a use — it widens the hole, which the end-of-life check reports.
fn check_instruction_uses(
    state: &[Node],
    instr: &MirInstr,
    f: &MirFunction,
    context: &MoveContext,
) -> Result<(), OwnershipError> {
    let widens_hole = matches!(
        instr,
        MirInstr::MovePlace { .. }
            | MirInstr::ConsumePlace { .. }
            | MirInstr::DropPlace { .. }
            | MirInstr::MarkDestroyed { .. }
    );
    for (root, path, touch, reg) in place_uses(instr, context) {
        let node = &state[root as usize];
        let (sev, blame) = match &touch {
            Touch::Read => node.read(&path),
            Touch::Write { parent } => node.base_at(parent),
        };
        if sev != Own::Owned {
            let var = place_display(&f.var_names[root as usize], &blame);
            return Err(match sev {
                Own::Moved => OwnershipError::UseAfterMove {
                    var,
                    span: use_span(f, reg),
                },
                _ => OwnershipError::ConditionallyMoved {
                    var,
                    span: use_span(f, reg),
                },
            });
        }
        if widens_hole {
            continue;
        }
        if let Some(hole) = node.disjoint_moved(&path, is_deinit_root(f, root)) {
            return Err(OwnershipError::ConsumedFieldUsedLater {
                field: place_display(&f.var_names[root as usize], &hole),
                var: f.var_names[root as usize].clone(),
                span: use_span(f, reg),
            });
        }
    }
    Ok(())
}

/// The whole-referent handles of `f`'s reference parameters and the owners
/// of its setup reference slots.
fn move_context(f: &MirFunction) -> MoveContext {
    let mut context = MoveContext {
        refills: HashMap::new(),
        setup_owners: HashMap::new(),
    };
    for_each_instr_deep(&f.blocks, &mut |instr| match instr {
        MirInstr::MakeRef { dest, place }
            if place.is_whole_root()
                && place.through == Some(place.root)
                && is_reference_parameter(f, place.root) =>
        {
            context.refills.insert(dest.0, place.root);
        }
        MirInstr::EstablishLoans {
            reference, loans, ..
        } => {
            let owners = loans
                .iter()
                .filter(|loan| loan.setup)
                .map(|loan| loan.place.clone());
            context
                .setup_owners
                .entry(*reference)
                .or_default()
                .extend(owners);
        }
        _ => {}
    });
    context.setup_owners.retain(|_, owners| !owners.is_empty());
    context
}

fn is_reference_parameter(f: &MirFunction, var: VarId) -> bool {
    (var as usize) < f.n_params && f.ref_params.get(var as usize).copied().unwrap_or(false)
}

/// Reject leaving the function with reference parameter `var` — or a field
/// of it — transferred away and not written back.
fn check_reference_parameter_refilled(
    node: &Node,
    var: VarId,
    f: &MirFunction,
    raising: bool,
) -> Result<(), OwnershipError> {
    if !is_reference_parameter(f, var) {
        return Ok(());
    }
    let (own, path) = node.whole();
    if own == Own::Owned {
        return Ok(());
    }
    let var_name = place_display(&f.var_names[var as usize], &path);
    let span = partial_move_span(f, var, &path);
    Err(if raising {
        OwnershipError::UseAfterMove {
            var: var_name,
            span,
        }
    } else {
        OwnershipError::ReferenceParameterLeftUninitialized {
            var: var_name,
            span,
        }
    })
}

/// The span recorded for a use's register, or an empty span when the
/// instruction has none.
fn use_span(f: &MirFunction, reg: Reg) -> mojito_common::token::SourceSpan {
    f.spans.0.get(&reg.0).map_or_else(
        || mojito_common::token::SourceSpan::new(None, (0, 0)),
        |(span, _)| span.clone(),
    )
}

/// Whether `var` is a `deinit` parameter, whose direct fields are independent
/// values: each is destroyed at its own last use, so moving one out leaves
/// no hole in the receiver, while a move below a direct field still does.
fn is_deinit_root(f: &MirFunction, var: VarId) -> bool {
    (var as usize) < f.n_params && f.deinit_params.get(var as usize).copied().unwrap_or(false)
}

/// Reject a whole destruction or redefinition of `var` while a field of it
/// (or, for a `deinit` parameter, a field of one of its direct fields) is
/// moved out and not reinitialized.
fn check_whole_destruction(node: &Node, var: VarId, f: &MirFunction) -> Result<(), OwnershipError> {
    let hole = if is_deinit_root(f, var) {
        node.children.iter().find_map(|(key, child)| {
            child.field_hole().map(|sub| {
                let mut full = vec![key.clone()];
                full.extend(sub);
                full
            })
        })
    } else {
        node.field_hole()
    };
    hole.map_or(Ok(()), |path| {
        Err(OwnershipError::FieldDestroyedOutOfTheMiddle {
            field: place_display(&f.var_names[var as usize], &path),
            span: partial_move_span(f, var, &path),
        })
    })
}

/// The source span of the transfer that moved `path` out of `var`: the
/// blamed hole's own `^`.
fn partial_move_span(
    f: &MirFunction,
    var: VarId,
    path: &[Key],
) -> mojito_common::token::SourceSpan {
    let mut reg = None;
    for_each_instr_deep(&f.blocks, &mut |instr| {
        let moved = match instr {
            MirInstr::MovePlace { dest, place } => Some((*dest, place)),
            MirInstr::ConsumePlace { place, marker } => Some((*marker, place)),
            _ => None,
        };
        if reg.is_none()
            && let Some((candidate, place)) = moved
            && place.root == var
            && place_path(place) == path
        {
            reg = Some(candidate);
        }
    });
    reg.map_or_else(
        || mojito_common::token::SourceSpan::new(None, (0, 0)),
        |reg| use_span(f, reg),
    )
}
