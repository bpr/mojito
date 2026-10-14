//! `canonicalize`: upstream's `Canonicalizer`, MLIR's `canonicalize` pass,
//! which its pre-elaboration pipeline runs after `EliminateDeadSymbols`.
//!
//! This is its region simplification, folding, and dead-op erasure: a
//! `branch` on a constant condition becomes a `jump` to the taken arm, as
//! `hlcf.if`'s `RemoveStaticCondition` pattern keeps the arm a static
//! condition takes; blocks the entry does not reach go, as MLIR's
//! `eraseUnreachableBlocks` erases them; an instruction that folds to one of
//! its operands ([`MirInstr::forwarded_operand`], an identity `value.copy`)
//! is replaced by that operand in every reader, as MLIR's greedy driver
//! applies a `fold` that returns an operand; and an instruction whose
//! results nothing reads goes when it has no effect but reading
//! ([`MirInstr::would_be_trivially_dead`]), repeated until nothing changes.
//!
//! Upstream's generators keep run-time control flow as structured `hlcf`
//! region ops until `LowerControlFlow`, after elaboration, so a join or a
//! loop latch is no block there. MIR is a CFG before elaboration, and the
//! blocks that structure would not have are removed here by MLIR's own CFG
//! rewrites: an edge into an empty block that only jumps on is threaded to
//! where it goes (`SimplifyPassThroughBr`), a branch whose arms name one
//! block becomes a jump (`SimplifyCondBranchIdenticalSuccessors`, as
//! `hlcf.if`'s `HoistYieldResults` and `comptime.if`'s `checkTrivialBlocks`
//! erase an `if` whose arms are bare yields), and a block a jump alone
//! reaches is merged into its predecessor
//! (`SimplifyBrToBlockWithSinglePred`). A `comptime for` header's targets
//! are never rewritten: the elaborator finds the loop body as the blocks its
//! `body` target dominates.

use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg,
    block_successors, instruction_regs_mut, prune_unreachable_blocks, terminator_regs_mut,
    terminator_targets, terminator_targets_mut,
};
use mojito_types::types::copy_is_identity;
use std::collections::{HashMap, HashSet};

/// The pass, under MLIR's flag spelling.
pub const CANONICALIZE: BodyPass = BodyPass {
    name: "canonicalize",
    run: canonicalize,
};

/// Fold each `branch` on a constant, simplify the control flow, erase
/// unreachable blocks, forward identity copies, and erase dead
/// instructions of `function`.
///
/// Erasure and control-flow simplification repeat until neither changes
/// anything: an erased instruction can empty a block, and a branch folded
/// to a jump leaves its condition unread. A removed instruction's results
/// lose their types and spans; registers are not renumbered.
pub fn canonicalize(_name: &str, function: &mut MirFunction, declarations: &MirDeclarations) {
    timing::count("folded_branches", fold_constant_branches(function) as u64);
    let blocks = block_count(&mut function.blocks);
    let mut edits = simplify_control_flow(&mut function.blocks, true);
    prune_unreachable_blocks(function);
    timing::count(
        "forwarded_copies",
        forward_copies(function, declarations) as u64,
    );
    let mut erased = HashSet::new();
    loop {
        let before = erased.len();
        erase_dead_until_none(function, &mut erased);
        if erased.len() == before {
            break;
        }
        let round = simplify_control_flow(&mut function.blocks, true);
        if round == ControlFlowEdits::default() {
            break;
        }
        edits.add(round);
        prune_unreachable_blocks(function);
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    timing::count(
        "erased_blocks",
        (blocks - block_count(&mut function.blocks)) as u64,
    );
    timing::count("threaded_branches", edits.threaded as u64);
    timing::count("folded_successors", edits.folded as u64);
    timing::count("merged_blocks", edits.merged as u64);
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

/// How many edges [`simplify_control_flow`] threaded, branches it folded to
/// a jump, and blocks it merged into their predecessor.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ControlFlowEdits {
    threaded: usize,
    folded: usize,
    merged: usize,
}

impl ControlFlowEdits {
    const fn add(&mut self, other: Self) {
        self.threaded += other.threaded;
        self.folded += other.folded;
        self.merged += other.merged;
    }
}

/// Thread the edges into empty pass-through blocks, fold each branch whose
/// arms name one block to a jump, and merge each block a jump alone reaches
/// into its predecessor, in `blocks` and every `try` region below them,
/// until none applies. `function_level` says `blocks` is a function's list,
/// whose blocks regions' escapes name too.
///
/// The blocks left unreachable stay for the pruning to erase. Only an
/// unconditional jump's target is merged and only an empty block is
/// threaded past, so an edge's drops and an escape's or return's cleanup
/// stay on their path.
fn simplify_control_flow(blocks: &mut [MirBlock], function_level: bool) -> ControlFlowEdits {
    let mut edits = ControlFlowEdits::default();
    for region in blocks
        .iter_mut()
        .flat_map(|block| block.instrs.iter_mut().flat_map(regions_mut))
    {
        edits.add(simplify_control_flow(region, false));
    }
    loop {
        let round = ControlFlowEdits {
            threaded: thread_pass_throughs(blocks),
            folded: fold_identical_successors(blocks),
            merged: merge_single_predecessors(blocks, function_level),
        };
        if round == ControlFlowEdits::default() {
            return edits;
        }
        edits.add(round);
    }
}

/// Retarget each `jump`, `branch`, and `comptime_branch` edge of `blocks`
/// into a pass-through block at the first block past the chain that is not
/// one; returns how many edges moved. An edge that would make a
/// pass-through jump to itself, or that enters a cycle of pass-throughs,
/// stays.
fn thread_pass_throughs(blocks: &mut [MirBlock]) -> usize {
    let mut threaded = 0;
    for at in 0..blocks.len() {
        let targets: Vec<MirBlockId> = match &blocks[at].term {
            MirTerm::Jump(_) | MirTerm::Branch { .. } | MirTerm::ComptimeBranch { .. } => {
                terminator_targets(&blocks[at].term)
            }
            _ => continue,
        };
        let resolved: Vec<MirBlockId> = targets
            .iter()
            .map(|&target| match resolve_pass_through(blocks, target) {
                end if end == at && pass_through(blocks, at).is_some() => target,
                end => end,
            })
            .collect();
        for (target, end) in terminator_targets_mut(&mut blocks[at].term)
            .into_iter()
            .zip(resolved)
        {
            if *target != end {
                *target = end;
                threaded += 1;
            }
        }
    }
    threaded
}

/// The block an empty block of `blocks` only jumps on to, when `at` is one
/// and the jump is not to itself.
fn pass_through(blocks: &[MirBlock], at: MirBlockId) -> Option<MirBlockId> {
    match blocks.get(at)? {
        MirBlock {
            instrs,
            term: MirTerm::Jump(next),
        } if instrs.is_empty() && *next != at => Some(*next),
        _ => None,
    }
}

/// The first block from `start` along a chain of pass-through blocks that
/// is not one, or the block where the chain closes a cycle.
fn resolve_pass_through(blocks: &[MirBlock], start: MirBlockId) -> MirBlockId {
    let mut seen = HashSet::from([start]);
    let mut at = start;
    while let Some(next) = pass_through(blocks, at) {
        if !seen.insert(next) {
            break;
        }
        at = next;
    }
    at
}

/// Rewrite each `branch` and `comptime_branch` of `blocks` whose arms name
/// one block to a `jump` there; returns how many were rewritten.
fn fold_identical_successors(blocks: &mut [MirBlock]) -> usize {
    let mut folded = 0;
    for block in blocks {
        if let MirTerm::Branch { then_b, else_b, .. }
        | MirTerm::ComptimeBranch { then_b, else_b, .. } = block.term
            && then_b == else_b
        {
            block.term = MirTerm::Jump(then_b);
            folded += 1;
        }
    }
    folded
}

/// Append to each reachable block of `blocks` ending in a `jump` the block
/// it jumps to, when that block is not the entry and nothing else reaches
/// it; returns how many were merged. A merged block is left empty and
/// returning, unreachable.
fn merge_single_predecessors(blocks: &mut [MirBlock], function_level: bool) -> usize {
    let successors = |block: &MirBlock| {
        if function_level {
            block_successors(block)
        } else {
            terminator_targets(&block.term)
        }
    };
    let mut reachable = vec![false; blocks.len()];
    let mut work = vec![0];
    while let Some(at) = work.pop() {
        if at < blocks.len() && !reachable[at] {
            reachable[at] = true;
            work.extend(successors(&blocks[at]));
        }
    }
    let mut predecessors = vec![0_usize; blocks.len()];
    for (block, _) in blocks.iter().zip(&reachable).filter(|(_, live)| **live) {
        for successor in successors(block) {
            if let Some(count) = predecessors.get_mut(successor) {
                *count += 1;
            }
        }
    }
    let mut merged = 0;
    for at in (0..blocks.len()).filter(|&at| reachable[at]) {
        while let MirTerm::Jump(next) = blocks[at].term
            && next != at
            && next != 0
            && predecessors.get(next) == Some(&1)
        {
            let MirBlock { instrs, term } = std::mem::replace(
                &mut blocks[next],
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Return(None),
                },
            );
            blocks[at].instrs.extend(instrs);
            blocks[at].term = term;
            predecessors[next] = 0;
            merged += 1;
        }
    }
    merged
}

/// Rewrite each `branch` of `function` whose condition is defined once, by
/// a `const bool`, to a `jump` to the arm it takes; returns how many were
/// rewritten. The condition's `const`, once unread, is erased as dead.
fn fold_constant_branches(function: &mut MirFunction) -> usize {
    let definitions = definition_counts(&mut function.blocks, function.n_regs);
    let mut conditions = HashMap::new();
    visit_blocks(&mut function.blocks, &mut |block| {
        for instruction in &block.instrs {
            if let MirInstr::Const {
                dest,
                k: Const::Bool(value),
            } = instruction
                && definitions.get(dest.0 as usize) == Some(&1)
            {
                conditions.insert(dest.0, *value);
            }
        }
    });
    let mut folded = 0;
    visit_blocks(&mut function.blocks, &mut |block| {
        if let MirTerm::Branch {
            cond,
            then_b,
            else_b,
        } = block.term
            && let Some(taken) = conditions.get(&cond.0)
        {
            block.term = MirTerm::Jump(if *taken { then_b } else { else_b });
            folded += 1;
        }
    });
    folded
}

/// How many times `blocks` and their regions define each of `n_regs`
/// registers.
fn definition_counts(blocks: &mut [MirBlock], n_regs: u32) -> Vec<u32> {
    let mut definitions = vec![0_u32; n_regs as usize];
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            for reg in &results {
                if let Some(count) = definitions.get_mut(reg.0 as usize) {
                    *count += 1;
                }
            }
        }
    });
    definitions
}

/// Call `visit` on every block of `blocks`, then on the blocks of the `try`
/// regions each holds.
fn visit_blocks(blocks: &mut [MirBlock], visit: &mut impl FnMut(&mut MirBlock)) {
    for block in blocks {
        visit(block);
        for region in block.instrs.iter_mut().flat_map(regions_mut) {
            visit_blocks(region, visit);
        }
    }
}

/// Replace each register an instruction folds to an operand
/// ([`MirInstr::forwarded_operand`]) by that operand, chains resolved to
/// their root, and remove the folding instructions; returns how many went.
///
/// Only a result with one definition whose operand has at most one is
/// forwarded, so every other occurrence of the result is a read the
/// operand's definition dominates. A copy of a value held in storage rather
/// than one register (a register-passable trivial aggregate) is forwarded
/// only when it is its operand's sole reader: a reader may consume such a
/// value (an owned argument, an `UninitStorage` initialization), so the
/// forwarded value keeps exactly the copy's readers.
fn forward_copies(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let MirFunction {
        blocks,
        reg_types,
        n_regs,
        ..
    } = function;
    let definitions = definition_counts(blocks, *n_regs);
    let mut reads = vec![0_u32; *n_regs as usize];
    count_reads(blocks, &mut reads);
    let mut folds = HashMap::new();
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if let (Some(operand), [dest]) = (
                instruction.forwarded_operand(reg_types, declarations),
                results.as_slice(),
            ) {
                folds.insert(dest.0, operand);
            }
        }
    });
    let single = |reg: u32, most: u32| definitions.get(reg as usize).is_some_and(|n| *n <= most);
    let in_register = |reg: &Reg| {
        reg_types
            .get(&reg.0)
            .is_some_and(|ty| copy_is_identity(ty.kind()))
    };
    let sole_reader = |reg: &Reg| reads.get(reg.0 as usize) == Some(&1);
    folds.retain(|dest: &u32, operand: &mut Reg| {
        single(*dest, 1) && single(operand.0, 1) && (in_register(operand) || sole_reader(operand))
    });
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

/// Erase the trivially dead instructions of `function` until none is left,
/// adding their results to `erased`.
fn erase_dead_until_none(function: &mut MirFunction, erased: &mut HashSet<u32>) {
    loop {
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_reads(&mut function.blocks, &mut reads);
        let before = erased.len();
        erase_dead(&mut function.blocks, &reads, erased);
        if erased.len() == before {
            break;
        }
    }
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
