//! Renumbering a function's variable slots: every place root and
//! `through`, every capture access, loan, interior, and cleanup that names a
//! slot, and the span table's origins. Runtime promotion and the unrolling
//! of a `comptime for` both move slots.

use std::collections::BTreeSet;

use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MirBlock, MirFunction, MirInstr, MirTerm, for_each_instruction_slot_mut, instruction_regions,
    instruction_regions_mut, instruction_slots,
};

/// Remove the `retired` slots from `function`, none of them a parameter nor
/// still addressed: every slot above one moves down past it, and its name
/// and type leave the tables.
pub(super) fn retire_slots(function: &mut MirFunction, retired: &BTreeSet<VarId>) {
    debug_assert!(
        retired
            .iter()
            .all(|&slot| slot as usize >= function.n_params),
        "a parameter slot is never retired"
    );
    let addressed = std::cell::Cell::new(false);
    let renumber = |var: VarId| {
        addressed.set(addressed.get() || retired.contains(&var));
        var - retired.range(..var).count() as VarId
    };
    renumber_slots(function, &renumber);
    debug_assert!(!addressed.get(), "a retired slot is still addressed");
    for &slot in retired.iter().rev() {
        function.var_names.remove(slot as usize);
    }
    function.var_tys = function
        .var_tys
        .iter()
        .filter(|(var, _)| !retired.contains(var))
        .map(|(var, ty)| (renumber(*var), ty.clone()))
        .collect();
    function.n_vars -= retired.len();
}

/// The slots `blocks` address, the regions they hold included.
pub(super) fn addressed_slots<'a>(
    blocks: impl IntoIterator<Item = &'a MirBlock>,
) -> BTreeSet<VarId> {
    let mut addressed = BTreeSet::new();
    for block in blocks {
        collect_block_slots(block, &mut addressed);
    }
    addressed
}

pub(super) fn renumber_slots(function: &mut MirFunction, renumber: &dyn Fn(VarId) -> VarId) {
    renumber_blocks(&mut function.blocks, renumber);
    for (_, origin) in function.spans.0.values_mut() {
        if let Some(var) = origin {
            *var = renumber(*var);
        }
    }
}

pub(super) fn renumber_blocks(blocks: &mut [MirBlock], renumber: &dyn Fn(VarId) -> VarId) {
    for block in blocks {
        for instruction in &mut block.instrs {
            renumber_instruction(instruction, renumber);
        }
        match &mut block.term {
            MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } => {
                for var in cleanup {
                    *var = renumber(*var);
                }
            }
            MirTerm::ComptimeFor { slot, .. } => *slot = renumber(*slot),
            MirTerm::Jump(_)
            | MirTerm::Branch { .. }
            | MirTerm::ComptimeBranch { .. }
            | MirTerm::Return(_)
            | MirTerm::FallOff
            | MirTerm::Yield(_)
            | MirTerm::ElifCond(_)
            | MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak
            | MirTerm::ComptimeContinue
            | MirTerm::Unreachable => {}
        }
    }
}

fn renumber_instruction(instruction: &mut MirInstr, renumber: &dyn Fn(VarId) -> VarId) {
    for_each_instruction_slot_mut(instruction, &mut |var| *var = renumber(*var));
    if let MirInstr::Try {
        handler, cleanup, ..
    } = instruction
    {
        if let Some((Some(slot), _)) = handler {
            *slot = renumber(*slot);
        }
        for var in cleanup {
            *var = renumber(*var);
        }
    }
    for region in instruction_regions_mut(instruction) {
        renumber_blocks(region, renumber);
    }
}

/// Every slot one block names: its instructions', its regions' and their
/// handler bindings and cleanups, and its terminator's.
fn collect_block_slots(block: &MirBlock, addressed: &mut BTreeSet<VarId>) {
    for instruction in &block.instrs {
        addressed.extend(instruction_slots(instruction));
        if let MirInstr::Try {
            handler, cleanup, ..
        } = instruction
        {
            addressed.extend(handler.iter().filter_map(|(slot, _)| *slot));
            addressed.extend(cleanup.iter().copied());
        }
        for inner in instruction_regions(instruction).into_iter().flatten() {
            collect_block_slots(inner, addressed);
        }
    }
    match &block.term {
        MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } => {
            addressed.extend(cleanup.iter().copied());
        }
        MirTerm::ComptimeFor { slot, .. } => {
            addressed.insert(*slot);
        }
        MirTerm::Jump(_)
        | MirTerm::Branch { .. }
        | MirTerm::ComptimeBranch { .. }
        | MirTerm::Return(_)
        | MirTerm::FallOff
        | MirTerm::Yield(_)
        | MirTerm::ElifCond(_)
        | MirTerm::Break { .. }
        | MirTerm::Continue { .. }
        | MirTerm::ComptimeYield(_)
        | MirTerm::ComptimeBreak
        | MirTerm::ComptimeContinue
        | MirTerm::Unreachable => {}
    }
}
