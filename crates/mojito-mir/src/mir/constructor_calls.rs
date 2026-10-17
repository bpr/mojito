//! Constructor naming: upstream's copy/move insertion naming the
//! `__copyinit__` or `__moveinit__` at each copy or move site.
//!
//! Each `var.use copy`, `var.use move`, `value.copy`, owned closure capture,
//! variant payload store (`variant.make`, `variant.set`, `variant.replace`),
//! and reference-read method or iterator result (`result_adapter`) names
//! the constructor its value's struct records as its `constructor`
//! operand, so the symbol walk, the elaborator's respell, and
//! `eliminate-dead-symbols` follow it as they follow a call's callee, and no
//! backend looks the constructor up from the value's type again. A site
//! whose type the member table names no such member for, a symbolic one
//! included, keeps `None`: the elaborator names an instance's constructor
//! once its type is concrete. Naming is a function of the site's type
//! alone, so a pass that makes or retypes a copy or move names it again by
//! rerunning this walk.

use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, MirPlace, UseMode, instruction_regions_mut,
};
use mojito_hir::hir::VarId;
use mojito_types::types::{Ty, TyKind, VariantIndex};
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

/// The type of the payload a variant store selects by `index` among
/// `alternatives`; none for an index only an instantiation resolves.
pub fn variant_payload_ty<'a>(alternatives: &'a [Ty], index: &VariantIndex) -> Option<&'a Ty> {
    alternatives.get(index.known()?)
}

/// The type of the payload a store into the variant at `place` selects by
/// `index`, read from the place's storage type.
pub fn variant_place_payload_ty<'a, S: std::hash::BuildHasher>(
    place: &'a MirPlace,
    index: &VariantIndex,
    var_tys: &'a HashMap<VarId, Ty, S>,
) -> Option<&'a Ty> {
    variant_place_alternatives(place, var_tys)
        .and_then(|alternatives| variant_payload_ty(alternatives, index))
}

/// The alternatives of the variant at `place`, read from the place's
/// storage type; `None` where that type is not a known variant.
pub fn variant_place_alternatives<'a, S: std::hash::BuildHasher>(
    place: &'a MirPlace,
    var_tys: &'a HashMap<VarId, Ty, S>,
) -> Option<&'a [Ty]> {
    match place.storage_ty(var_tys)?.kind() {
        TyKind::Variant(alternatives) => Some(alternatives.as_slice()),
        _ => None,
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
    let copied = |ty: Option<&Ty>| named(ty, Some("__copyinit__"));
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
            } => *constructor = copied(reg_types.get(&dest.0)),
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    capture.constructor = named(
                        capture.place.storage_ty(var_tys),
                        capture.constructor_member(),
                    );
                }
            }
            MirInstr::MakeVariant {
                alternatives,
                index,
                constructor,
                ..
            } => *constructor = copied(variant_payload_ty(alternatives, index)),
            MirInstr::VariantSet {
                place,
                index,
                constructor,
                ..
            }
            | MirInstr::VariantReplace {
                place,
                input_index: index,
                constructor,
                ..
            } => *constructor = copied(variant_place_payload_ty(place, index, var_tys)),
            MirInstr::MethodCall {
                dest,
                result_adapter: Some(adapter),
                ..
            } => adapter.constructor = copied(reg_types.get(&dest.0)),
            MirInstr::TryNext {
                dest,
                call,
                adapter_constructor,
                ..
            } => {
                *adapter_constructor = copied(
                    reg_types
                        .get(&dest.0)
                        .filter(|_| call.result_adapter.is_some()),
                );
            }
            _ => {}
        }
        for region in instruction_regions_mut(instruction) {
            name_in_blocks(region, var_tys, reg_types, declarations);
        }
    }
}
