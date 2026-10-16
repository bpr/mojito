//! Drops lowered to calls of their destructor.
//!
//! Upstream's destructor insertion leaves, at each point a value dies, a
//! plain call of its type's `__del__` consuming the storage. A `drop.var`
//! names that destructor but stays a drop, which both backends run under a
//! runtime test of the slot's state: drop elaboration places a drop by
//! liveness, so on some path to it the slot may be empty or partly moved.
//! Where the slot holds a whole value on every path to the drop, the state
//! is definite and the drop is the call: a forwarding use of the slot,
//! which ends it and hands its storage to the callee as Mojo's owned
//! argument convention does, and a call of the named destructor on it as
//! the `deinit self` receiver, which is what an explicit `x^.finish()`
//! lowers to. Every other drop stays a drop.

use super::{
    CleanupDrop, MirBlock, MirFunction, MirInstr, MirTerm, UseMode, VarId, cleanup_drops,
    fresh_register, instruction_regions, instruction_regions_mut, instruction_slots,
    region_escape_targets, terminator_targets,
};
use mojito_common::token::SourceSpan;
use mojito_types::types::TyKind;
use std::collections::{HashMap, HashSet};

/// Lower every named `drop.var` of `function` whose slot holds a whole
/// value on every path to it to a call of the destructor.
///
/// The drop becomes a forwarding use of the slot and a call of the named
/// destructor on it, in the function's regions too. An owned parameter
/// holds a whole value as the function is entered; every other slot holds
/// one once a whole store fills it.
pub fn lower_whole_drops(function: &mut MirFunction) {
    let mut blocks = std::mem::take(&mut function.blocks);
    let entry: Vec<bool> = (0..function.n_vars)
        .map(|var| {
            var < function.n_params && function.owned_params.get(var).copied().unwrap_or(false)
        })
        .collect();
    lower_list(
        &mut blocks,
        &|var| entry.get(var as usize).copied().unwrap_or(false),
        function,
    );
    function.blocks = blocks;
}

/// Lower the whole drops of the block list `list`, whose slots hold a whole
/// value on entry where `entry` says so, and of its regions.
fn lower_list(list: &mut [MirBlock], entry: &dyn Fn(VarId) -> bool, function: &mut MirFunction) {
    let blocks: &[MirBlock] = list;
    // A lowered drop leaves every slot's state as the drop did, so the
    // sites are decided over the list as it is.
    let sites: Vec<(usize, usize, VarId, String)> = blocks
        .iter()
        .enumerate()
        .flat_map(|(block, holder)| {
            holder
                .instrs
                .iter()
                .enumerate()
                .filter_map(move |(index, instruction)| match instruction {
                    MirInstr::DropVar {
                        var,
                        destructor: Some(destructor),
                    } => Some((block, index, *var, destructor.clone())),
                    _ => None,
                })
        })
        .filter(|&(block, index, var, _)| is_whole_at(blocks, block, index, var, entry(var)))
        .collect();
    for block in 0..list.len() {
        for index in 0..list[block].instrs.len() {
            let instruction = &list[block].instrs[index];
            if instruction_regions(instruction).is_empty() {
                continue;
            }
            let states: HashMap<VarId, bool> = named_drop_slots(instruction)
                .into_iter()
                .map(|var| (var, is_whole_at(list, block, index, var, entry(var))))
                .collect();
            let entries = region_entries(instruction, &states);
            for (region, entry) in instruction_regions_mut(&mut list[block].instrs[index])
                .into_iter()
                .zip(entries)
            {
                lower_list(
                    region,
                    &|var| entry.get(&var).copied().unwrap_or(false),
                    function,
                );
            }
        }
    }
    for (block, index, var, destructor) in sites.into_iter().rev() {
        let span = death_span(function, var);
        let Some(ty) = function.var_tys.get(&var).cloned() else {
            continue;
        };
        let recv = fresh_register(function, ty, span.as_ref());
        let dest = fresh_register(function, TyKind::None.into(), span.as_ref());
        list[block].instrs.splice(
            index..=index,
            [
                MirInstr::UseVar {
                    dest: recv,
                    var,
                    mode: UseMode::Forward,
                    constructor: None,
                },
                MirInstr::MethodCall {
                    dest,
                    recv,
                    method: "__deinit__".to_string(),
                    resolved: Some(destructor),
                    raises: None,
                    reference_result: None,
                    result_adapter: None,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    recv_place: None,
                    recv_writes: true,
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
        );
    }
}

/// Whether the slot `var` holds a whole value on every path through `list`
/// to the instruction at `index` of `block`: stored whole, and neither moved
/// from, in whole or in part, nor ended since. `entry` says it holds one as
/// `list` is entered; an escape into `list` from one of its regions is
/// taken to hold none.
fn is_whole_at(list: &[MirBlock], block: usize, index: usize, var: VarId, entry: bool) -> bool {
    let through = |state: bool, instructions: &[MirInstr]| {
        let mut conditional = HashSet::new();
        instructions.iter().fold(state, |state, instruction| {
            whole_after(state, instruction, var, &mut conditional)
        })
    };
    let mut inputs = vec![true; list.len()];
    inputs[0] = entry;
    let mut pending: Vec<usize> = (0..list.len()).rev().collect();
    while let Some(current) = pending.pop() {
        let state = through(inputs[current], &list[current].instrs);
        let escapes = list[current]
            .instrs
            .iter()
            .flat_map(region_escape_targets)
            .map(|target| (target, false));
        let successors = terminator_targets(&list[current].term)
            .into_iter()
            .map(|target| (target, state));
        for (target, state) in successors.chain(escapes) {
            if let Some(input) = inputs.get_mut(target)
                && *input
                && !state
            {
                *input = false;
                pending.push(target);
            }
        }
    }
    through(inputs[block], &list[block].instrs[..index])
}

/// The state each region of `instruction` is entered with, for the slots
/// `states` gives the state of before the instruction, in the order
/// [`instruction_regions`] lists the regions. An `if`'s arms and a
/// `comptime if`'s follow the condition alone; a loop's body follows
/// itself; a `try`'s handler and `finally` follow its body through the
/// raise edge's cleanup, and its `else` follows the body.
fn region_entries(
    instruction: &MirInstr,
    states: &HashMap<VarId, bool>,
) -> Vec<HashMap<VarId, bool>> {
    let entered = |before: &[&Vec<MirBlock>], cleanup: &[CleanupDrop]| -> HashMap<VarId, bool> {
        states
            .iter()
            .map(|(&var, &state)| {
                let whole = state
                    && !cleanup_drops(cleanup, var)
                    && before.iter().all(|blocks| keeps_whole(blocks, var));
                (var, whole)
            })
            .collect()
    };
    match instruction {
        MirInstr::If { elifs, .. } => {
            let conds: Vec<&Vec<MirBlock>> = elifs.iter().map(|elif| &elif.cond).collect();
            vec![entered(&conds, &[]); instruction_regions(instruction).len()]
        }
        MirInstr::Loop { body, .. } => vec![entered(&[body], &[])],
        MirInstr::ComptimeIf { arms, .. } => vec![entered(&[], &[]); arms.len() + 1],
        MirInstr::ComptimeFor { body, .. } => vec![entered(&[body], &[]); 2],
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } => {
            let mut entries = vec![entered(&[], &[])];
            if handler.is_some() {
                entries.push(entered(&[body], cleanup));
            }
            if orelse.is_some() {
                entries.push(entered(&[body], &[]));
            }
            if finalbody.is_some() {
                let before: Vec<&Vec<MirBlock>> = std::iter::once(body)
                    .chain(handler.iter().map(|(_, blocks)| blocks))
                    .chain(orelse.iter())
                    .collect();
                entries.push(entered(&before, cleanup));
            }
            entries
        }
        _ => Vec::new(),
    }
}

/// Whether every instruction of `blocks`, regions included, leaves the slot
/// `var` whole when it was, and no escape from them ends it first.
fn keeps_whole(blocks: &[MirBlock], var: VarId) -> bool {
    blocks.iter().all(|block| {
        let mut conditional = HashSet::new();
        !matches!(&block.term, MirTerm::EscapeJump { cleanup, .. } if cleanup_drops(cleanup, var))
            && block.instrs.iter().all(|instruction| {
                whole_after(true, instruction, var, &mut conditional)
                    && instruction_regions(instruction)
                        .into_iter()
                        .all(|region| keeps_whole(region, var))
            })
    })
}

/// Whether the slot `var` holds a whole value after `instruction`, given
/// whether it held one before: a whole store fills it, and so does the
/// iterator a `for` loop makes in a slot of its own; a read, a borrow, a
/// store into it, or an advance of the iterator keeps it; anything else
/// naming it, a `try`'s raise-edge cleanup included, leaves it possibly
/// partial or empty; a lifetime marker names it without touching it. An
/// iterable a `for` loop consumes in place holds the
/// loop's iterator from then on, a value of another type than the slot's,
/// which the slot's destructor does not destroy. A store of what a
/// `try_next` yielded fills the slot on the yielding edge alone: the
/// block's `conditional` registers are those, which the fold collects.
fn whole_after(
    state: bool,
    instruction: &MirInstr,
    var: VarId,
    conditional: &mut HashSet<u32>,
) -> bool {
    match instruction {
        MirInstr::TryNext { dest, .. } => {
            conditional.insert(dest.0);
            state
        }
        MirInstr::DefVar { var: slot, src, .. } if *slot == var => !conditional.contains(&src.0),
        MirInstr::GetIter { source, dest, .. } if *source == var && *dest == var => false,
        MirInstr::GetIter { dest, .. } if *dest == var => true,
        MirInstr::UseVar {
            var: slot, mode, ..
        } if *slot == var => state && !mode.moves(),
        MirInstr::MakeClosure { captures, .. } => {
            state
                && captures.iter().all(|capture| {
                    capture.mode == super::MirCaptureMode::Reference || capture.place.root != var
                })
        }
        MirInstr::Try { cleanup, .. } if cleanup_drops(cleanup, var) => false,
        MirInstr::LoadPlace { .. }
        | MirInstr::Store { .. }
        | MirInstr::StoreRef { .. }
        | MirInstr::MakeRef { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::InvalidateInteriors { .. }
        | MirInstr::EstablishLoans { .. }
        | MirInstr::VariantSet { .. }
        | MirInstr::VariantSetInitWith { .. }
        | MirInstr::VariantReplace { .. }
        | MirInstr::Call { .. }
        | MirInstr::CallIndirect { .. }
        | MirInstr::MethodCall { .. }
        | MirInstr::Index { .. }
        | MirInstr::Slice { .. }
        | MirInstr::MultiIndex { .. }
        | MirInstr::MultiSet { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::HasNext { .. }
        | MirInstr::Next { .. } => state,
        _ => state && !names_slot(instruction, var),
    }
}

/// Whether `instruction` names the slot `var`, in its regions too.
fn names_slot(instruction: &MirInstr, var: VarId) -> bool {
    instruction_slots(instruction).contains(&var)
        || instruction_regions(instruction)
            .into_iter()
            .flatten()
            .flat_map(|block| &block.instrs)
            .any(|instruction| names_slot(instruction, var))
}

/// The slots the named drops inside `instruction`'s regions end.
fn named_drop_slots(instruction: &MirInstr) -> Vec<VarId> {
    let mut slots = Vec::new();
    for instruction in instruction_regions(instruction)
        .into_iter()
        .flatten()
        .flat_map(|block| &block.instrs)
    {
        if let MirInstr::DropVar {
            var,
            destructor: Some(_),
        } = instruction
        {
            slots.push(*var);
        }
        slots.extend(named_drop_slots(instruction));
    }
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// Where the slot `var` dies: the location of its last read, which a drop
/// of it follows.
fn death_span(function: &MirFunction, var: VarId) -> Option<SourceSpan> {
    function
        .spans
        .0
        .iter()
        .filter(|(_, (_, origin))| *origin == Some(var))
        .max_by_key(|(reg, _)| **reg)
        .map(|(_, (span, _))| span.clone())
}
