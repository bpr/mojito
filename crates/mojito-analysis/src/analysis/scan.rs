//! Shared MIR scans: per-instruction variable uses/defs/moves, deep
//! instruction walks, and droppable-root classification.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{instruction_regions, instruction_regions_mut};

/// The variables a MIR instruction reads, each paired with a nearby result
/// register (for a diagnostic span). Covers direct reads (`UseVar`), place roots
/// (`Store`/`LoadPlace`/a `mut self` receiver — so a write *through* a moved value
/// is caught too), and the `for` iterator variable.
pub(super) fn var_uses(i: &MirInstr) -> Vec<(VarId, Reg)> {
    instruction_uses(i)
        .into_iter()
        .flat_map(|(used, reg)| match used {
            Used::Var(var) => vec![(var, reg)],
            Used::Place(place) => place_loan_uses(place, reg),
        })
        .collect()
}

/// One read of an instruction: a variable, or a place, which reads its root
/// and the reference it is reached through.
pub(super) enum Used<'a> {
    Var(VarId),
    Place(&'a MirPlace),
}

/// What a MIR instruction reads ([`var_uses`]), each place whole, each
/// paired with a nearby result register.
pub(super) fn instruction_uses(i: &MirInstr) -> Vec<(Used<'_>, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .map(|loan| (Used::Place(&loan.place), *marker))
            .collect(),
        MirInstr::MakeRef { dest, place } => vec![(Used::Place(place), *dest)],
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .map(|capture| (Used::Place(&capture.place), *dest))
            .collect(),
        MirInstr::UseVar { dest, var, .. } => vec![(Used::Var(*var), *dest)],
        MirInstr::KeepAlive { var } => vec![(Used::Var(*var), Reg(0))],
        MirInstr::MovePlace { dest, place } => vec![(Used::Place(place), *dest)],
        MirInstr::Store { place, src } => vec![(Used::Place(place), *src)],
        MirInstr::StoreRef { place, reference } => vec![(Used::Place(place), *reference)],
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
                .map(|place| (Used::Place(place), *value))
                .collect::<Vec<_>>();
            uses.extend(
                call.capture_accesses
                    .iter()
                    .map(|access| (Used::Var(access.root), *value)),
            );
            uses
        }
        MirInstr::VariantSet { place, value, .. } => vec![(Used::Place(place), *value)],
        MirInstr::VariantSetInitWith { place, factory, .. } => vec![(Used::Place(place), *factory)],
        MirInstr::VariantReplace { place, value, .. } => vec![(Used::Place(place), *value)],
        MirInstr::LoadPlace { dest, place } => vec![(Used::Place(place), *dest)],
        MirInstr::DropPlace { place, .. }
        | MirInstr::MarkInitialized { place }
        | MirInstr::MarkDestroyed { place } => vec![(Used::Place(place), Reg(0))],
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
                .map(|place| (Used::Place(place), *dest))
                .collect::<Vec<_>>();
            uses.extend(
                capture_accesses
                    .iter()
                    .map(|access| (Used::Var(access.root), *dest)),
            );
            uses
        }
        MirInstr::ConstructTypeParam {
            dest, kwarg_places, ..
        } => kwarg_places
            .iter()
            .flatten()
            .map(|place| (Used::Place(place), *dest))
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
                .map(|place| (Used::Place(place), *dest))
                .collect::<Vec<_>>();
            uses.extend(
                capture_accesses
                    .iter()
                    .map(|access| (Used::Var(access.root), *dest)),
            );
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
                .map(|place| (Used::Place(place), *dest))
                .collect::<Vec<_>>();
            uses.extend(
                capture_accesses
                    .iter()
                    .map(|access| (Used::Var(access.root), *dest)),
            );
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
                .map(|place| (Used::Place(place), *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (Used::Var(access.root), *dest)),
                );
            }
            uses
        }
        MirInstr::MultiIndex {
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
                .map(|place| (Used::Place(place), *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (Used::Var(access.root), *dest)),
                );
            }
            uses
        }
        MirInstr::HasNext { dest, iter, .. } | MirInstr::Next { dest, iter, .. } => {
            vec![(Used::Var(*iter), *dest)]
        }
        // `GetIter` normalizes its source into the iterator; count the source as a
        // use so a borrowed named source (a reference bound before the loop) stays
        // live through the read and is not dropped before the iterator is derived.
        MirInstr::GetIter { source, .. } => vec![(Used::Var(*source), Reg(0))],
        // A `try` reads every variable its sub-regions read, at any depth:
        // the outer liveness must treat it as one big use, so a value used
        // only inside the `try` is not dropped *before* it.
        MirInstr::Try { .. } => {
            let mut uses = Vec::new();
            for region in instruction_regions(i) {
                for_each_instr_deep(region, &mut |instr| {
                    if !matches!(instr, MirInstr::Try { .. }) {
                        uses.extend(
                            var_uses(instr)
                                .into_iter()
                                .map(|(var, reg)| (Used::Var(var), reg)),
                        );
                    }
                });
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

/// The variable a MIR instruction writes (a `DefVar`, or the iterator a
/// `GetIter` derives), if any.
pub(super) const fn var_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::DefVar { var, .. } | MirInstr::GetIter { dest: var, .. } => Some(*var),
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
        // A `deinit` parameter's or pack's end before the exit.
        MirInstr::ConsumeVar { var } => vec![*var],
        // A `deinit`-receiver call consumes its receiver slot (its fields
        // destroyed by the callee, whole-value `__deinit__` skipped), as
        // upstream's call consumes its `DeinitMem` operand; splicing an
        // ordinary drop as well would run the destructor the named
        // destructor replaced.
        MirInstr::MethodCall { .. } => i.consumed_receiver_slot().into_iter().collect(),
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
        // A whole slot marked destroyed has handed its value on through a
        // pointer (an owned `VariadicList`'s element).
        MirInstr::MarkDestroyed { place } if place.is_whole_root() => vec![place.root],
        _ => Vec::new(),
    }
}

/// Whether this instruction is a nested `try`, whose regions may raise
/// outward: a walk that does not enter it seeds the raise channel here. A
/// raising call raises through its error region's `raise` terminator.
pub(super) const fn raises_to_handler(instr: &MirInstr) -> bool {
    matches!(instr, MirInstr::Try { .. })
}

/// The values a block's terminator hands on: those an arm yields, and the
/// error a `raise` raises.
pub(super) fn handed_values(term: &MirTerm) -> &[Reg] {
    match term {
        MirTerm::Yield(values) | MirTerm::ComptimeYield(values) => values,
        MirTerm::Raise { src, .. } => std::slice::from_ref(src),
        _ => &[],
    }
}

/// Visit every instruction under `blocks`, recursing into every region op's
/// regions.
pub(super) fn for_each_instr_deep(blocks: &[MirBlock], visit: &mut impl FnMut(&MirInstr)) {
    for block in blocks {
        for instr in &block.instrs {
            visit(instr);
            for region in instruction_regions(instr) {
                for_each_instr_deep(region, visit);
            }
        }
    }
}

/// Visit every instruction under `blocks` outside any `try`: a `try` is
/// visited, its regions are not.
pub(super) fn for_each_instr_outside_try(blocks: &[MirBlock], visit: &mut impl FnMut(&MirInstr)) {
    for block in blocks {
        for instr in &block.instrs {
            visit(instr);
            if !matches!(instr, MirInstr::Try { .. }) {
                for region in instruction_regions(instr) {
                    for_each_instr_outside_try(region, visit);
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
            for region in instruction_regions_mut(instr) {
                for_each_instr_deep_mut(region, visit);
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
    may_alias_owned_storage(ty) || matches!(ty.kind(), TyKind::Func(_))
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
