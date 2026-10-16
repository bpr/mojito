//! Destructor naming: the last step of drop elaboration, upstream's
//! destructor insertion after the lifetime check.
//!
//! Upstream leaves, at each death, a call naming the `__del__` of the dead
//! value's type, in the unwind blocks too. Each `drop.var`, `drop.place`,
//! and cleanup-list drop here names that destructor as its `destructor`
//! operand, so the symbol walk, the elaborator's respell, and
//! `eliminate-dead-symbols` follow it as they follow a call's callee, and
//! no consumer looks the destructor up from the value's type again. A drop
//! of a type the member table names no `__deinit__` for, a symbolic one
//! included, keeps `None`: the elaborator names an instance's destructor
//! once its type is concrete. A named drop whose slot holds a whole value
//! on every path to it then becomes the call upstream leaves
//! (`mir::lower_whole_drops`), the elaborator doing the same once it has
//! named an instance's.

use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    CleanupDrop, MirBlock, MirDeclarations, MirFunction, MirInstr, MirTerm,
    instruction_regions_mut, lower_whole_drops,
};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Name the destructor of every drop in `function`, regions and cleanup
/// lists included, and lower each named drop of a whole slot to the call
/// of its destructor.
pub fn name_destructors(function: &mut MirFunction, declarations: &MirDeclarations) {
    name_in_blocks(&mut function.blocks, &function.var_tys, declarations);
    lower_whole_drops(function);
}

fn name_in_blocks(
    blocks: &mut [MirBlock],
    var_tys: &HashMap<VarId, Ty>,
    declarations: &MirDeclarations,
) {
    let named = |ty: Option<&Ty>| {
        ty.and_then(|ty| declarations.destructor(ty))
            .map(str::to_owned)
    };
    let name_cleanup = |cleanup: &mut [CleanupDrop]| {
        for drop in cleanup {
            drop.destructor = named(var_tys.get(&drop.var));
        }
    };
    for block in blocks {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::DropVar { var, destructor } => *destructor = named(var_tys.get(var)),
                MirInstr::DropPlace { place, destructor } => *destructor = named(place.ty.as_ref()),
                MirInstr::Try { cleanup, .. } => name_cleanup(cleanup),
                _ => {}
            }
            for region in instruction_regions_mut(instruction) {
                name_in_blocks(region, var_tys, declarations);
            }
        }
        if let MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } =
            &mut block.term
        {
            name_cleanup(cleanup);
        }
    }
}
