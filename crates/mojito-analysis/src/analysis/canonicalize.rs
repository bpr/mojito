//! `canonicalize`: upstream's `Canonicalizer`, MLIR's `canonicalize` pass,
//! which its pre-elaboration pipeline runs after `EliminateDeadSymbols`.
//!
//! This is its region simplification and dead-op erasure: blocks the entry
//! does not reach go, as MLIR's `eraseUnreachableBlocks` erases them, and an
//! instruction whose results nothing reads goes when it has no effect but
//! reading ([`MirInstr::would_be_trivially_dead`]), repeated until nothing
//! changes, as MLIR's greedy driver does. Folding patterns join this pass.

use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, instruction_regs_mut,
    prune_unreachable_blocks, terminator_regs_mut,
};
use std::collections::HashSet;

/// The pass, under MLIR's flag spelling.
pub const CANONICALIZE: BodyPass = BodyPass {
    name: "canonicalize",
    run: canonicalize,
};

/// Erase `function`'s unreachable blocks, then every trivially dead
/// instruction, until none is left. An erased instruction's results lose
/// their types and spans; registers are not renumbered.
pub fn canonicalize(_name: &str, function: &mut MirFunction, _declarations: &MirDeclarations) {
    let blocks = block_count(&mut function.blocks);
    prune_unreachable_blocks(function);
    timing::count(
        "erased_blocks",
        (blocks - block_count(&mut function.blocks)) as u64,
    );
    let mut erased = HashSet::new();
    loop {
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_reads(&mut function.blocks, &mut reads);
        let before = erased.len();
        erase_dead(&mut function.blocks, &reads, &mut erased);
        if erased.len() == before {
            break;
        }
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    timing::count("erased_instructions", erased.len() as u64);
}

/// The block lists of a `try` instruction's regions, mutably.
fn regions_mut(instruction: &mut MirInstr) -> Vec<&mut Vec<MirBlock>> {
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
    std::iter::once(body)
        .chain(handler.as_mut().map(|(_, blocks)| blocks))
        .chain(orelse.as_mut())
        .chain(finalbody.as_mut())
        .collect()
}

/// Every block of `blocks` and of the `try` regions they hold.
fn block_count(blocks: &mut [MirBlock]) -> usize {
    blocks
        .iter_mut()
        .map(|block| {
            1 + block
                .instrs
                .iter_mut()
                .flat_map(regions_mut)
                .map(|region| block_count(region))
                .sum::<usize>()
        })
        .sum()
}

/// Add every register read in `blocks`, their regions included, to
/// `reads`. Each register an instruction names counts as a read but its
/// own results, so an operand no enumeration classifies is kept alive.
fn count_reads(blocks: &mut [MirBlock], reads: &mut [u32]) {
    let count = |reads: &mut [u32], reg: &Reg| {
        if let Some(count) = reads.get_mut(reg.0 as usize) {
            *count += 1;
        }
    };
    for block in blocks {
        for instruction in &mut block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            for reg in instruction_regs_mut(instruction) {
                match results.iter().position(|result| result == reg) {
                    Some(at) => {
                        results.swap_remove(at);
                    }
                    None => count(reads, reg),
                }
            }
            for region in regions_mut(instruction) {
                count_reads(region, reads);
            }
        }
        for reg in terminator_regs_mut(&mut block.term) {
            count(reads, reg);
        }
    }
}

/// Remove each instruction of `blocks` and their regions that would be
/// trivially dead and defines only unread results, adding them to `erased`.
fn erase_dead(blocks: &mut [MirBlock], reads: &[u32], erased: &mut HashSet<u32>) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in regions_mut(instruction) {
                erase_dead(region, reads, erased);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            let dead = instruction.would_be_trivially_dead()
                && !results.is_empty()
                && results
                    .iter()
                    .all(|reg| reads.get(reg.0 as usize) == Some(&0));
            if dead {
                erased.extend(results.iter().map(|reg| reg.0));
            }
            !dead
        });
    }
}
