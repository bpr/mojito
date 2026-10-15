//! `mem-2-reg`: upstream's `Mem2Reg`, which promotes a stack allocation to
//! SSA values when every user is a load, a store, or a lifetime marker, and
//! `lower-arg-conventions`, the callee half of upstream's
//! `LowerArgConventions`, which stores each register-passable argument into
//! a temporary that `Mem2Reg` then promotes.
//!
//! A MIR variable slot is the allocation. One is promoted when its type is
//! trivially register-passable at a concrete size
//! ([`MirDeclarations::copy_is_identity`], never symbolic), and every
//! mention of it, in one block list, is a store (`var.store`), a read
//! (`var.use`, or a whole-root `place.load`), or a mention with no effect
//! on such a type once drops are elaborated (`var.declare`, `drop.var`,
//! `var.consume`, and an `interiors.invalidate` whose marker nothing reads).
//! A destructor or copy constructor would take the slot's address, so a
//! type with either is never one, as upstream's never is; a borrow, a
//! capture, a loan, or a projected place keeps the slot too.
//!
//! Upstream walks structured `hlcf` regions and carries an allocation
//! stored inside one across it as region arguments and results. A MIR body
//! is a CFG at this seam, and its blocks take no arguments, so only a slot
//! each of whose reads sees one reaching store is promoted: the read becomes
//! a `value.copy` of the stored register, which the `canonicalize` after it
//! forwards. That store lies on every path to the read, and its register's
//! one definition lies before it in its block, so the register holds the
//! stored value at the read. A slot whose reads merge two stores keeps its
//! slot.
//!
//! The elaborator copies a `comptime for` body once per iteration with
//! every register renumbered, so in a generator no promoted read may sit on
//! the other side of a loop body's boundary from its store.

use super::canonicalize::{count_reads, definition_counts};
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg, UseMode,
    block_successors, comptime_for_body, has_structured_control_flow, instruction_regions,
    instruction_regions_mut, instruction_slots, region_escape_targets, renumber_block_slots,
    retire_slots, terminator_targets,
};
use mojito_types::types::{Ty, is_symbolic};
use std::collections::{BTreeSet, HashMap, HashSet};

/// The pass, under upstream's flag spelling.
pub const MEM_2_REG: BodyPass = BodyPass {
    name: "mem-2-reg",
    run: mem_2_reg,
};

/// The callee half of upstream's `LowerArgConventions`, which the
/// post-elaboration pipeline runs before [`MEM_2_REG`].
pub const LOWER_ARG_CONVENTIONS: BodyPass = BodyPass {
    name: "lower-arg-conventions",
    run: lower_arg_conventions,
};

/// Promote each local slot of `function` whose reads each see one store to
/// registers, and remove it from the function; returns whether any was.
pub fn mem_2_reg(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) -> bool {
    if has_structured_control_flow(function) {
        return false;
    }
    let promotion = promotable(function, declarations, &BTreeSet::new());
    if promotion.slots.is_empty() {
        return false;
    }
    let mut erased_results = HashSet::new();
    let mut lists = 0;
    let erased = rewrite_list(
        &mut function.blocks,
        &mut lists,
        &promotion,
        &mut erased_results,
    );
    function
        .reg_types
        .retain(|reg, _| !erased_results.contains(reg));
    function
        .spans
        .0
        .retain(|reg, _| !erased_results.contains(reg));
    for (_, origin) in function.spans.0.values_mut() {
        if origin.is_some_and(|var| promotion.slots.contains(&var)) {
            *origin = None;
        }
    }
    retire_slots(function, &promotion.slots);
    timing::count("promoted_slots", promotion.slots.len() as u64);
    timing::count("erased_slot_instructions", erased as u64);
    true
}

/// Store each argument of `function` that [`mem_2_reg`] would promote into
/// a fresh local at the entry, which every mention of the argument then
/// names; returns whether any was.
///
/// The argument is a `read` or owned parameter of a trivially
/// register-passable type, as upstream promotes one; a `mut` or `deinit`
/// one is never stored. Its own slot, the call ABI's, keeps the one read at
/// the entry. A body whose entry is a jump's target is left alone: the
/// store would run again there.
pub fn lower_arg_conventions(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    if has_structured_control_flow(function)
        || function
            .blocks
            .iter()
            .any(|block| block_successors(block).contains(&0))
    {
        return false;
    }
    let eligible: BTreeSet<VarId> = (0..function.n_params)
        .filter(|&param| {
            !function.ref_params.get(param).copied().unwrap_or(true)
                && !function.deinit_params.get(param).copied().unwrap_or(true)
                && function
                    .param_types
                    .get(param)
                    .is_some_and(|ty| promotable_type(ty, declarations))
        })
        .filter_map(|param| VarId::try_from(param).ok())
        .collect();
    if eligible.is_empty() {
        return false;
    }
    let stored: Vec<VarId> = promotable(function, declarations, &eligible)
        .slots
        .intersection(&eligible)
        .copied()
        .collect();
    if stored.is_empty() {
        return false;
    }
    let mut entry = Vec::with_capacity(stored.len() * 2);
    for param in stored {
        let ty = function.param_types[param as usize].clone();
        let local = function.n_vars as VarId;
        function.n_vars += 1;
        function
            .var_names
            .push(function.var_names[param as usize].clone());
        function.var_tys.insert(local, ty.clone());
        renumber_block_slots(&mut function.blocks, &|var| {
            if var == param { local } else { var }
        });
        let value = Reg(function.n_regs);
        function.n_regs += 1;
        function.reg_types.insert(value.0, ty.clone());
        if let Some((span, _)) = function
            .spans
            .0
            .values()
            .find(|(_, origin)| *origin == Some(param))
            .cloned()
        {
            function.spans.0.insert(value.0, (span, Some(param)));
        }
        entry.push(MirInstr::UseVar {
            dest: value,
            var: param,
            mode: UseMode::Copy,
        });
        entry.push(MirInstr::DefVar {
            var: local,
            src: value,
            binding_ty: Some(ty),
            in_place: false,
        });
    }
    let block = &mut function.blocks[0];
    block.instrs.splice(0..0, entry);
    true
}

/// A store, read, or kill site: the block list (numbered in preorder from
/// the function's own, 0), the block, and the instruction's index.
type Site = (usize, MirBlockId, usize);

/// The slots [`mem_2_reg`] promotes, and the register each of their reads
/// becomes a copy of.
struct Promotion {
    slots: BTreeSet<VarId>,
    reads: HashMap<Site, Reg>,
}

/// One mention of a candidate slot.
#[derive(Clone, Copy)]
enum Mention {
    /// A store of the register.
    Store(Reg),
    /// A read into the register; `ends` when it ends the slot.
    Read { dest: Reg, ends: bool },
    /// A declaration, drop, or consumption: the slot holds nothing after it.
    Kill,
    /// An interior invalidation, a no-op on a slot of this type, defining
    /// this marker.
    Marker(Reg),
}

/// A candidate's reaching store at a point of its list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reaching {
    /// No path reaches the point yet: the join's identity.
    Unreached,
    /// Some path reaches it with the slot holding nothing.
    Uninit,
    /// Every path reaches it past this store last.
    Store(usize),
    /// Paths reach it past different stores.
    Conflict,
}

impl Reaching {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unreached, other) | (other, Self::Unreached) => other,
            (a, b) if a == b => a,
            _ => Self::Conflict,
        }
    }
}

/// The census of one slot: the list holding its mentions, and whether some
/// mention keeps it.
#[derive(Default)]
struct Census {
    list: Option<usize>,
    kept: bool,
    mentions: Vec<(Site, Mention)>,
}

/// The slots of `function` [`mem_2_reg`] would promote, with each read's
/// stored register. Each of the `entry_stored` parameters counts as stored
/// at the entry, by no register, and is a candidate as a local is.
fn promotable(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    entry_stored: &BTreeSet<VarId>,
) -> Promotion {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut reads = vec![0_u32; function.n_regs as usize];
    count_reads(&mut function.blocks, &mut reads);
    let mut lists = Vec::new();
    collect_lists(&function.blocks, true, &mut lists);
    let mut census: HashMap<VarId, Census> = HashMap::new();
    for (list, (blocks, _)) in lists.iter().enumerate() {
        take_census(list, blocks, &mut census);
    }
    let candidate = |var: VarId, entry: &Census| {
        let typed = function.var_tys.get(&var);
        (var as usize >= function.n_params || entry_stored.contains(&var))
            && !entry.kept
            && typed.is_some_and(|ty| promotable_type(ty, declarations))
            && entry.mentions.iter().all(|(_, mention)| match *mention {
                Mention::Store(src) => {
                    function.reg_types.get(&src.0) == typed
                        && definitions.get(src.0 as usize) == Some(&1)
                        && reads.get(src.0 as usize) == Some(&1)
                }
                Mention::Read { dest, .. } => function.reg_types.get(&dest.0) == typed,
                Mention::Kill => true,
                Mention::Marker(marker) => reads.get(marker.0 as usize) == Some(&0),
            })
    };
    let mut candidates: Vec<Vec<VarId>> = vec![Vec::new(); lists.len()];
    for (&var, entry) in &census {
        if let Some(list) = entry.list
            && candidate(var, entry)
        {
            candidates[list].push(var);
        }
    }
    let mut promotion = Promotion {
        slots: BTreeSet::new(),
        reads: HashMap::new(),
    };
    for (list, slots) in candidates.iter_mut().enumerate() {
        if slots.is_empty() {
            continue;
        }
        slots.sort_unstable();
        let (blocks, function_level) = lists[list];
        let entry: Vec<Reaching> = slots
            .iter()
            .map(|var| {
                if list == 0 && entry_stored.contains(var) {
                    Reaching::Store(usize::MAX)
                } else {
                    Reaching::Uninit
                }
            })
            .collect();
        solve(
            &ListFlow {
                list,
                blocks,
                function_level,
                slots,
                census: &census,
            },
            entry,
            &mut promotion,
        );
    }
    promotion
}

/// Whether a slot of `ty` may be promoted: a trivially register-passable
/// value of a size the declaration fixes.
fn promotable_type(ty: &Ty, declarations: &MirDeclarations) -> bool {
    !is_symbolic(ty) && declarations.copy_is_identity(ty)
}

/// Every block list of `blocks` in preorder, `blocks` first, each with
/// whether it is the function's own.
fn collect_lists<'a>(
    blocks: &'a [MirBlock],
    function_level: bool,
    lists: &mut Vec<(&'a [MirBlock], bool)>,
) {
    lists.push((blocks, function_level));
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        for region in instruction_regions(instruction) {
            collect_lists(region, false, lists);
        }
    }
}

/// Record every slot mention of the list `list`'s own blocks (not of the
/// regions they hold) in `census`.
fn take_census(list: usize, blocks: &[MirBlock], census: &mut HashMap<VarId, Census>) {
    for (at_block, block) in blocks.iter().enumerate() {
        let mut defined = HashSet::new();
        for (at, instruction) in block.instrs.iter().enumerate() {
            let site = (list, at_block, at);
            let slots = instruction_slots(instruction);
            // A stored register must be defined before the store in its block.
            let used = slot_use(instruction).filter(|(_, mention)| match mention {
                Mention::Store(src) => defined.contains(&src.0),
                _ => true,
            });
            match (used, slots.as_slice()) {
                (Some((var, mention)), [only]) if *only == var => {
                    let entry = census.entry(var).or_default();
                    entry.kept |= entry.list.is_some_and(|other| other != list);
                    entry.list = Some(list);
                    entry.mentions.push((site, mention));
                }
                _ => keep(census, slots),
            }
            if let MirInstr::Try {
                handler, cleanup, ..
            } = instruction
            {
                keep(census, handler.iter().filter_map(|(slot, _)| *slot));
                keep(census, cleanup.iter().copied());
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            defined.extend(results.iter().map(|reg| reg.0));
        }
        if let MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } =
            &block.term
        {
            keep(census, cleanup.iter().copied());
        }
    }
}

/// Mark each of `slots` kept.
fn keep(census: &mut HashMap<VarId, Census>, slots: impl IntoIterator<Item = VarId>) {
    for var in slots {
        census.entry(var).or_default().kept = true;
    }
}

/// The slot `instruction` mentions as a promotable use, and how.
const fn slot_use(instruction: &MirInstr) -> Option<(VarId, Mention)> {
    match instruction {
        MirInstr::DefVar { var, src, .. } => Some((*var, Mention::Store(*src))),
        MirInstr::UseVar { dest, var, mode } => match mode {
            UseMode::Copy => Some((
                *var,
                Mention::Read {
                    dest: *dest,
                    ends: false,
                },
            )),
            UseMode::Move | UseMode::Forward => Some((
                *var,
                Mention::Read {
                    dest: *dest,
                    ends: true,
                },
            )),
            UseMode::BorrowShared | UseMode::BorrowMut => None,
        },
        MirInstr::LoadPlace { dest, place } if place.proj.is_empty() && place.through.is_none() => {
            Some((
                place.root,
                Mention::Read {
                    dest: *dest,
                    ends: false,
                },
            ))
        }
        MirInstr::DeclareVar { var } | MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            Some((*var, Mention::Kill))
        }
        MirInstr::InvalidateInteriors {
            base,
            except: None,
            marker,
            ..
        } => Some((base.root, Mention::Marker(*marker))),
        _ => None,
    }
}

/// One block list's candidates, as [`solve`] reads them.
struct ListFlow<'a> {
    list: usize,
    blocks: &'a [MirBlock],
    function_level: bool,
    slots: &'a [VarId],
    census: &'a HashMap<VarId, Census>,
}

/// One event of a block for the reaching-store flow, at its instruction
/// index.
enum Event {
    Store {
        slot: usize,
        store: usize,
    },
    Kill {
        slot: usize,
    },
    Read {
        slot: usize,
        at: usize,
    },
    /// A region op whose regions escape to these function-level blocks.
    Escape(Vec<MirBlockId>),
}

/// Solve the reaching stores of `flow`'s candidates from the `entry`
/// states, and add each candidate whose every read sees one store, on its
/// own side of each `comptime for` body's boundary, to `promotion`.
fn solve(flow: &ListFlow<'_>, entry: Vec<Reaching>, promotion: &mut Promotion) {
    let &ListFlow {
        list,
        blocks,
        function_level,
        slots,
        census,
    } = flow;
    // Store sites, by number: the block and the stored register.
    let mut stores: Vec<(MirBlockId, Reg)> = Vec::new();
    let mut events: Vec<Vec<(usize, Event)>> = (0..blocks.len()).map(|_| Vec::new()).collect();
    for (slot, var) in slots.iter().enumerate() {
        for &((_, block, at), mention) in &census[var].mentions {
            let event = match mention {
                Mention::Store(src) => {
                    stores.push((block, src));
                    Event::Store {
                        slot,
                        store: stores.len() - 1,
                    }
                }
                Mention::Read { ends, .. } => {
                    events[block].push((at, Event::Read { slot, at }));
                    if !ends {
                        continue;
                    }
                    Event::Kill { slot }
                }
                Mention::Kill => Event::Kill { slot },
                Mention::Marker(_) => continue,
            };
            events[block].push((at, event));
        }
    }
    if function_level {
        for (at_block, block) in blocks.iter().enumerate() {
            for (at, instruction) in block.instrs.iter().enumerate() {
                let escapes = region_escape_targets(instruction);
                if !escapes.is_empty() {
                    events[at_block].push((at, Event::Escape(escapes)));
                }
            }
        }
    }
    // A read and the kill that ends it share an index: the read comes first.
    for block in &mut events {
        block.sort_by_key(|(at, event)| (*at, !matches!(event, Event::Read { .. })));
    }
    let mut states = vec![vec![Reaching::Unreached; slots.len()]; blocks.len()];
    if blocks.is_empty() {
        return;
    }
    states[0] = entry;
    let mut pending = vec![0];
    let flow_into = |target: MirBlockId,
                     state: &[Reaching],
                     states: &mut Vec<Vec<Reaching>>,
                     pending: &mut Vec<MirBlockId>| {
        let Some(into) = states.get_mut(target) else {
            return;
        };
        let mut changed = false;
        for (into, &from) in into.iter_mut().zip(state) {
            let joined = into.join(from);
            changed |= joined != *into;
            *into = joined;
        }
        if changed {
            pending.push(target);
        }
    };
    while let Some(block) = pending.pop() {
        let mut state = states[block].clone();
        for (_, event) in &events[block] {
            match event {
                Event::Store { slot, store } => state[*slot] = Reaching::Store(*store),
                Event::Kill { slot } => state[*slot] = Reaching::Uninit,
                Event::Read { .. } => {}
                Event::Escape(targets) => {
                    for &target in targets {
                        flow_into(target, &state, &mut states, &mut pending);
                    }
                }
            }
        }
        for target in terminator_targets(&blocks[block].term) {
            flow_into(target, &state, &mut states, &mut pending);
        }
    }
    // Each read's reaching store, replayed from the solved entry states.
    let mut promoted = vec![true; slots.len()];
    let mut reached: Vec<(usize, Site, usize)> = Vec::new();
    for (at_block, block_events) in events.iter().enumerate() {
        let mut state = states[at_block].clone();
        for (_, event) in block_events {
            match event {
                Event::Store { slot, store } => state[*slot] = Reaching::Store(*store),
                Event::Kill { slot } => state[*slot] = Reaching::Uninit,
                Event::Read { slot, at } => match state[*slot] {
                    Reaching::Store(store) => reached.push((*slot, (list, at_block, *at), store)),
                    _ => promoted[*slot] = false,
                },
                Event::Escape(_) => {}
            }
        }
    }
    let bodies: Vec<HashSet<MirBlockId>> = blocks
        .iter()
        .enumerate()
        .filter_map(|(header, block)| match block.term {
            MirTerm::ComptimeFor { body, exit, .. } => Some(
                comptime_for_body(blocks, header, body, exit, function_level)
                    .into_iter()
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    let block_of = |store: usize| stores.get(store).map_or(0, |(block, _)| *block);
    for &(slot, (_, read_block, _), store) in &reached {
        let store_block = block_of(store);
        if bodies
            .iter()
            .any(|body| body.contains(&read_block) != body.contains(&store_block))
        {
            promoted[slot] = false;
        }
    }
    for (slot, var) in slots.iter().enumerate() {
        if promoted[slot] {
            promotion.slots.insert(*var);
        }
    }
    for (slot, site, store) in reached {
        if promoted[slot]
            && let Some(&(_, src)) = stores.get(store)
        {
            promotion.reads.insert(site, src);
        }
    }
}

/// Rewrite the block list numbered `*next` and the regions below it: each
/// promoted read becomes a copy of its stored register, and each other
/// mention of a promoted slot goes, its results added to `erased_results`.
/// Returns how many instructions went.
fn rewrite_list(
    blocks: &mut [MirBlock],
    next: &mut usize,
    promotion: &Promotion,
    erased_results: &mut HashSet<u32>,
) -> usize {
    let list = *next;
    *next += 1;
    let mut erased = 0;
    for (at_block, block) in blocks.iter_mut().enumerate() {
        let instructions = std::mem::take(&mut block.instrs);
        for (at, mut instruction) in instructions.into_iter().enumerate() {
            for region in instruction_regions_mut(&mut instruction) {
                erased += rewrite_list(region, next, promotion, erased_results);
            }
            if let Some(&value) = promotion.reads.get(&(list, at_block, at)) {
                let mut results = Vec::new();
                instruction_result_regs(&instruction, &mut results);
                block.instrs.push(MirInstr::CopyValue {
                    dest: results[0],
                    value,
                });
                continue;
            }
            if slot_use(&instruction).is_some_and(|(var, _)| promotion.slots.contains(&var)) {
                let mut results = Vec::new();
                instruction_result_regs(&instruction, &mut results);
                erased_results.extend(results.iter().map(|reg| reg.0));
                erased += 1;
                continue;
            }
            block.instrs.push(instruction);
        }
    }
    erased
}
