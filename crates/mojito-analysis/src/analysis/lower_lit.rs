//! `lower-lit`: the part of upstream's `LowerLIT` that MIR has, run on
//! each body right after drop elaboration.
//!
//! Upstream's `LITLowerer::lowerLITOps` erases the ownership ops "used
//! internally by the frontend and ownership lowering, but … not needed after
//! that", `lit.ownership.use` among them: a no-op use that keeps a value
//! alive, which is MIR's `lifetime.keep_alive`. Upstream has no op for
//! interior invalidation at all, since that state lives inside
//! `CheckLifetimes`; MIR's `interiors.invalidate` carries it to the
//! ownership analysis and drop elaboration, which together are MIR's
//! lifetime check, and ends here too. Below this pass neither marker is
//! legal: concrete verification refuses one, and so do both backends.
//!
//! `var.declare` stays. It is `lit.var.decl`, which upstream lowers to a
//! stack allocation rather than erasing, and `mem-2-reg` reads it as the
//! point where a slot declared in a loop is uninitialized again.

use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::{MirBlock, MirDeclarations, MirFunction, MirInstr, instruction_regions_mut};
use std::collections::HashSet;

/// The pass, under upstream's name for the lowering it is part of.
pub const LOWER_LIT: BodyPass = BodyPass {
    name: "lower-lit",
    run: lower_lit,
};

/// Erase every `lifetime.keep_alive` and `interiors.invalidate` in
/// `function`, its regions included.
///
/// The erased markers' registers lose their types and spans, and registers
/// are not renumbered. Returns whether any marker was erased.
pub fn lower_lit(_name: &str, function: &mut MirFunction, _declarations: &MirDeclarations) -> bool {
    let mut erased_markers = HashSet::new();
    let erased = erase_markers(&mut function.blocks, &mut erased_markers);
    function
        .reg_types
        .retain(|reg, _| !erased_markers.contains(reg));
    function
        .spans
        .0
        .retain(|reg, _| !erased_markers.contains(reg));
    timing::count("erased_lifetime_markers", erased as u64);
    erased > 0
}

/// Erase the lifetime-only markers of `blocks` and their regions, adding
/// each erased invalidation's marker register to `erased_markers`, and
/// return how many instructions went.
fn erase_markers(blocks: &mut [MirBlock], erased_markers: &mut HashSet<u32>) -> usize {
    blocks
        .iter_mut()
        .map(|block| {
            let before = block.instrs.len();
            block.instrs.retain(|instruction| match instruction {
                MirInstr::KeepAlive { .. } => false,
                MirInstr::InvalidateInteriors { marker, .. } => {
                    erased_markers.insert(marker.0);
                    false
                }
                _ => true,
            });
            let nested: usize = block
                .instrs
                .iter_mut()
                .flat_map(instruction_regions_mut)
                .map(|region| erase_markers(region, erased_markers))
                .sum();
            before - block.instrs.len() + nested
        })
        .sum()
}
