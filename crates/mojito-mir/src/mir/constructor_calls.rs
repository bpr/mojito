//! Constructor naming: upstream's copy/move insertion naming the
//! `__copyinit__` or `__moveinit__` at each copy or move site.
//!
//! Each `var.use copy`, `var.use move`, and `value.copy` names the
//! constructor its value's struct records as its `constructor` operand, so
//! the symbol walk, the elaborator's respell, and `eliminate-dead-symbols`
//! follow it as they follow a call's callee, and no backend looks the
//! constructor up from the value's type again. A site whose type the member
//! table names no such member for, a symbolic one included, keeps `None`:
//! the elaborator names an instance's constructor once its type is
//! concrete. Naming is a function of the site's type alone, so a pass that
//! makes or retypes a copy or move names it again by rerunning this walk.

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, UseMode, instruction_regions_mut,
};
use mojito_hir::hir::VarId;
use mojito_types::types::Ty;
use std::collections::HashMap;

/// Name the constructor of every copy and move in `function`, regions
/// included.
pub fn name_constructors(function: &mut MirFunction, declarations: &MirDeclarations) {
    name_in_blocks(
        &mut function.blocks,
        &function.var_tys,
        &function.reg_types,
        declarations,
    );
}

/// The lifecycle member a `var.use` in `mode` runs: `__copyinit__` for a
/// copy, `__moveinit__` for a move, none for a borrow or a forward.
pub const fn use_constructor_member(mode: UseMode) -> Option<&'static str> {
    match mode {
        UseMode::Copy => Some("__copyinit__"),
        UseMode::Move => Some("__moveinit__"),
        UseMode::BorrowShared | UseMode::BorrowMut | UseMode::Forward => None,
    }
}

fn name_in_blocks(
    blocks: &mut [MirBlock],
    var_tys: &HashMap<VarId, Ty>,
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) {
    let named = |ty: Option<&Ty>, member: Option<&str>| {
        ty.zip(member)
            .and_then(|(ty, member)| declarations.struct_member(ty, member))
            .map(str::to_owned)
    };
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        match instruction {
            MirInstr::UseVar {
                var,
                mode,
                constructor,
                ..
            } => *constructor = named(var_tys.get(var), use_constructor_member(*mode)),
            MirInstr::CopyValue {
                dest, constructor, ..
            } => *constructor = named(reg_types.get(&dest.0), Some("__copyinit__")),
            _ => {}
        }
        for region in instruction_regions_mut(instruction) {
            name_in_blocks(region, var_tys, reg_types, declarations);
        }
    }
}
