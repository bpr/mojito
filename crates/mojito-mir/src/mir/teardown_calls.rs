//! Teardown naming: upstream's destructor insertion naming the `__del__` a
//! library body's teardown primitive runs.
//!
//! A storage destroy (`unsafe_deinit_pointee`, `MaybeUninit.unsafe_deinit`),
//! a variant store's replaced payload (`Variant.set`, per alternative under
//! its tag), and an owned closure capture released at the closure's death
//! each name the `__deinit__` the value's struct records as a `destructor`
//! operand, as a `drop.var` does, so the symbol walk, the elaborator's
//! respell, and `eliminate-dead-symbols` follow it as they follow a call's
//! callee, and no backend looks the destructor up from the value's type
//! again. A site whose type the member table names no destructor for, a
//! symbolic one included, keeps `None`: the elaborator names an instance's
//! once its type is concrete. Naming is a function of the site's type
//! alone, so a pass that makes or retypes such a site names it again by
//! rerunning this walk, as [`crate::mir::name_constructors`] does for
//! copies and moves.

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, instruction_regions_mut,
    variant_place_alternatives,
};
use mojito_hir::hir::VarId;
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Name the destructor of every storage destroy, variant store, and owned
/// closure capture in `function`, regions included.
pub fn name_teardown_destructors(function: &mut MirFunction, declarations: &MirDeclarations) {
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
    for block in blocks {
        for instruction in &mut block.instrs {
            match instruction {
                MirInstr::PointerStorageDestroy {
                    element,
                    destructor,
                    ..
                }
                | MirInstr::UninitStorageDestroy {
                    element,
                    destructor,
                    ..
                } => *destructor = named(Some(element)),
                MirInstr::VariantSet {
                    place, destructors, ..
                }
                | MirInstr::VariantSetInitWith {
                    place, destructors, ..
                } => {
                    *destructors = variant_place_alternatives(place, var_tys)
                        .into_iter()
                        .flatten()
                        .map(|alternative| named(Some(alternative)))
                        .collect();
                }
                MirInstr::MakeClosure { captures, .. } => {
                    for capture in captures {
                        capture.destructor = capture
                            .destructor_member()
                            .and_then(|_| named(capture.place.storage_ty(var_tys)));
                    }
                }
                _ => {}
            }
            for region in instruction_regions_mut(instruction) {
                name_in_blocks(region, var_tys, declarations);
            }
        }
    }
}
