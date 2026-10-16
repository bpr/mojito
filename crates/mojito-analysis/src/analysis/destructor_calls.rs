//! Destructor naming: the last step of drop elaboration, upstream's
//! destructor insertion after the lifetime check.
//!
//! Upstream leaves, at each death, a call naming the `__del__` of the dead
//! value's type. Each `drop.var` and `drop.place` here names that
//! destructor as its `destructor` operand, so the symbol walk, the
//! elaborator's respell, and `eliminate-dead-symbols` follow it as they
//! follow a call's callee, and no consumer looks the destructor up from the
//! value's type again. A drop of a type the member table names no
//! `__deinit__` for, a symbolic one included, keeps `None`: the elaborator
//! names an instance's destructor once its type is concrete.

use mojito_hir::hir::VarId;
use mojito_mir::mir::{MirBlock, MirDeclarations, MirFunction, MirInstr, instruction_regions_mut};
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Name the destructor of every drop in `function`, regions included.
pub fn name_destructors(function: &mut MirFunction, declarations: &MirDeclarations) {
    name_in_blocks(&mut function.blocks, &function.var_tys, declarations);
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
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
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
