//! Shared MIR scans: per-instruction variable uses/defs/moves, deep
//! instruction walks, and droppable-root classification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The variables a MIR instruction reads, each paired with a nearby result
/// register (for a diagnostic span). Covers direct reads (`UseVar`), place roots
/// (`Store`/`LoadPlace`/a `mut self` receiver — so a write *through* a moved value
/// is caught too), and the `for` iterator variable.
pub(super) fn var_uses(i: &MirInstr) -> Vec<(VarId, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .flat_map(|loan| place_loan_uses(&loan.place, *marker))
            .collect(),
        MirInstr::MakeRef { dest, place } => place_loan_uses(place, *dest),
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .flat_map(|capture| place_loan_uses(&capture.place, *dest))
            .collect(),
        MirInstr::UseVar { dest, var, .. } => vec![(*var, *dest)],
        MirInstr::KeepAlive { var } => vec![(*var, Reg(0))],
        MirInstr::MovePlace { dest, place } => place_loan_uses(place, *dest),
        MirInstr::Store { place, src } => place_loan_uses(place, *src),
        MirInstr::StoreRef { place, reference } => place_loan_uses(place, *reference),
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            value,
            ..
        } => {
            let mut uses = receiver_place
                .as_deref()
                .into_iter()
                .chain(arg_places.iter().flatten())
                .chain(value_place.as_deref())
                .flat_map(|place| place_loan_uses(place, *value))
                .collect::<Vec<_>>();
            uses.extend(
                call.capture_accesses
                    .iter()
                    .map(|access| (access.root, *value)),
            );
            uses
        }
        MirInstr::VariantSet { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::VariantSetInitWith { place, factory, .. } => place_loan_uses(place, *factory),
        MirInstr::VariantReplace { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::LoadPlace { dest, place } => place_loan_uses(place, *dest),
        // Consuming a projected field reads its root: a `deinit` receiver's
        // whole-variable teardown must follow the field consumption, not
        // precede it.
        MirInstr::ConsumePlace { place, marker } => place_loan_uses(place, *marker),
        MirInstr::DropPlace { place, .. }
        | MirInstr::MarkInitialized { place }
        | MirInstr::MarkDestroyed { place } => place_loan_uses(place, Reg(0)),
        // Call arguments are evaluated into registers first, but mutable/ref
        // conventions still consume their retained places at the call
        // boundary for handle passing and write-back. Keep those slots alive
        // through the instruction. (Interior stale-use checking deliberately
        // does not count this as a second value read.)
        MirInstr::Call {
            dest,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = arg_places
                .iter()
                .flatten()
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::ConstructTypeParam {
            dest, kwarg_places, ..
        } => kwarg_places
            .iter()
            .flatten()
            .flat_map(|place| place_loan_uses(place, *dest))
            .collect(),
        MirInstr::CallIndirect {
            dest,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = callee_place
                .as_deref()
                .into_iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::MethodCall {
            dest,
            recv_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = recv_place
                .as_deref()
                .into_iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .as_deref()
                .into_iter()
                .chain(index_place.as_deref())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::Slice {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .as_deref()
                .into_iter()
                .chain(index_place.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            vec![(*iter, *dest)]
        }
        // `GetIter` normalizes its source into the iterator; count the source as a
        // use so a borrowed named source (a reference bound before the loop) stays
        // live through the read and is not dropped before the iterator is derived.
        MirInstr::GetIter { source, .. } => vec![(*source, Reg(0))],
        // A `try` reads every variable its sub-regions read: the outer liveness must
        // treat it as one big use, so a value used only inside the `try` is not
        // dropped *before* it.
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            let mut uses = Vec::new();
            let mut add = |bs: &[MirBlock]| {
                for b in bs {
                    for instr in &b.instrs {
                        uses.extend(var_uses(instr));
                    }
                }
            };
            add(body);
            if let Some((_, h)) = handler {
                add(h);
            }
            if let Some(e) = orelse {
                add(e);
            }
            if let Some(fb) = finalbody {
                add(fb);
            }
            uses
        }
        _ => Vec::new(),
    }
}

pub(super) fn place_loan_uses(place: &MirPlace, reg: Reg) -> Vec<(VarId, Reg)> {
    let mut uses = vec![(place.root, reg)];
    if let Some(reference) = place.through {
        uses.push((reference, reg));
    }
    uses
}

/// The variables **local** to a `try` region's blocks — those whose every
/// def site (`DefVar` or `DeclareVar`) in the whole function lies within the
/// region (at any nesting), excluding those [`consumed_in_place`] — the
/// body-local candidates of the `try`'s raise-edge cleanup. A local a `UseVar` transfers stays listed: the
/// transfer empties its slot, which the cleanup then skips. A variable declared outside the region — a
/// `DeclareVar` or a `DefVar` beyond it — has a def site there and survives
/// the exit, however many of its stores lie inside; `function_defs` supplies
/// the function-wide def-site counts to compare against.
pub(super) fn region_cleanup_vars(
    blocks: &[MirBlock],
    function_defs: &HashMap<VarId, usize>,
) -> Vec<VarId> {
    let mut counts: HashMap<VarId, usize> = HashMap::new();
    let mut order: Vec<VarId> = Vec::new();
    let mut moved: HashSet<VarId> = HashSet::new();
    collect_region_defs(blocks, &mut counts, &mut order, &mut moved);
    let consumed = consumed_in_place(blocks);
    order.retain(|v| !consumed.contains(v) && counts.get(v) == function_defs.get(v));
    order
}

/// The variables a region moves out other than by a `UseVar` (which
/// empties its slot), deep through `try` regions: a `ConsumeVar` or a
/// whole-root move capture, which a cleanup must not destroy again.
pub(super) fn consumed_in_place(blocks: &[MirBlock]) -> HashSet<VarId> {
    let mut consumed = HashSet::new();
    for_each_instr_deep(blocks, &mut |instruction| {
        if !matches!(instruction, MirInstr::UseVar { .. }) {
            consumed.extend(vars_moved(instruction));
        }
    });
    consumed
}

/// Count every def site (a `DefVar` write, a `DeclareVar`, or a handler's
/// error binding) per variable across `blocks`, recursing into nested `try`
/// sub-regions through [`collect_try_defs`], recording
/// first-site order and `^`-moved variables. Over a function's top-level
/// blocks this yields the function-wide def-site counts that region-locality
/// is judged against.
pub(super) fn collect_region_defs(
    blocks: &[MirBlock],
    counts: &mut HashMap<VarId, usize>,
    order: &mut Vec<VarId>,
    moved: &mut HashSet<VarId>,
) {
    for b in blocks {
        for instr in &b.instrs {
            if let Some(v) = var_def(instr).or_else(|| var_declared(instr)) {
                if !counts.contains_key(&v) {
                    order.push(v);
                }
                *counts.entry(v).or_insert(0) += 1;
            }
            for v in vars_moved(instr) {
                moved.insert(v);
            }
            collect_try_defs(instr, counts, order, moved);
        }
    }
}

/// Count the def sites of a `try`'s four regions as [`collect_region_defs`]
/// does. The handler's error binding is one def site, at the handler's entry:
/// the caught error arrives as that block's argument.
pub(super) fn collect_try_defs(
    try_instr: &MirInstr,
    counts: &mut HashMap<VarId, usize>,
    order: &mut Vec<VarId>,
    moved: &mut HashSet<VarId>,
) {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = try_instr
    else {
        return;
    };
    collect_region_defs(body, counts, order, moved);
    if let Some((binding, h)) = handler {
        if let Some(v) = binding {
            if !counts.contains_key(v) {
                order.push(*v);
            }
            *counts.entry(*v).or_insert(0) += 1;
        }
        collect_region_defs(h, counts, order, moved);
    }
    if let Some(e) = orelse {
        collect_region_defs(e, counts, order, moved);
    }
    if let Some(fb) = finalbody {
        collect_region_defs(fb, counts, order, moved);
    }
}

/// The variable a MIR instruction writes (a `DefVar`, or the iterator a
/// `GetIter` derives), if any.
pub(super) const fn var_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::DefVar { var, .. } | MirInstr::GetIter { dest: var, .. } => Some(*var),
        _ => None,
    }
}

/// The variable a MIR instruction declares (a `DeclareVar`), if any: a def
/// site for region locality, but no liveness def.
const fn var_declared(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::DeclareVar { var } => Some(*var),
        _ => None,
    }
}

/// `EstablishLoans` starts the reference's analytical live range, but it does not
/// overwrite the runtime handle already stored by `DefVar`.
pub(super) const fn loan_liveness_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::EstablishLoans { reference, .. } => Some(*reference),
        _ => var_def(i),
    }
}

/// The variable transferred out by this instruction (a `^` move), if any — such a
/// variable is *not* dropped here (its value has moved to a new owner).
pub(super) fn vars_moved(i: &MirInstr) -> Vec<VarId> {
    match i {
        MirInstr::UseVar { var, mode, .. } if mode.moves() => vec![*var],
        // A lowered explicit-destructor call already consumes the receiver
        // slot (residual fields destroyed, whole-value `__deinit__` skipped);
        // splicing an ordinary drop as well would run the destructor the
        // named destructor replaced.
        MirInstr::ConsumeVar { var } => vec![*var],
        // A declaration-time move capture transfers a whole root into the
        // closure owner. Projected captures leave a residual aggregate that
        // still needs its ordinary drop, so only whole-root moves suppress it.
        MirInstr::MakeClosure { captures, .. } => captures
            .iter()
            .filter(|capture| capture.mode == MirCaptureMode::Move && capture.place.is_whole_root())
            .map(|capture| capture.place.root)
            .collect(),
        // An owned `__iter__` consumes the iterable into the iterator.
        MirInstr::GetIter { .. } => i.consumed_iteration_source().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Whether executing this instruction can surface a caught
/// `RuntimeError::Raised`, transferring control to an enclosing handler.
/// Minimal allowlist of provably silent instructions; everything else — calls,
/// subscripts, `Raise`, nested `Try`, ... — counts as a potential raise.
/// Copy elision and field-drop placement use it; raising→silent could let a
/// handler observe a vacated slot, so the allowlist stays minimal.
/// (Non-`Raised` runtime errors abort the program, so drops are unobservable
/// on those paths.)
pub(super) const fn may_raise(instr: &MirInstr) -> bool {
    !matches!(
        instr,
        MirInstr::DefVar { .. }
            | MirInstr::StoreToMem { .. }
            | MirInstr::DeclareVar { .. }
            | MirInstr::UseVar { .. }
            | MirInstr::DropVar { .. }
            | MirInstr::ConsumeVar { .. }
            | MirInstr::KeepAlive { .. }
    )
}

/// Whether this instruction is a checked raise point
/// ([`MirInstr::is_raise_point`]) or a nested `try` whose regions may raise
/// outward. Drop elaboration seeds a handler's live set only here, as
/// upstream's CFG has raise edges only at raising operations.
pub(super) fn raises_to_handler(instr: &MirInstr) -> bool {
    matches!(instr, MirInstr::Try { .. }) || instr.is_raise_point()
}

/// Visit every instruction under `blocks`, recursing into `try` sub-regions.
pub(super) fn for_each_instr_deep(blocks: &[MirBlock], visit: &mut impl FnMut(&MirInstr)) {
    for block in blocks {
        for instr in &block.instrs {
            visit(instr);
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                for_each_instr_deep(body, visit);
                if let Some((_, h)) = handler {
                    for_each_instr_deep(h, visit);
                }
                if let Some(e) = orelse {
                    for_each_instr_deep(e, visit);
                }
                if let Some(fb) = finalbody {
                    for_each_instr_deep(fb, visit);
                }
            }
        }
    }
}

/// Mutable counterpart of [`for_each_instr_deep`].
pub(super) fn for_each_instr_deep_mut(
    blocks: &mut [MirBlock],
    visit: &mut impl FnMut(&mut MirInstr),
) {
    for block in blocks {
        for instr in &mut block.instrs {
            visit(instr);
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                for_each_instr_deep_mut(body, visit);
                if let Some((_, h)) = handler {
                    for_each_instr_deep_mut(h, visit);
                }
                if let Some(e) = orelse {
                    for_each_instr_deep_mut(e, visit);
                }
                if let Some(fb) = finalbody {
                    for_each_instr_deep_mut(fb, visit);
                }
            }
        }
    }
}

/// Whether the value in variable `v` is dropped by *this* function: locals
/// always; a consuming `var` or `deinit` parameter — receiver included — yes,
/// at its last use in the body (Mojo destroys an unused `deinit self`'s fields
/// at the callee's entry); a borrowed parameter or a non-parameter `self`
/// never (the caller owns a borrow; `self` is written back / would recurse).
pub(super) fn is_droppable_root(f: &MirFunction, v: VarId) -> bool {
    let vi = v as usize;
    if vi < f.n_params {
        // A consuming `var` parameter is destroyed here; a `deinit` parameter is
        // *consumed* here (its teardown is rewritten to `ConsumeVar` below).
        return f.owned_params.get(vi).copied().unwrap_or(false)
            || f.deinit_params.get(vi).copied().unwrap_or(false);
    }
    // A `self` outside the leading parameter range (generated/specialized
    // method CFGs whose stable binding slot is not leading) is caller-owned:
    // classifying such a receiver as a local makes drop elaboration
    // recursively destroy borrowed fields (and, for pointer-backed
    // collections, free the caller's allocation).
    f.var_names.get(vi).is_none_or(|name| name != "self")
}

/// Whether destroying a field of this type can do observable work (run a
/// destructor, release storage). Scalars, pointers, and references die
/// silently, so they need no instruction.
pub(super) fn field_needs_drop(ty: &mojito_types::types::Ty) -> bool {
    use mojito_types::types::TyKind;
    may_alias_owned_storage(ty) || matches!(ty.kind(), TyKind::Func(_) | TyKind::Error)
}

/// A set of small dense indices (variables or registers) as a bit vector,
/// growing to the largest member: the liveness sets the drop analyses update
/// at every instruction.
#[derive(Clone, Default)]
pub(super) struct DenseSet(Vec<u64>);

impl DenseSet {
    /// Add `index`; whether it was absent.
    pub(super) fn insert(&mut self, index: u32) -> bool {
        let (word, bit) = Self::position(index);
        if word >= self.0.len() {
            self.0.resize(word + 1, 0);
        }
        let absent = self.0[word] & bit == 0;
        self.0[word] |= bit;
        absent
    }

    /// Remove `index`; whether it was present.
    pub(super) fn remove(&mut self, index: u32) -> bool {
        let (word, bit) = Self::position(index);
        self.0.get_mut(word).is_some_and(|word| {
            let present = *word & bit != 0;
            *word &= !bit;
            present
        })
    }

    pub(super) fn contains(&self, index: u32) -> bool {
        let (word, bit) = Self::position(index);
        self.0.get(word).is_some_and(|word| word & bit != 0)
    }

    /// Add every member of `other`.
    pub(super) fn union(&mut self, other: &Self) {
        if other.0.len() > self.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (word, other) in self.0.iter_mut().zip(&other.0) {
            *word |= other;
        }
    }

    /// The members in increasing order.
    pub(super) fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().enumerate().flat_map(|(word, bits)| {
            let base = word as u32 * 64;
            (0..64)
                .filter(move |bit| bits & (1 << bit) != 0)
                .map(move |bit| base + bit)
        })
    }

    const fn position(index: u32) -> (usize, u64) {
        ((index / 64) as usize, 1 << (index % 64))
    }
}

impl PartialEq for DenseSet {
    /// Equal members, whatever trailing empty words either holds.
    fn eq(&self, other: &Self) -> bool {
        let (short, long) = if self.0.len() <= other.0.len() {
            (&self.0, &other.0)
        } else {
            (&other.0, &self.0)
        };
        short == &long[..short.len()] && long[short.len()..].iter().all(|word| *word == 0)
    }
}

impl Eq for DenseSet {}

impl FromIterator<u32> for DenseSet {
    fn from_iter<I: IntoIterator<Item = u32>>(members: I) -> Self {
        let mut set = Self::default();
        set.extend(members);
        set
    }
}

impl Extend<u32> for DenseSet {
    fn extend<I: IntoIterator<Item = u32>>(&mut self, members: I) {
        for member in members {
            self.insert(member);
        }
    }
}
