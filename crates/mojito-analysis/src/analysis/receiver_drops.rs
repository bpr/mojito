//! A partly built `self` at a raise: drop elaboration ends an initializer's
//! `out self` receiver wherever a path leaves without returning it
//! ([`elaborate_drops`]), and this pass turns each such `DropVar` into what
//! the receiver holds there, as upstream's `DestructorInsertion` destroys a
//! partially initialized `self` field by field.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::instruction_regions_mut;
use mojito_types::types::{Ty, TyKind};

/// Rewrite each `DropVar` of an initializer's receiver in function `name`
/// by the receiver's initialization state reaching it: a wholly initialized
/// receiver stays one drop, which runs its destructor; a partly initialized
/// one becomes a `DropPlace` per field set on every path that needs one, in
/// declaration order; an uninitialized one drops nothing.
pub(super) fn refine_receiver_drops(
    f: &mut MirFunction,
    name: &str,
    declarations: &MirDeclarations,
) {
    let Some(receiver) = initializer_receiver_entry(f, name, declarations) else {
        return;
    };
    let fields = match f.var_tys.get(&0).map(Ty::kind) {
        Some(TyKind::Struct(struct_name, _)) => declarations
            .structure(struct_name)
            .map(|decl| decl.fields.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let root_ty = f.var_tys.get(&0).cloned();
    let mut ends: HashMap<*const MirInstr, Vec<MirInstr>> = HashMap::new();
    let observed = observe_move_states(
        f,
        entry_move_states(f, Some(receiver)),
        &mut |state, instr| {
            if matches!(instr, MirInstr::DropVar { var: 0, .. }) {
                ends.insert(
                    std::ptr::from_ref(instr),
                    receiver_end(&state[0], &fields, root_ty.as_ref()),
                );
            }
        },
    );
    if observed.is_err() {
        ends.clear();
    }
    rewrite_blocks(&mut f.blocks, &ends);
}

/// What ends a receiver in state `node`: itself whole, when every field is
/// initialized; otherwise a `DropPlace` of each field it holds on every
/// path whose type needs one, in declaration order.
fn receiver_end(node: &Node, fields: &[(String, Ty)], root_ty: Option<&Ty>) -> Vec<MirInstr> {
    if node.whole().0 == Own::Owned {
        return vec![MirInstr::DropVar {
            var: 0,
            destructor: None,
        }];
    }
    fields
        .iter()
        .filter(|(field, ty)| {
            field_needs_drop(ty) && node.read(&[Key::Field(field.clone())]).0 == Own::Owned
        })
        .map(|(field, ty)| {
            let mut place = MirPlace::root(0, root_ty.cloned());
            place.project(Proj::Field(field.clone()), ty.clone());
            MirInstr::DropPlace {
                place,
                destructor: None,
            }
        })
        .collect()
}

/// Replace each receiver drop in `ends` by its end, recursing into every
/// region op's regions. A drop is looked up while it is still in place,
/// before its block is rebuilt; a drop no state reached ends nothing.
fn rewrite_blocks(blocks: &mut [MirBlock], ends: &HashMap<*const MirInstr, Vec<MirInstr>>) {
    for block in blocks {
        let replaced: Vec<Option<Vec<MirInstr>>> = block
            .instrs
            .iter()
            .map(|instr| {
                matches!(instr, MirInstr::DropVar { var: 0, .. }).then(|| {
                    ends.get(&std::ptr::from_ref(instr))
                        .cloned()
                        .unwrap_or_default()
                })
            })
            .collect();
        for instr in &mut block.instrs {
            for region in instruction_regions_mut(instr) {
                rewrite_blocks(region, ends);
            }
        }
        let instrs = std::mem::take(&mut block.instrs);
        block.instrs = instrs
            .into_iter()
            .zip(replaced)
            .flat_map(|(instr, end)| end.unwrap_or_else(|| vec![instr]))
            .collect();
    }
}
