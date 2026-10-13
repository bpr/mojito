//! Copy elision at a dying source: after drop elaboration, a named-struct
//! copy construction `Name(copy=v)` whose whole owned source `v` dies right
//! at it becomes `v`'s move, as Mojo's lifetime checker turns a copy
//! initialization followed by the source's destruction into a move.
//!
//! The rewrite keys on the `DropVar` drop elaboration placed directly after
//! the construction (or after its in-place binding): that drop is the proof
//! that `v` is dead there on every path, loans included. Read, `ref`, and
//! `deinit` slots get no such drop, and a field source is not a whole slot,
//! so neither is rewritten. A type parameter's `T(copy=x)` is still a
//! `ConstructTypeParam` here and stays a copy, as at Mojo. A source that dies
//! only on a successor edge keeps its copy.

use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_operand_regs;
use mojito_mir::mir::{MirBlock, MirDeclarations, MirInstr, MirPlace, MirTerm, UseMode};
use std::collections::HashMap;

/// Rewrite every dying named-struct copy construction in `blocks`, `Try`
/// regions included, into a move of its source.
pub(super) fn elide_dying_copies(blocks: &mut [MirBlock], declarations: &MirDeclarations) {
    let mut uses = HashMap::new();
    count_operand_uses(blocks, &mut uses);
    rewrite_blocks(blocks, declarations, &uses);
}

fn count_operand_uses(blocks: &[MirBlock], uses: &mut HashMap<u32, usize>) {
    let mut operands = Vec::new();
    for block in blocks {
        for instruction in &block.instrs {
            operands.clear();
            instruction_operand_regs(instruction, &mut operands);
            for register in &operands {
                *uses.entry(register.0).or_default() += 1;
            }
            for region in try_regions(instruction) {
                count_operand_uses(region, uses);
            }
        }
        if let MirTerm::Branch { cond: register, .. }
        | MirTerm::Return(Some(register))
        | MirTerm::ReturnWithCleanup {
            value: Some(register),
            ..
        } = &block.term
        {
            *uses.entry(register.0).or_default() += 1;
        }
    }
}

fn rewrite_blocks(
    blocks: &mut [MirBlock],
    declarations: &MirDeclarations,
    uses: &HashMap<u32, usize>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in try_regions_mut(instruction) {
                rewrite_blocks(region, declarations, uses);
            }
        }
        let mut at = 1;
        while at < block.instrs.len() {
            if let Some(source) = dying_copy(&block.instrs, at, declarations, uses) {
                elide(&mut block.instrs, at, source);
                at -= 1;
            }
            at += 1;
        }
    }
}

/// A dying copy construction rewritten into a move.
#[derive(Clone, Copy)]
struct DyingCopy {
    var: VarId,
    drop_at: usize,
    binding_at: Option<usize>,
}

/// The source slot and its death of the copy construction at `at`, when the
/// construction copies a whole slot loaded just before it and that slot's
/// `DropVar` follows it (or its in-place binding) in the run of drops.
fn dying_copy(
    instrs: &[MirInstr],
    at: usize,
    declarations: &MirDeclarations,
    uses: &HashMap<u32, usize>,
) -> Option<DyingCopy> {
    let MirInstr::Call {
        dest,
        func,
        raises: None,
        args,
        kwargs,
        ..
    } = &instrs[at]
    else {
        return None;
    };
    let [(keyword, operand)] = kwargs.as_slice() else {
        return None;
    };
    if keyword != "copy" || !args.is_empty() || declarations.structure(&func.0).is_none() {
        return None;
    }
    let MirInstr::LoadPlace {
        dest: loaded,
        place: MirPlace { root, proj, .. },
    } = &instrs[at - 1]
    else {
        return None;
    };
    if loaded != operand || !proj.is_empty() || uses.get(&operand.0) != Some(&1) {
        return None;
    }
    let var = *root;
    let binding_at = match instrs.get(at + 1) {
        Some(MirInstr::DefVar {
            src,
            in_place: true,
            ..
        }) if src == dest => Some(at + 1),
        _ => None,
    };
    let drop_at = instrs[binding_at.unwrap_or(at) + 1..]
        .iter()
        .take_while(|instruction| {
            matches!(
                instruction,
                MirInstr::DropVar { .. } | MirInstr::Drop { .. } | MirInstr::DropPlace { .. }
            )
        })
        .position(
            |instruction| matches!(instruction, MirInstr::DropVar { var: dying } if *dying == var),
        )?
        + binding_at.unwrap_or(at)
        + 1;
    Some(DyingCopy {
        var,
        drop_at,
        binding_at,
    })
}

/// Replace the construction at `at` with its source's move: the source's
/// death and the dead load go, and the binding is no longer in place.
fn elide(instrs: &mut Vec<MirInstr>, at: usize, source: DyingCopy) {
    instrs.remove(source.drop_at);
    if let Some(MirInstr::DefVar { in_place, .. }) = source
        .binding_at
        .and_then(|binding| instrs.get_mut(binding))
    {
        *in_place = false;
    }
    let MirInstr::Call { dest, .. } = instrs[at] else {
        return;
    };
    instrs[at] = MirInstr::UseVar {
        dest,
        var: source.var,
        mode: UseMode::Move,
    };
    instrs.remove(at - 1);
}

fn try_regions(instruction: &MirInstr) -> Vec<&[MirBlock]> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instruction
    else {
        return Vec::new();
    };
    std::iter::once(body.as_slice())
        .chain(handler.iter().map(|(_, blocks)| blocks.as_slice()))
        .chain(orelse.iter().map(Vec::as_slice))
        .chain(finalbody.iter().map(Vec::as_slice))
        .collect()
}

fn try_regions_mut(instruction: &mut MirInstr) -> Vec<&mut [MirBlock]> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instruction
    else {
        return Vec::new();
    };
    std::iter::once(body.as_mut_slice())
        .chain(handler.iter_mut().map(|(_, blocks)| blocks.as_mut_slice()))
        .chain(orelse.iter_mut().map(Vec::as_mut_slice))
        .chain(finalbody.iter_mut().map(Vec::as_mut_slice))
        .collect()
}
