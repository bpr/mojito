//! `stack-reuse`: upstream's `StackReuse` (`lib/Transforms/StackReuse.cpp`),
//! which the function pipeline `AutomaticInline` nests runs after its second
//! `Mem2Reg`.
//!
//! A MIR variable slot is the stack allocation. A local slot is tracked
//! when no mention of it lets it escape (upstream's `runAnalysis`): every
//! mention is a load (a forwarding `var.use`, a copying or moving one of a
//! type whose copy is the identity, or a `place.load` of the slot or of an
//! owned part of it), a store (a `var.store`, or a `place.store` of the
//! slot or of an owned part), or a lifetime marker (`var.declare`, and a
//! `drop.var` or `var.consume` of a type whose copy is the identity, which
//! runs no code). A borrow, a `ref.make` (R2074), a projection through a
//! pointer or reference, a constructor's run, a destructor's, or a
//! forwarding `var.use` of a memory-only type whose register is not only
//! stored, which hands the slot's storage to its consumer as upstream
//! passes an `owned_in_mem` argument's address, is an escape, as
//! upstream's any other user is.
//!
//! The scan (`scanRegion`) walks the body in order, tracking each tracked
//! slot's potential value: the register a whole store stored, or, when that
//! register is a whole load's, the loaded slot's value then, which sees
//! through a copy; a store to a projection, a lifetime marker, and a store
//! of a register defined more than once give the slot a fresh opaque value.
//! At each load, the slot's value is held by a set of slots; when the most
//! dominating of them is another slot, the load could read that one
//! instead, and a slot every load of which could is elided, unless another
//! elided slot reads it. Entering and leaving each region of a `loop`,
//! `if`, `match`, or `try`, every slot written in it (outside its own
//! allocation region) takes a fresh opaque value.
//!
//! A slot's allocation is the point, in the innermost region holding every
//! mention of it, just before the instruction holding its first mention;
//! one slot dominates another when its allocation region holds the other's
//! and its allocation comes first.
//!
//! The rewrite (`processRegion`) points each load of an elided slot at the
//! slot it reads instead and erases the elided slot's stores and markers.
//!
//! MIR's moving `var.use` ends its slot, where upstream's `pop.load` leaves
//! memory as it was: a moving load of a slot that stays gives that slot a
//! fresh opaque value, a redirected moving load becomes a `place.load` of
//! the slot it reads, which leaves it, and a moving load whose register
//! only a whole store into an elided slot reads (a relocation) is erased
//! with that store, so the slot it read keeps its value. A relocation into
//! a slot that stays keeps its moving load, which ends the slot it read; the
//! scan runs again with it until no relocation it read through stays.
//!
//! Upstream's `optimizeReadOnlyMemory`, which promotes a slot stored once
//! from a large `pop.array` constant to a global constant, is not ported
//! (R2073).

use super::canonicalize::definition_counts;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, MirPlace, Reg, UseMode, count_register_reads,
    instruction_regions, instruction_regions_mut, instruction_slots, retire_slots,
};
use mojito_types::types::{Ty, TyKind};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// The pass, under upstream's flag spelling.
pub const STACK_REUSE: BodyPass = BodyPass {
    name: "stack-reuse",
    run: stack_reuse,
};

/// Elide each local slot of `function` every load of which can read another
/// slot holding the same value; returns whether any was.
pub fn stack_reuse(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let analysis = Analysis::of(function, declarations);
    if analysis.slots.is_empty() {
        return false;
    }
    let mut kept: HashSet<usize> = HashSet::new();
    let outcome = loop {
        let outcome = Scan::run(&function.blocks, &analysis, &kept);
        let staying: Vec<usize> = analysis
            .relocations
            .iter()
            .filter(|&(id, target)| !kept.contains(id) && !outcome.elided.contains(target))
            .map(|(&id, _)| id)
            .collect();
        if staying.is_empty() {
            break outcome;
        }
        kept.extend(staying);
    };
    if outcome.elided.is_empty() {
        return false;
    }
    let mut rewrite = Rewrite {
        analysis: &analysis,
        outcome: &outcome,
        kept: &kept,
        var_tys: &function.var_tys,
        next: 0,
        erased_results: HashSet::new(),
        erased: 0,
    };
    let mut blocks = std::mem::take(&mut function.blocks);
    rewrite.blocks(&mut blocks);
    let Rewrite {
        erased_results,
        erased,
        ..
    } = rewrite;
    function.blocks = blocks;
    function
        .reg_types
        .retain(|reg, _| !erased_results.contains(reg));
    function
        .spans
        .0
        .retain(|reg, _| !erased_results.contains(reg));
    retire_slots(function, &outcome.elided);
    timing::count("elided_stack_slots", outcome.elided.len() as u64);
    timing::count("erased_stack_slot_instructions", erased as u64);
    true
}

/// How an instruction mentions the one slot it names, when that mention
/// does not let the slot escape.
#[derive(Clone, Copy)]
enum Access {
    /// A load into `dest`, of the whole slot or of a projection of it;
    /// `moving` when it ends the slot.
    Load {
        dest: Reg,
        whole: bool,
        moving: bool,
    },
    /// A store of `src`, to the whole slot or to a projection of it.
    Store { src: Reg, whole: bool },
    /// A lifetime marker, after which the slot holds nothing.
    Marker,
}

/// What [`Analysis::of`] finds: the tracked slots, where each is allocated,
/// the slots variant across each region op, and the relocations.
///
/// Instructions are numbered in preorder, each before the regions it
/// holds, as every walk of this pass visits them.
struct Analysis {
    /// Each tracked slot and its allocation.
    slots: BTreeMap<VarId, Allocation>,
    /// The tracked slots stored, marked, or moved from inside each region
    /// op, by its number, outside their own allocation region.
    variants: HashMap<usize, Vec<VarId>>,
    /// Each region, by its preorder number from the function's body, 0: the
    /// region holding the op that holds it, and that op's number.
    regions: Vec<Option<(usize, usize)>>,
    /// The tracked slot each instruction mentions, and how.
    accesses: HashMap<usize, (VarId, Access)>,
    /// Each relocation, by number: a moving whole load whose register, defined
    /// once, only a whole store into another tracked slot reads; the slot it
    /// stores into.
    relocations: HashMap<usize, VarId>,
    /// How many times the function defines each register.
    definitions: Vec<u32>,
    var_tys: HashMap<VarId, Ty>,
}

/// Where a tracked slot is allocated.
#[derive(Clone, Copy)]
struct Allocation {
    /// The innermost region holding every mention of the slot.
    region: usize,
    /// The number of the instruction of `region` holding its first mention,
    /// before which it is allocated.
    at: usize,
    /// The number of its first mention.
    first: usize,
}

impl Analysis {
    fn of(function: &mut MirFunction, declarations: &MirDeclarations) -> Self {
        let definitions = definition_counts(&mut function.blocks, function.n_regs);
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let mut stores = vec![0_u32; function.n_regs as usize];
        count_whole_stores(&function.blocks, &mut stores);
        let stored_once = (0..function.n_regs)
            .filter(|&reg| {
                let at = reg as usize;
                stores[at] == 1 && reads[at] == 1
            })
            .collect();
        let mut census = Census {
            function,
            declarations,
            stored_once,
            next: 0,
            regions: Vec::new(),
            mentions: HashMap::new(),
            escaped: HashSet::new(),
        };
        census.region(&function.blocks, None);
        let Census {
            regions,
            mentions,
            escaped,
            ..
        } = census;
        let tracked = |var: &VarId| {
            (*var as usize) >= function.n_params
                && function.error_slot != Some(*var)
                && !escaped.contains(var)
                && function
                    .var_tys
                    .get(var)
                    .is_some_and(|ty| !matches!(ty.kind(), TyKind::Ref(_) | TyKind::LitRef { .. }))
        };
        let mut slots = BTreeMap::new();
        let mut variants: HashMap<usize, Vec<VarId>> = HashMap::new();
        let mut accesses = HashMap::new();
        let mut stored: HashMap<u32, Vec<VarId>> = HashMap::new();
        let mentions: BTreeMap<VarId, Vec<Mention>> = mentions
            .into_iter()
            .filter(|(var, _)| tracked(var))
            .collect();
        for (&var, list) in &mentions {
            let region = innermost_common(&regions, list.iter().map(|mention| mention.region));
            let first = &list[0];
            let at = crossings(&regions, first.region, region)
                .last()
                .map_or(first.id, |&(_, op)| op);
            slots.insert(
                var,
                Allocation {
                    region,
                    at,
                    first: first.id,
                },
            );
            let mut crossed = BTreeSet::new();
            for mention in list {
                accesses.insert(mention.id, (var, mention.access));
                if let Access::Store { src, whole: true } = mention.access {
                    stored.entry(src.0).or_default().push(var);
                }
                if matches!(mention.access, Access::Load { moving: false, .. }) {
                    continue;
                }
                crossed.extend(
                    crossings(&regions, mention.region, region)
                        .into_iter()
                        .map(|(_, op)| op),
                );
            }
            for op in crossed {
                variants.entry(op).or_default().push(var);
            }
        }
        let relocations = accesses
            .iter()
            .filter_map(|(&id, &(var, access))| {
                let Access::Load {
                    dest,
                    whole: true,
                    moving: true,
                } = access
                else {
                    return None;
                };
                let at = dest.0 as usize;
                if definitions.get(at) != Some(&1) || reads.get(at) != Some(&1) {
                    return None;
                }
                match stored.get(&dest.0).map(Vec::as_slice) {
                    Some(&[target]) if target != var => Some((id, target)),
                    _ => None,
                }
            })
            .collect();
        Self {
            slots,
            variants,
            regions,
            accesses,
            relocations,
            definitions,
            var_tys: function.var_tys.clone(),
        }
    }

    /// Whether the tracked slot `a` properly dominates the tracked slot `b`:
    /// `a`'s allocation region holds `b`'s and `a` is allocated first.
    fn dominates(&self, a: VarId, b: VarId) -> bool {
        let (first, second) = (&self.slots[&a], &self.slots[&b]);
        let mut region = Some(second.region);
        while let Some(at) = region {
            if at == first.region {
                return (first.at, first.first, a) < (second.at, second.first, b);
            }
            region = self.regions[at].map(|(holder, _)| holder);
        }
        false
    }

    /// Whether the register `reg` has one definition.
    fn defined_once(&self, reg: Reg) -> bool {
        self.definitions.get(reg.0 as usize) == Some(&1)
    }
}

/// One mention of a slot, outside an escape.
struct Mention {
    id: usize,
    region: usize,
    access: Access,
}

/// The walk [`Analysis::of`] takes a census of the slots by.
struct Census<'f> {
    function: &'f MirFunction,
    declarations: &'f MirDeclarations,
    /// The registers read once, by a whole store.
    stored_once: HashSet<u32>,
    next: usize,
    regions: Vec<Option<(usize, usize)>>,
    /// Each slot's mentions outside an escape, in order.
    mentions: HashMap<VarId, Vec<Mention>>,
    escaped: HashSet<VarId>,
}

impl Census<'_> {
    fn region(&mut self, blocks: &[MirBlock], parent: Option<(usize, usize)>) {
        let region = self.regions.len();
        self.regions.push(parent);
        for instruction in blocks.iter().flat_map(|block| &block.instrs) {
            let id = self.next;
            self.next += 1;
            let slots = instruction_slots(instruction);
            match access(instruction, self.function, self.declarations) {
                Some((var, access))
                    if slots.iter().all(|&slot| slot == var) && !self.hands_over(instruction) =>
                {
                    self.mentions
                        .entry(var)
                        .or_default()
                        .push(Mention { id, region, access });
                }
                _ => self.escaped.extend(slots),
            }
            for held in instruction_regions(instruction) {
                self.region(held, Some((region, id)));
            }
        }
    }
}

impl Census<'_> {
    /// Whether `instruction` hands its slot's storage to the register's
    /// consumer, as upstream passes an `owned_in_mem` argument's address: a
    /// forwarding `var.use` of a memory-only type whose register is not
    /// only stored, which moves the bytes as a load and a store would.
    fn hands_over(&self, instruction: &MirInstr) -> bool {
        let MirInstr::UseVar {
            dest,
            var,
            mode: UseMode::Forward,
            ..
        } = instruction
        else {
            return false;
        };
        !self.stored_once.contains(&dest.0)
            && self
                .function
                .var_tys
                .get(var)
                .is_none_or(|ty| !self.declarations.register_passable(ty))
    }
}

/// What one scan finds: the elided slots, and the slot each load of one
/// reads instead, by the load's number.
struct Outcome {
    elided: BTreeSet<VarId>,
    reuses: HashMap<usize, VarId>,
}

/// A slot's potential value: a register stored into it, or an opaque value.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Potential {
    Reg(u32),
    Opaque(u32),
}

/// Upstream's `scanRegion`, over the body with the relocations `kept`
/// keeping their moving loads.
struct Scan<'a> {
    analysis: &'a Analysis,
    kept: &'a HashSet<usize>,
    next: usize,
    values: HashMap<VarId, Potential>,
    holders: HashMap<Potential, BTreeSet<VarId>>,
    /// The value each whole load's register read, by register.
    loaded: HashMap<u32, Potential>,
    opaque: u32,
    /// Each slot that may be elided, and the slots its loads would read.
    elidable: BTreeMap<VarId, BTreeSet<VarId>>,
    reuses: HashMap<usize, VarId>,
}

impl Scan<'_> {
    fn run(blocks: &[MirBlock], analysis: &Analysis, kept: &HashSet<usize>) -> Outcome {
        let mut scan = Scan {
            analysis,
            kept,
            next: 0,
            values: HashMap::new(),
            holders: HashMap::new(),
            loaded: HashMap::new(),
            opaque: 0,
            elidable: analysis
                .slots
                .keys()
                .map(|&var| (var, BTreeSet::new()))
                .collect(),
            reuses: HashMap::new(),
        };
        for &var in analysis.slots.keys() {
            scan.fresh(var);
        }
        scan.region(blocks);
        let Scan {
            mut elidable,
            mut reuses,
            ..
        } = scan;
        // Only a slot no elided slot reads is elided; as each reads the most
        // dominating holder, none read is itself elidable by another.
        let read: BTreeSet<VarId> = elidable.values().flatten().copied().collect();
        elidable.retain(|var, _| !read.contains(var));
        reuses.retain(|id, _| {
            analysis
                .accesses
                .get(id)
                .is_some_and(|(var, _)| elidable.contains_key(var))
        });
        Outcome {
            elided: elidable.into_keys().collect(),
            reuses,
        }
    }

    fn region(&mut self, blocks: &[MirBlock]) {
        for instruction in blocks.iter().flat_map(|block| &block.instrs) {
            let id = self.next;
            self.next += 1;
            if let Some(&(var, access)) = self.analysis.accesses.get(&id) {
                self.access(id, var, access);
            }
            let held = instruction_regions(instruction);
            if held.is_empty() {
                continue;
            }
            let variant = self.analysis.variants.get(&id).cloned().unwrap_or_default();
            for region in held {
                for &var in &variant {
                    self.fresh(var);
                }
                self.region(region);
            }
            for &var in &variant {
                self.fresh(var);
            }
        }
    }

    fn access(&mut self, id: usize, var: VarId, access: Access) {
        match access {
            Access::Load {
                dest,
                whole,
                moving,
            } => {
                let value = self.values[&var];
                let reuse = self.most_dominating(var, value);
                if reuse == var {
                    self.elidable.remove(&var);
                } else if let Some(read) = self.elidable.get_mut(&var) {
                    read.insert(reuse);
                    self.reuses.insert(id, reuse);
                }
                if whole {
                    self.loaded.insert(dest.0, value);
                }
                let relocates =
                    self.analysis.relocations.contains_key(&id) && !self.kept.contains(&id);
                if moving && !relocates {
                    self.fresh(var);
                }
            }
            Access::Store { src, whole } => {
                let value = if whole && self.analysis.defined_once(src) {
                    self.loaded
                        .get(&src.0)
                        .copied()
                        .unwrap_or(Potential::Reg(src.0))
                } else {
                    self.next_opaque()
                };
                self.set(var, value);
            }
            Access::Marker => self.fresh(var),
        }
    }

    /// The slot holding `value` that most dominates `var`, which holds it,
    /// of `var`'s type: `var` itself when none does.
    fn most_dominating(&self, var: VarId, value: Potential) -> VarId {
        let ty = self.analysis.var_tys.get(&var);
        self.holders
            .get(&value)
            .into_iter()
            .flatten()
            .filter(|&&other| self.analysis.var_tys.get(&other) == ty)
            .fold(var, |best, &other| {
                if self.analysis.dominates(other, best) {
                    other
                } else {
                    best
                }
            })
    }

    fn fresh(&mut self, var: VarId) {
        let value = self.next_opaque();
        self.set(var, value);
    }

    const fn next_opaque(&mut self) -> Potential {
        self.opaque += 1;
        Potential::Opaque(self.opaque)
    }

    fn set(&mut self, var: VarId, value: Potential) {
        if let Some(old) = self.values.insert(var, value)
            && let Some(holders) = self.holders.get_mut(&old)
        {
            holders.remove(&var);
        }
        self.holders.entry(value).or_default().insert(var);
    }
}

/// Upstream's `processRegion` and its erasure of the elided allocations.
struct Rewrite<'a> {
    analysis: &'a Analysis,
    outcome: &'a Outcome,
    kept: &'a HashSet<usize>,
    var_tys: &'a HashMap<VarId, Ty>,
    next: usize,
    erased_results: HashSet<u32>,
    erased: usize,
}

impl Rewrite<'_> {
    fn blocks(&mut self, blocks: &mut [MirBlock]) {
        for block in blocks {
            let mut out = Vec::with_capacity(block.instrs.len());
            for mut instruction in std::mem::take(&mut block.instrs) {
                let id = self.next;
                self.next += 1;
                if self.erases(id) {
                    let mut results = Vec::new();
                    instruction_result_regs(&instruction, &mut results);
                    self.erased_results.extend(results.iter().map(|reg| reg.0));
                    self.erased += 1;
                    continue;
                }
                if let Some(&reuse) = self.outcome.reuses.get(&id) {
                    self.redirect(&mut instruction, reuse);
                }
                for region in instruction_regions_mut(&mut instruction) {
                    self.blocks(region);
                }
                out.push(instruction);
            }
            block.instrs = out;
        }
    }

    /// Whether the instruction numbered `id` goes: a store or marker of an
    /// elided slot, or a relocation into one whose moving load is not kept.
    fn erases(&self, id: usize) -> bool {
        if let Some(target) = self.analysis.relocations.get(&id)
            && !self.kept.contains(&id)
            && self.outcome.elided.contains(target)
        {
            return true;
        }
        self.analysis
            .accesses
            .get(&id)
            .is_some_and(|(var, access)| {
                !matches!(access, Access::Load { .. }) && self.outcome.elided.contains(var)
            })
    }

    /// Point the load `instruction` at `reuse`; a moving whole load becomes
    /// a `place.load`, which leaves the slot it reads.
    fn redirect(&self, instruction: &mut MirInstr, reuse: VarId) {
        match instruction {
            MirInstr::UseVar {
                dest, var, mode, ..
            } if mode.moves() => {
                let ty = self.var_tys.get(var).cloned();
                *instruction = MirInstr::LoadPlace {
                    dest: *dest,
                    place: MirPlace::root(reuse, ty),
                };
            }
            MirInstr::UseVar { var, .. } => *var = reuse,
            MirInstr::LoadPlace { place, .. } => place.root = reuse,
            _ => {}
        }
    }
}

/// The slot `instruction` names as a load, store, or lifetime marker that
/// does not let it escape, and how.
fn access(
    instruction: &MirInstr,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Option<(VarId, Access)> {
    // A projection is a view of the slot when it names an owned part of
    // it, as upstream's `kgen.struct.gep` does; one leading from a pointer
    // reads the pointer and addresses its pointee.
    let view = |place: &MirPlace| {
        place.owned_path().is_some()
            && function.var_tys.get(&place.root).is_some_and(|ty| {
                !matches!(ty.kind(), TyKind::Pointer { .. } | TyKind::KgenPointer(_))
            })
    };
    let slot_typed = |var: VarId, reg: Reg| {
        function.var_tys.contains_key(&var)
            && function.reg_types.get(&reg.0) == function.var_tys.get(&var)
    };
    match instruction {
        MirInstr::UseVar {
            dest,
            var,
            mode,
            constructor: None,
        } if slot_typed(*var, *dest)
            && match mode {
                UseMode::Forward => true,
                UseMode::Copy | UseMode::Move => function
                    .var_tys
                    .get(var)
                    .is_some_and(|ty| declarations.copy_is_identity(ty)),
                UseMode::BorrowShared | UseMode::BorrowMut => false,
            } =>
        {
            Some((
                *var,
                Access::Load {
                    dest: *dest,
                    whole: true,
                    moving: mode.moves(),
                },
            ))
        }
        MirInstr::LoadPlace { dest, place } if place.through.is_none() => {
            let whole = place.proj.is_empty();
            (if whole {
                slot_typed(place.root, *dest)
            } else {
                view(place)
            })
            .then_some((
                place.root,
                Access::Load {
                    dest: *dest,
                    whole,
                    moving: false,
                },
            ))
        }
        MirInstr::DefVar { var, src, .. } => slot_typed(*var, *src).then_some((
            *var,
            Access::Store {
                src: *src,
                whole: true,
            },
        )),
        MirInstr::Store { place, src } if place.through.is_none() => {
            let whole = place.proj.is_empty();
            (if whole {
                slot_typed(place.root, *src)
            } else {
                view(place)
            })
            .then_some((place.root, Access::Store { src: *src, whole }))
        }
        MirInstr::DeclareVar { var } => Some((*var, Access::Marker)),
        MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var }
            if function
                .var_tys
                .get(var)
                .is_some_and(|ty| declarations.copy_is_identity(ty)) =>
        {
            Some((*var, Access::Marker))
        }
        _ => None,
    }
}

/// Count in `stores` each register's reads as the value of a whole store
/// (`var.store`, or a `place.store` of a whole slot) in `blocks`, their
/// regions included.
fn count_whole_stores(blocks: &[MirBlock], stores: &mut [u32]) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        for region in instruction_regions(instruction) {
            count_whole_stores(region, stores);
        }
        let src = match instruction {
            MirInstr::DefVar { src, .. } => src,
            MirInstr::Store { place, src } if place.proj.is_empty() && place.through.is_none() => {
                src
            }
            _ => continue,
        };
        if let Some(count) = stores.get_mut(src.0 as usize) {
            *count += 1;
        }
    }
}

/// The region ops crossed climbing from the region `from` to its enclosing
/// region `to`: each as the region holding it and its number, innermost
/// first.
fn crossings(regions: &[Option<(usize, usize)>], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut crossed = Vec::new();
    let mut at = from;
    while at != to
        && let Some((holder, op)) = regions[at]
    {
        crossed.push((holder, op));
        at = holder;
    }
    crossed
}

/// The innermost region holding each of `lists`, by the parents `regions`
/// records.
fn innermost_common(
    regions: &[Option<(usize, usize)>],
    mut lists: impl Iterator<Item = usize>,
) -> usize {
    let Some(first) = lists.next() else {
        return 0;
    };
    let mut chain = vec![first];
    while let Some((holder, _)) = regions[chain[chain.len() - 1]] {
        chain.push(holder);
    }
    for list in lists {
        let mut at = list;
        loop {
            if let Some(found) = chain.iter().position(|&region| region == at) {
                chain.drain(..found);
                break;
            }
            at = regions[at].map_or(0, |(holder, _)| holder);
        }
    }
    chain[0]
}
