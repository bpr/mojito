//! Constructor naming: upstream's copy/move insertion naming the
//! `__copyinit__` or `__moveinit__` at each copy or move site.
//!
//! Each `var.use copy`, `var.use move`, `value.copy`, owned closure capture,
//! variant payload store (`variant.make`, `variant.set`, `variant.replace`,
//! each a move), and reference-read method or iterator result
//! (`result_adapter`) names the constructor its value's struct records as
//! its `constructor` operand, and each call naming a struct over one `copy=` keyword
//! (`S(copy=x)` at a struct whose copy initializer is synthesized) becomes
//! that struct's copy ([`crate::mir::struct_copy`]), so the symbol walk, the elaborator's respell, and
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
use mojito_types::types::{Ty, TyKind, VariantIndex, struct_template};
use std::collections::{HashMap, HashSet};

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
    let moved = |ty: Option<&Ty>| named(ty, Some("__moveinit__"));
    // A copy that is no call leaves its binding no longer in place.
    let mut copied_values = HashSet::new();
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        if let Some(copy) = struct_named_copy(instruction, reg_types, declarations) {
            if let MirInstr::CopyValue { dest, .. } = &copy {
                copied_values.insert(dest.0);
            }
            *instruction = copy;
        }
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
            MirInstr::DefVar { src, in_place, .. } if copied_values.contains(&src.0) => {
                *in_place = false;
            }
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
            } => *constructor = moved(variant_payload_ty(alternatives, index)),
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
            } => *constructor = moved(variant_place_payload_ty(place, index, var_tys)),
            MirInstr::MethodCall {
                dest,
                result_adapter: Some(adapter),
                ..
            } => adapter.constructor = copied(reg_types.get(&dest.0)),
            _ => {}
        }
        for region in instruction_regions_mut(instruction) {
            name_in_blocks(region, var_tys, reg_types, declarations);
        }
    }
}

/// The copy a call naming the struct it builds over one `copy=` keyword
/// (`S(copy=x)`) is; `None` for any other instruction.
fn struct_named_copy(
    instruction: &MirInstr,
    reg_types: &HashMap<u32, Ty>,
    declarations: &MirDeclarations,
) -> Option<MirInstr> {
    let MirInstr::Call {
        dest,
        func,
        args,
        kwargs,
        kwarg_places,
        ..
    } = instruction
    else {
        return None;
    };
    let [(keyword, source)] = kwargs.as_slice() else {
        return None;
    };
    let ty = reg_types.get(&dest.0)?;
    let TyKind::Struct(name, _) = ty.kind() else {
        return None;
    };
    (keyword == "copy"
        && args.is_empty()
        && struct_template(name) == struct_template(&func.0)
        && declarations.structure(struct_template(name)).is_some())
    .then(|| {
        crate::mir::struct_copy(
            *dest,
            ty,
            (*source, kwarg_places.first().cloned().flatten()),
            declarations,
        )
    })
}
