//! Destructor naming: the last step of drop elaboration, upstream's
//! destructor insertion after the lifetime check.
//!
//! Upstream leaves, at each death, a call naming the `__del__` of the dead
//! value's type, in the unwind blocks too. Each `drop.var`, `drop.place`,
//! and cleanup-list drop here names that destructor as its `destructor`
//! operand, and the teardown primitives a library body runs name theirs
//! again (`mir::name_teardown_destructors`, which a pass that retypes one
//! reruns), so the symbol walk, the elaborator's respell, and
//! `eliminate-dead-symbols` follow it as they follow a call's callee, and
//! no consumer looks the destructor up from the value's type again. A drop
//! of a type the member table names no `__deinit__` for, a symbolic one
//! included, keeps `None`: the elaborator names an instance's destructor
//! once its type is concrete. Every named drop, decided where drop
//! elaboration placed it, then becomes the call upstream leaves
//! (`mir::lower_drops`), the elaborator doing the same once it has named an
//! instance's; one whose destructor is available only under a `where`
//! clause waits for that.

use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, instruction_regions_mut, lower_drops,
    name_teardown_destructors,
};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Name the destructor of every drop in `function`, regions included, and
/// lower each named drop to the call of its destructor.
pub fn name_destructors(function: &mut MirFunction, declarations: &MirDeclarations) {
    name_in_blocks(&mut function.blocks, &function.var_tys, declarations);
    name_teardown_destructors(function, declarations);
    lower_drops(function, &|destructor| {
        declarations
            .function(destructor)
            .is_none_or(|declaration| declaration.availability.is_empty())
    });
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
    for block in blocks {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::DropVar { var, destructor } => *destructor = named(var_tys.get(var)),
                MirInstr::DropPlace { place, destructor } => *destructor = named(place.ty.as_ref()),
                _ => {}
            }
            for region in instruction_regions_mut(instruction) {
                name_in_blocks(region, var_tys, declarations);
            }
        }
    }
}
