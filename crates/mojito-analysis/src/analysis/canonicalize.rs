//! `canonicalize`: upstream's `Canonicalizer`, MLIR's `canonicalize` pass,
//! which its pre-elaboration pipeline runs after `EliminateDeadSymbols`.
//!
//! This is its folding, region simplification, and dead-op erasure: an
//! instruction [`MirInstr::folded_constant`] folds over operands defined
//! once by a constant becomes that `const`, as MLIR's greedy driver applies
//! a `fold` that returns an attribute (through the replacement `sccp`
//! shares, `fold::replace_folded`); blocks the entry does not reach go, as
//! MLIR's `eraseUnreachableBlocks` erases them; an instruction that folds to
//! one of its operands ([`MirInstr::forwarded_operand`], an identity
//! `value.copy`, or a copy of a closure of reference captures, a struct of
//! pointers) is replaced by that operand in every reader, as MLIR's greedy
//! driver applies a `fold` that returns an operand; and an instruction
//! whose results nothing reads goes when it has no effect but reading
//! ([`MirInstr::would_be_trivially_dead`]), repeated until nothing changes.
//!
//! A structured body's region ops take upstream's `hlcf` canonicalization
//! patterns (`hlcf`): an `if` on a constant keeps the arm it selects, exits
//! every arm shares are hoisted after the `if`, arms that only yield fold
//! into the results, a loop that only breaks goes, and a `comptime if` on a
//! constant keeps its live arm. Folding and the patterns alternate until
//! the patterns stop, as the greedy driver's worklist does.
//!
//! A lowered body, which the post-elaboration pipeline still canonicalizes,
//! takes MLIR's own CFG rewrites in their place: a `branch` on a constant
//! condition becomes a `jump` to the taken arm, an edge into an empty block
//! that only jumps on is threaded to where it goes (`SimplifyPassThroughBr`),
//! a branch whose arms name one block becomes a jump
//! (`SimplifyCondBranchIdenticalSuccessors`), and a block a jump alone
//! reaches is merged into its predecessor
//! (`SimplifyBrToBlockWithSinglePred`). A `comptime for` header's targets
//! are never rewritten: the elaborator finds the loop body as the blocks its
//! `body` target dominates. Neither set applies to the other's shape.

mod hlcf;

use super::fold;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, MirBlock, MirBlockId, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg,
    block_successors, count_register_reads, instruction_regions_mut, instruction_regs_mut,
    prune_unreachable_blocks, terminator_regs_mut, terminator_targets, terminator_targets_mut,
};
use mojito_types::types::{Ty, TyKind};
use std::collections::{HashMap, HashSet};

/// The pass, under MLIR's flag spelling.
pub const CANONICALIZE: BodyPass = BodyPass {
    name: "canonicalize",
    run: canonicalize,
};

/// Fold constants and constant branches, simplify the control flow, and
/// erase what `function` no longer reaches or reads.
///
/// Each instruction over constant operands becomes a `const`, each region
/// op the `hlcf` patterns match is rewritten, and each `branch` on a
/// constant becomes a `jump`; unreachable blocks go, identity copies are
/// forwarded, and dead instructions are erased. Constants fold first, so a
/// comparison folded to a `const bool` decides its `if` or `branch` in the
/// same run. Erasure, the patterns, and control-flow simplification repeat
/// until none changes anything: an erased instruction can empty a block or
/// an arm, and a branch folded to a jump leaves its condition unread. A removed instruction's results
/// lose their types and spans; registers are not renumbered. Returns
/// whether any of these rewrites applied.
pub fn canonicalize(
    _name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let mut erased = HashSet::new();
    let mut folded_constants = 0;
    let mut patterns = 0;
    loop {
        folded_constants += fold_constants(function, declarations);
        let rewritten = hlcf::hlcf_patterns(function, &mut erased);
        if rewritten == 0 {
            break;
        }
        patterns += rewritten;
    }
    let folded_branches = fold_constant_branches(function);
    timing::count("folded_constants", folded_constants as u64);
    timing::count("folded_branches", folded_branches as u64);
    let blocks = block_count(&mut function.blocks);
    let mut edits = simplify_control_flow(&mut function.blocks, true);
    prune_unreachable_blocks(function);
    // A fold can expose another, as a struct copy whose fields were copies
    // does once they are forwarded: the greedy driver folds to a fixpoint.
    let mut forwarded_copies = 0;
    loop {
        let forwarded = forward_copies(function, declarations);
        if forwarded == 0 {
            break;
        }
        forwarded_copies += forwarded;
    }
    timing::count("forwarded_copies", forwarded_copies as u64);
    loop {
        let before = erased.len();
        erase_dead_until_none(function, &mut erased);
        if erased.len() == before {
            break;
        }
        let rewritten = hlcf::hlcf_patterns(function, &mut erased);
        patterns += rewritten;
        let round = simplify_control_flow(&mut function.blocks, true);
        if round == ControlFlowEdits::default() && rewritten == 0 {
            break;
        }
        edits.add(round);
        prune_unreachable_blocks(function);
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    let erased_blocks = blocks - block_count(&mut function.blocks);
    timing::count("erased_blocks", erased_blocks as u64);
    timing::count("threaded_branches", edits.threaded as u64);
    timing::count("folded_successors", edits.folded as u64);
    timing::count("merged_blocks", edits.merged as u64);
    timing::count("erased_instructions", erased.len() as u64);
    folded_constants
        + patterns
        + folded_branches
        + forwarded_copies
        + erased_blocks
        + edits.threaded
        + edits.folded
        + edits.merged
        + erased.len()
        > 0
}

/// How many times `blocks` and their regions define each of `n_regs`
/// registers.
pub(super) fn definition_counts(blocks: &mut [MirBlock], n_regs: u32) -> Vec<u32> {
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
        .flat_map(|block| block.instrs.iter_mut().flat_map(instruction_regions_mut))
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

/// Replace each instruction of `function` that [`MirInstr::folded_constant`]
/// folds over operands defined once by a folded `const`
/// ([`Const::is_folded`]) by that `const`, until none folds; returns how
/// many were replaced.
///
/// One definition makes every read of an operand see its constant; a folded
/// instruction's own result may have several, as replacing one definition
/// by the constant it computes is exact. Each round folds one level of a
/// chain, the fixpoint MLIR's greedy driver reaches through its worklist.
fn fold_constants(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let mut folded = 0;
    loop {
        let definitions = definition_counts(&mut function.blocks, function.n_regs);
        let mut constants = HashMap::new();
        visit_blocks(&mut function.blocks, &mut |block| {
            for instruction in &block.instrs {
                if let MirInstr::Const { dest, k } = instruction
                    && k.is_folded()
                    && definitions.get(dest.0 as usize) == Some(&1)
                {
                    constants.insert(dest.0, k.clone());
                }
            }
        });
        let constant = |reg: Reg| constants.get(&reg.0).cloned();
        let round = fold::replace_folded(
            &mut function.blocks,
            &constant,
            &function.reg_types,
            declarations,
        );
        if round == 0 {
            return folded;
        }
        folded += round;
    }
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

/// Call `visit` on every block of `blocks`, then on the blocks of the `try`
/// regions each holds.
fn visit_blocks(blocks: &mut [MirBlock], visit: &mut impl FnMut(&mut MirBlock)) {
    for block in blocks {
        visit(block);
        for region in block.instrs.iter_mut().flat_map(instruction_regions_mut) {
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
/// operand's definition dominates. A forwarded register-passable trivial
/// value ([`MirDeclarations::copy_is_identity`]) gains the copy's readers,
/// which copy it where they consume it, as `cse` relies on too. Any other
/// forwarded value (a struct field read out of a `struct.create`) is
/// forwarded only when the fold is its operand's sole reader: a reader may
/// consume such a value, so it keeps exactly the fold's readers.
fn forward_copies(function: &mut MirFunction, declarations: &MirDeclarations) -> usize {
    let MirFunction {
        blocks,
        reg_types,
        n_regs,
        ..
    } = function;
    let definitions = definition_counts(blocks, *n_regs);
    let mut reads = vec![0_u32; *n_regs as usize];
    count_register_reads(blocks, &mut reads);
    let mut closures = HashSet::new();
    let mut creates = HashMap::new();
    let mut extracts = HashMap::new();
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::MakeClosure { dest, .. } if instruction.would_be_trivially_dead() => {
                    closures.insert(dest.0);
                }
                MirInstr::MakeStruct { dest, fields } => {
                    creates.insert(dest.0, fields.clone());
                }
                MirInstr::GetField {
                    dest,
                    base,
                    field,
                    constructors,
                } if constructors.is_empty() => {
                    extracts.insert(dest.0, (*base, field.clone()));
                }
                _ => {}
            }
        }
    });
    let structs = StructFolds {
        reg_types,
        declarations,
        creates,
        extracts,
    };
    // A copy of a closure of reference captures copies its pointers.
    let copied_closure = |instruction: &MirInstr| match instruction {
        MirInstr::CopyValue { dest, value, .. }
            if closures.contains(&value.0) && reg_types.get(&dest.0) == reg_types.get(&value.0) =>
        {
            Some(*value)
        }
        _ => None,
    };
    let mut folds = HashMap::new();
    let mut copies = HashMap::new();
    visit_blocks(blocks, &mut |block| {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            if let (Some(operand), [dest]) = (
                instruction
                    .forwarded_operand(reg_types, declarations)
                    .or_else(|| copied_closure(instruction))
                    .or_else(|| structs.extracted(instruction)),
                results.as_slice(),
            ) {
                folds.insert(dest.0, operand);
            }
            if let MirInstr::MakeStruct { dest, fields } = instruction
                && let Some(container) = structs.trivial_copy(*dest, fields, &reads)
            {
                copies.insert(dest.0, container);
            }
        }
    });
    let single = |reg: u32, most: u32| definitions.get(reg as usize).is_some_and(|n| *n <= most);
    let in_register = |reg: &Reg| {
        closures.contains(&reg.0)
            || reg_types
                .get(&reg.0)
                .is_some_and(|ty| declarations.copy_is_identity(ty))
    };
    let sole_reader = |reg: &Reg| reads.get(reg.0 as usize) == Some(&1);
    folds.retain(|dest: &u32, operand: &mut Reg| {
        single(*dest, 1) && single(operand.0, 1) && (in_register(operand) || sole_reader(operand))
    });
    // A trivial struct copy's container keeps the copy's readers alone: the
    // extracts that were its only other readers go with the copy.
    folds.extend(
        copies
            .into_iter()
            .filter(|(dest, container)| single(*dest, 1) && single(container.0, 1)),
    );
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

/// The struct values a body builds and takes apart, for upstream's
/// `kgen.struct` folds.
struct StructFolds<'a> {
    reg_types: &'a HashMap<u32, Ty>,
    declarations: &'a MirDeclarations,
    /// Each `struct.create`'s field operands, by its result.
    creates: HashMap<u32, Vec<Reg>>,
    /// Each `field.get`'s container and field, by its result.
    extracts: HashMap<u32, (Reg, String)>,
}

impl StructFolds<'_> {
    /// The operand an extract of a created struct's field reads: upstream's
    /// `StructExtractOp::fold`.
    fn extracted(&self, instruction: &MirInstr) -> Option<Reg> {
        let MirInstr::GetField {
            base,
            field,
            constructors,
            ..
        } = instruction
        else {
            return None;
        };
        if !constructors.is_empty() {
            return None;
        }
        let position = self.position(*base, field)?;
        self.creates.get(&base.0)?.get(position).copied()
    }

    /// The container a `struct.create` of every field extracted from it in
    /// order copies, when the extracts, each read once, are its only
    /// readers: upstream's `foldTrivialStructCopy`.
    fn trivial_copy(&self, dest: Reg, fields: &[Reg], reads: &[u32]) -> Option<Reg> {
        let (container, _) = self.extracts.get(&fields.first()?.0)?;
        let read_once =
            |reg: &Reg, times: usize| reads.get(reg.0 as usize).map(|&n| n as usize) == Some(times);
        (self.reg_types.get(&container.0) == self.reg_types.get(&dest.0)
            && read_once(container, fields.len())
            && fields.iter().enumerate().all(|(index, field)| {
                read_once(field, 1)
                    && self.extracts.get(&field.0).is_some_and(|(base, name)| {
                        base.0 == container.0 && self.position(*base, name) == Some(index)
                    })
            }))
        .then_some(*container)
    }

    /// The declaration position of the field `field` of the struct `base`
    /// holds.
    fn position(&self, base: Reg, field: &str) -> Option<usize> {
        let TyKind::Struct(name, _) = self.reg_types.get(&base.0)?.kind() else {
            return None;
        };
        self.declarations
            .structure(name)?
            .fields
            .iter()
            .position(|(candidate, _)| candidate == field)
    }
}

/// Every block of `blocks` and of the `try` regions they hold.
fn block_count(blocks: &mut [MirBlock]) -> usize {
    blocks
        .iter_mut()
        .map(|block| {
            1 + block
                .instrs
                .iter_mut()
                .flat_map(instruction_regions_mut)
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
        count_register_reads(&mut function.blocks, &mut reads);
        let before = erased.len();
        erase_dead(&mut function.blocks, &reads, erased);
        if erased.len() == before {
            break;
        }
    }
}

/// Remove each instruction of `blocks` and their regions that would be
/// trivially dead and defines only unread results, adding them to `erased`.
fn erase_dead(blocks: &mut [MirBlock], reads: &[u32], erased: &mut HashSet<u32>) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
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
pub(super) fn forward(blocks: &mut [MirBlock], roots: &HashMap<u32, Reg>) {
    let rewrite = |reg: &mut Reg| {
        if let Some(root) = roots.get(&reg.0) {
            *reg = *root;
        }
    };
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
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
