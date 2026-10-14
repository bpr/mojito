//! `canonicalize`: upstream's `Canonicalizer`, MLIR's `canonicalize` pass,
//! which its pre-elaboration pipeline runs after `EliminateDeadSymbols`.
//!
//! This is its region simplification, folding, and dead-op erasure: blocks
//! the entry does not reach go, as MLIR's `eraseUnreachableBlocks` erases
//! them; an instruction that folds to one of its operands
//! ([`MirInstr::forwarded_operand`], an identity `value.copy`) is replaced
//! by that operand in every reader, as MLIR's greedy driver applies a `fold`
//! that returns an operand; and an instruction whose results nothing reads
//! goes when it has no effect but reading
//! ([`MirInstr::would_be_trivially_dead`]), repeated until nothing changes.

use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    MirBlock, MirDeclarations, MirFunction, MirInstr, Reg, instruction_regs_mut,
    prune_unreachable_blocks, terminator_regs_mut,
};
use mojito_types::types::Ty;
use std::collections::{HashMap, HashSet};

/// The pass, under MLIR's flag spelling.
pub const CANONICALIZE: BodyPass = BodyPass {
    name: "canonicalize",
    run: canonicalize,
};

/// Erase `function`'s unreachable blocks, forward each identity copy's
/// operand to its readers, then erase every trivially dead instruction.
///
/// Erasure repeats until none is left. A removed instruction's results lose their types and spans;
/// registers are not renumbered.
pub fn canonicalize(_name: &str, function: &mut MirFunction, _declarations: &MirDeclarations) {
    let blocks = block_count(&mut function.blocks);
    prune_unreachable_blocks(function);
    timing::count(
        "erased_blocks",
        (blocks - block_count(&mut function.blocks)) as u64,
    );
    timing::count("forwarded_copies", forward_copies(function) as u64);
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

/// Replace each register an instruction folds to an operand
/// ([`MirInstr::forwarded_operand`]) by that operand, chains resolved to
/// their root, and remove the folding instructions; returns how many went.
///
/// Only a result with one definition whose operand has at most one is
/// forwarded, so every other occurrence of the result is a read the
/// operand's definition dominates.
fn forward_copies(function: &mut MirFunction) -> usize {
    let MirFunction {
        blocks,
        reg_types,
        n_regs,
        ..
    } = function;
    let mut definitions = vec![0_u32; *n_regs as usize];
    let mut folds = HashMap::new();
    collect_folds(blocks, reg_types, &mut definitions, &mut folds);
    let single = |reg: u32, most: u32| definitions.get(reg as usize).is_some_and(|n| *n <= most);
    folds.retain(|dest: &u32, operand: &mut Reg| single(*dest, 1) && single(operand.0, 1));
    let roots: HashMap<u32, Reg> = folds
        .keys()
        .filter_map(|dest| {
            let mut root = folds[dest];
            for _ in 0..folds.len() {
                match folds.get(&root.0) {
                    Some(next) => root = *next,
                    None => return Some((*dest, root)),
                }
            }
            None
        })
        .collect();
    forward(blocks, &roots);
    reg_types.retain(|reg, _| !roots.contains_key(reg));
    function.spans.0.retain(|reg, _| !roots.contains_key(reg));
    roots.len()
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

/// Count each register's definitions in `blocks` and their regions, and map
/// each folding instruction's result to the operand it folds to.
fn collect_folds(
    blocks: &mut [MirBlock],
    reg_types: &HashMap<u32, Ty>,
    definitions: &mut [u32],
    folds: &mut HashMap<u32, Reg>,
) {
    for block in blocks {
        for instruction in &mut block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            for reg in &results {
                if let Some(count) = definitions.get_mut(reg.0 as usize) {
                    *count += 1;
                }
            }
            if let (Some(operand), [dest]) =
                (instruction.forwarded_operand(reg_types), results.as_slice())
            {
                folds.insert(dest.0, operand);
            }
            for region in regions_mut(instruction) {
                collect_folds(region, reg_types, definitions, folds);
            }
        }
    }
}

/// Remove the instructions defining `roots`' keys from `blocks` and their
/// regions, and rewrite every read of a key to its root.
fn forward(blocks: &mut [MirBlock], roots: &HashMap<u32, Reg>) {
    let rewrite = |reg: &mut Reg| {
        if let Some(root) = roots.get(&reg.0) {
            *reg = *root;
        }
    };
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in regions_mut(instruction) {
                forward(region, roots);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if results.iter().any(|reg| roots.contains_key(&reg.0)) {
                return false;
            }
            instruction_regs_mut(instruction)
                .into_iter()
                .for_each(rewrite);
            true
        });
        terminator_regs_mut(&mut block.term)
            .into_iter()
            .for_each(rewrite);
    }
}
