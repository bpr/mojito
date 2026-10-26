//! The canonicalization patterns of upstream's `hlcf` ops
//! (`HLCFOpsCanonicalizers.cpp`) and the generator `Canonicalizer`'s
//! `ConditionPropagation`, over a structured body.
//!
//! Every region of a structured body is one block, as `verify` requires.
//! The patterns rewrite an op in place, splice a region into its parent
//! block, or move the rest of a block into an arm, so each reads one region
//! block at a time. The driver walks each block list innermost
//! regions first and applies the patterns at each op until none applies, as
//! MLIR's greedy driver does, then repeats the walk until a whole walk
//! changes nothing. A replaced result is rewritten to its replacement in
//! every reader once a walk ends.

use super::definition_counts;
use mojito_common::timing;
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{
    Const, LoopLabel, MirBlock, MirElif, MirFunction, MirInstr, MirTerm, Reg, count_register_reads,
    fresh_register, instruction_regions, instruction_regions_mut, loop_breaks_mut,
    loop_continues_mut, replace_register_reads, resolve_replacements,
};
use mojito_types::types::{GenericConstraint, Ty};
use std::collections::{HashMap, HashSet};

/// Apply the `hlcf` patterns to `function` until none applies; returns how
/// many rewrites applied. The registers the rewrites leave undefined join
/// `erased`.
pub(super) fn hlcf_patterns(function: &mut MirFunction, erased: &mut HashSet<u32>) -> usize {
    let mut counts = Counts::default();
    loop {
        let mut reads = vec![0_u32; function.n_regs as usize];
        count_register_reads(&mut function.blocks, &mut reads);
        let mut blocks = std::mem::take(&mut function.blocks);
        let definitions = definition_counts(&mut blocks, function.n_regs);
        let mut constants = HashMap::new();
        bool_constants(&blocks, &definitions, &mut constants);
        let mut walk = Walk {
            function,
            reads,
            constants,
            replaced: HashMap::new(),
            erased,
            counts: Counts::default(),
        };
        walk.list(&mut blocks, &[]);
        let Walk {
            function: _,
            replaced,
            counts: round,
            ..
        } = walk;
        let resolved = resolve_replacements(&replaced);
        replace_register_reads(&mut blocks, &resolved);
        function.blocks = blocks;
        erased.extend(replaced.keys());
        if round.total() == 0 {
            break;
        }
        counts.add(&round);
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    counts.report();
    counts.total()
}

/// Add the results `instruction` and its regions define to `erased`.
pub(in crate::analysis) fn erased_results(instruction: &MirInstr, erased: &mut HashSet<u32>) {
    let mut results = Vec::new();
    instruction_result_regs(instruction, &mut results);
    erased.extend(results.iter().map(|reg| reg.0));
    for region in instruction_regions(instruction) {
        for nested in region.iter().flat_map(|block| &block.instrs) {
            erased_results(nested, erased);
        }
    }
}

/// How many times each pattern applied.
#[derive(Default)]
struct Counts {
    static_conditions: usize,
    condition_propagations: usize,
    unconditional_returns: usize,
    conditional_returns: usize,
    yield_results: usize,
    unused_results: usize,
    dead_loops: usize,
    unused_loop_results: usize,
    unused_loop_args: usize,
    comptime_ifs: usize,
}

impl Counts {
    const fn total(&self) -> usize {
        self.static_conditions
            + self.condition_propagations
            + self.unconditional_returns
            + self.conditional_returns
            + self.yield_results
            + self.unused_results
            + self.dead_loops
            + self.unused_loop_results
            + self.unused_loop_args
            + self.comptime_ifs
    }

    const fn add(&mut self, other: &Self) {
        self.static_conditions += other.static_conditions;
        self.condition_propagations += other.condition_propagations;
        self.unconditional_returns += other.unconditional_returns;
        self.conditional_returns += other.conditional_returns;
        self.yield_results += other.yield_results;
        self.unused_results += other.unused_results;
        self.dead_loops += other.dead_loops;
        self.unused_loop_results += other.unused_loop_results;
        self.unused_loop_args += other.unused_loop_args;
        self.comptime_ifs += other.comptime_ifs;
    }

    fn report(&self) {
        for (name, count) in [
            ("removed_static_conditions", self.static_conditions),
            ("propagated_conditions", self.condition_propagations),
            ("hoisted_unconditional_exits", self.unconditional_returns),
            ("hoisted_conditional_returns", self.conditional_returns),
            ("hoisted_yield_results", self.yield_results),
            ("removed_unused_results", self.unused_results),
            ("removed_dead_loops", self.dead_loops),
            ("removed_unused_loop_results", self.unused_loop_results),
            ("removed_unused_loop_args", self.unused_loop_args),
            ("folded_comptime_ifs", self.comptime_ifs),
        ] {
            timing::count(name, count as u64);
        }
    }
}

/// An `if` whose body arm holds the block being walked, as
/// `HoistConditionalReturn`'s `findParentTermOp` climbs out through it.
#[derive(Clone)]
struct Enclosing {
    results: Vec<Reg>,
    has_elifs: bool,
    /// Whether the `if` is the last instruction of its block.
    last: bool,
    /// The terminator of the `if`'s block.
    term: MirTerm,
}

/// One walk's state. Register reads and constants are counted when the walk
/// starts; a result a pattern replaces is rewritten once it ends.
struct Walk<'a> {
    function: &'a mut MirFunction,
    reads: Vec<u32>,
    /// The registers defined once, by a `const bool`.
    constants: HashMap<u32, bool>,
    replaced: HashMap<u32, Reg>,
    erased: &'a mut HashSet<u32>,
    counts: Counts,
}

/// The exit an `if` arm ends in, for the hoisting patterns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitKind {
    Return,
    Break,
    Continue,
}

impl Walk<'_> {
    /// Canonicalize each block of `blocks`, whose enclosing `if` arms are
    /// `enclosing`, innermost last.
    fn list(&mut self, blocks: &mut [MirBlock], enclosing: &[Enclosing]) {
        for block in blocks {
            self.block(block, enclosing);
        }
    }

    /// Canonicalize the regions `block`'s ops hold, then apply the patterns
    /// at each op until none applies.
    fn block(&mut self, block: &mut MirBlock, enclosing: &[Enclosing]) {
        let last = block.instrs.len().saturating_sub(1);
        for index in 0..block.instrs.len() {
            let term = block.term.clone();
            let instruction = &mut block.instrs[index];
            if let MirInstr::If {
                results,
                then,
                elifs,
                orelse,
                ..
            } = instruction
            {
                let frame = Enclosing {
                    results: results.clone(),
                    has_elifs: !elifs.is_empty(),
                    last: index == last,
                    term,
                };
                let mut nested = enclosing.to_vec();
                nested.push(frame);
                self.list(then, &nested);
                for elif in elifs {
                    self.list(&mut elif.cond, &[]);
                    self.list(&mut elif.then, &nested);
                }
                self.list(orelse, &nested);
            } else {
                for region in instruction_regions_mut(instruction) {
                    self.list(region, &[]);
                }
            }
        }
        let mut index = 0;
        while index < block.instrs.len() {
            if !self.apply(block, index, enclosing) {
                index += 1;
            }
        }
    }

    /// Apply the first pattern that matches the op at `index` of `block`,
    /// in upstream's order: `RemoveStaticCondition`, `ConditionPropagation`,
    /// the hoists, `HoistYieldResults`, and `RemoveUnusedResults` for an
    /// `if`; `RemoveDeadLoop`, `RemoveUnusedLoopResults`, and
    /// `RemoveUnusedLoopArgs` for a loop;
    /// `ComptimeIfOp::canonicalize` for a `comptime if`.
    fn apply(&mut self, block: &mut MirBlock, index: usize, enclosing: &[Enclosing]) -> bool {
        match &block.instrs[index] {
            MirInstr::If { .. } => {
                self.remove_static_condition(block, index)
                    || self.propagate_condition(block, index)
                    || self.hoist_unconditional_exit(block, index)
                    || self.hoist_conditional_return(block, index, enclosing)
                    || self.hoist_yield_results(block, index)
                    || self.remove_unused_results(block, index)
            }
            MirInstr::Loop { .. } => {
                self.remove_dead_loop(block, index)
                    || self.remove_unused_loop_results(block, index)
                    || self.remove_unused_loop_args(block, index)
            }
            MirInstr::ComptimeIf { .. } => self.canonicalize_comptime_if(block, index),
            _ => false,
        }
    }

    /// `RemoveStaticCondition`: an `if` on a constant takes the arm it
    /// selects. A false condition skips each elif whose condition region is
    /// trivial and constant false, takes the first trivial and constant
    /// true one or the else, and makes the first other elif the `if`'s
    /// condition, which needs a trivial condition region: the pattern fails
    /// without one. An elif whose condition carries values is never trivial,
    /// so every region the pattern splices or promotes takes no arguments.
    fn remove_static_condition(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::If {
            cond,
            then,
            elifs,
            orelse,
            ..
        } = &block.instrs[index]
        else {
            return false;
        };
        let Some(&taken) = self.constants.get(&cond.0) else {
            return false;
        };
        let single = |region: &[MirBlock]| region.len() == 1;
        if !body_regions(then, elifs, orelse).into_iter().all(single) {
            return false;
        }
        let decided: Vec<Option<bool>> = elifs
            .iter()
            .map(|elif| self.trivial_constant_condition(&elif.cond))
            .collect();
        let promote = match decided.iter().position(|value| *value != Some(false)) {
            Some(at) if !taken && decided[at].is_none() => {
                let Some(cond) = trivial_condition(&elifs[at].cond) else {
                    return false;
                };
                Some((at, cond))
            }
            _ => None,
        };
        let Some(IfParts {
            results,
            then,
            elifs,
            else_args,
            orelse,
        }) = take_if(block, index)
        else {
            return false;
        };
        if taken {
            for elif in elifs {
                self.erase_elif(elif);
            }
            self.erased.extend(else_args.iter().map(|arg| arg.0));
            self.erase_region(orelse);
            return self.replace_with_region(block, index, &results, then);
        }
        self.erase_region(then);
        let mut elifs = elifs.into_iter().enumerate();
        while let Some((at, mut elif)) = elifs.next() {
            match (decided[at], promote) {
                (Some(true), _) => {
                    let then = std::mem::take(&mut elif.then);
                    self.erase_elif(elif);
                    for (_, rest) in elifs {
                        self.erase_elif(rest);
                    }
                    self.erased.extend(else_args.iter().map(|arg| arg.0));
                    self.erase_region(orelse);
                    return self.replace_with_region(block, index, &results, then);
                }
                (_, Some((promoted, cond))) if promoted == at => {
                    let hoisted = elif.cond.into_iter().flat_map(|block| block.instrs);
                    let promoted = MirInstr::If {
                        results,
                        cond,
                        then: elif.then,
                        elifs: elifs.map(|(_, rest)| rest).collect(),
                        else_args,
                        orelse,
                    };
                    block.instrs.splice(index..index, hoisted.chain([promoted]));
                    self.counts.static_conditions += 1;
                    return true;
                }
                _ => self.erase_elif(elif),
            }
        }
        self.replace_with_region(block, index, &results, orelse)
    }

    /// The constant an elif condition region yields when it is trivial:
    /// one block of `const`s ending in its condition, a `const bool`.
    fn trivial_constant_condition(&self, region: &[MirBlock]) -> Option<bool> {
        let cond = trivial_condition(region)?;
        region[0]
            .instrs
            .iter()
            .find_map(|instruction| match instruction {
                MirInstr::Const {
                    dest,
                    k: Const::Bool(value),
                } if *dest == cond => Some(*value),
                _ => None,
            })
            .or_else(|| self.constants.get(&cond.0).copied())
    }

    /// `ConditionPropagation`: inside the `then` arm of an `if` without
    /// elifs its condition is true, and inside the `else` arm false, so each
    /// read there reads a constant defined before the `if`.
    fn propagate_condition(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::If {
            cond,
            then,
            elifs,
            orelse,
            ..
        } = &mut block.instrs[index]
        else {
            return false;
        };
        if !elifs.is_empty() || self.constants.contains_key(&cond.0) {
            return false;
        }
        let cond = *cond;
        let arms = [reads_register(then, cond), reads_register(orelse, cond)];
        if arms == [false, false] {
            return false;
        }
        let ty = self
            .function
            .reg_types
            .get(&cond.0)
            .cloned()
            .unwrap_or_else(Ty::bool);
        let span = self
            .function
            .spans
            .0
            .get(&cond.0)
            .map(|(span, _)| span.clone());
        let mut constants = Vec::new();
        for (at, value) in [true, false].into_iter().enumerate() {
            if !arms[at] {
                continue;
            }
            let dest = fresh_register(self.function, ty.clone(), span.as_ref());
            self.reads.push(1);
            if let MirInstr::If { then, orelse, .. } = &mut block.instrs[index] {
                let arm = if value { then } else { orelse };
                replace_register_reads(arm, &HashMap::from([(cond.0, dest)]));
            }
            constants.push(MirInstr::Const {
                dest,
                k: Const::Bool(value),
            });
        }
        block.instrs.splice(index..index, constants);
        self.counts.condition_propagations += 1;
        true
    }

    /// `HoistUnconditionalReturn`: when every body arm ends in the same
    /// kind of exit, with the same operand types and, for a `break` or
    /// `continue`, the same label, the arms yield the exit's operands to a
    /// new `if`, which one exit follows; the rest of the block goes.
    fn hoist_unconditional_exit(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::If {
            then,
            elifs,
            orelse,
            ..
        } = &block.instrs[index]
        else {
            return false;
        };
        let mut terms = Vec::new();
        for body in body_regions(then, elifs, orelse) {
            let [region] = body else {
                return false;
            };
            terms.push(&region.term);
        }
        let Some((kind, label, operands)) = exit_of(terms[0]) else {
            return false;
        };
        let types = |operands: &[Reg]| -> Vec<Option<Ty>> {
            operands
                .iter()
                .map(|reg| self.function.reg_types.get(&reg.0).cloned())
                .collect()
        };
        let first_types = types(&operands);
        let same = terms[1..].iter().all(|term| {
            exit_of(term).is_some_and(|(other_kind, other_label, other)| {
                other_kind == kind
                    && (kind == ExitKind::Return || other_label == label)
                    && types(&other) == first_types
            })
        });
        if !same {
            return false;
        }
        let span = operands
            .first()
            .and_then(|reg| self.function.spans.0.get(&reg.0))
            .map(|(span, _)| span.clone());
        let MirInstr::If {
            results,
            then,
            elifs,
            orelse,
            ..
        } = &mut block.instrs[index]
        else {
            return false;
        };
        let new_results: Vec<Reg> = first_types
            .into_iter()
            .map(|ty| {
                self.reads.push(1);
                fresh_register(self.function, ty.unwrap_or_else(Ty::none), span.as_ref())
            })
            .collect();
        for region in body_regions_mut(then, elifs, orelse) {
            let operands = exit_of(&region[0].term).map(|(_, _, operands)| operands);
            region[0].term = MirTerm::Yield(operands.unwrap_or_default());
        }
        let old_results = std::mem::replace(results, new_results.clone());
        self.erased.extend(old_results.iter().map(|reg| reg.0));
        let tail = block.instrs.split_off(index + 1);
        self.erase_ops(&tail);
        block.term = match kind {
            ExitKind::Return => MirTerm::Return(new_results.first().copied()),
            ExitKind::Break => MirTerm::Break {
                label,
                values: new_results,
            },
            ExitKind::Continue => MirTerm::Continue {
                label,
                values: new_results,
            },
        };
        self.counts.unconditional_returns += 1;
        true
    }

    /// `HoistConditionalReturn`: when exactly one body arm yields and the
    /// others `return` (or `break` to one loop), the rest of the block moves
    /// into the yielding arm, and one exit after a new `if` returns what
    /// every arm now yields. The exit the yielding arm reaches is found by
    /// climbing out through enclosing `if`s without elifs, each the last op
    /// of its block, as `findParentTermOp` climbs.
    fn hoist_conditional_return(
        &mut self,
        block: &mut MirBlock,
        index: usize,
        enclosing: &[Enclosing],
    ) -> bool {
        let MirInstr::If {
            results,
            then,
            elifs,
            orelse,
            ..
        } = &block.instrs[index]
        else {
            return false;
        };
        let mut yielding = None;
        let mut exit: Option<(ExitKind, Option<LoopLabel>, Vec<Reg>)> = None;
        for (at, body) in body_regions(then, elifs, orelse).into_iter().enumerate() {
            let [region] = body else {
                return false;
            };
            match &region.term {
                MirTerm::Yield(values) => {
                    if yielding.is_some() {
                        return false;
                    }
                    yielding = Some((at, values.clone()));
                }
                term @ (MirTerm::Return(_) | MirTerm::Break { .. }) => {
                    let Some(found) = exit_of(term) else {
                        return false;
                    };
                    if let Some((kind, label, _)) = &exit
                        && (*kind != found.0 || *label != found.1)
                    {
                        return false;
                    }
                    exit.get_or_insert(found);
                }
                _ => return false,
            }
        }
        let (Some((yield_arm, yielded)), Some((kind, label, sample))) = (yielding, exit) else {
            return false;
        };
        let Some((parent, mut operands)) = climb(&block.term, enclosing) else {
            return false;
        };
        for operand in &mut operands {
            if let Some(at) = results.iter().position(|result| result == operand) {
                let Some(value) = yielded.get(at) else {
                    return false;
                };
                *operand = *value;
            }
        }
        let Some((parent_kind, parent_label, parent_operands)) = exit_of(&parent) else {
            return false;
        };
        let types = |operands: &[Reg]| -> Vec<Option<Ty>> {
            operands
                .iter()
                .map(|reg| self.function.reg_types.get(&reg.0).cloned())
                .collect()
        };
        if parent_kind != kind
            || (kind == ExitKind::Break && parent_label != label)
            || types(&sample) != types(&parent_operands)
            || yielded.len() != results.len()
            || operands.len() != sample.len()
        {
            return false;
        }
        let span = sample
            .first()
            .and_then(|reg| self.function.spans.0.get(&reg.0))
            .map(|(span, _)| span.clone());
        let result_types = types(&sample);
        let mut tail = Some(block.instrs.split_off(index + 1));
        let MirInstr::If {
            results,
            then,
            elifs,
            orelse,
            ..
        } = &mut block.instrs[index]
        else {
            block.instrs.extend(tail.into_iter().flatten());
            return false;
        };
        let new_results: Vec<Reg> = result_types
            .into_iter()
            .map(|ty| {
                self.reads.push(1);
                fresh_register(self.function, ty.unwrap_or_else(Ty::none), span.as_ref())
            })
            .collect();
        for (at, region) in body_regions_mut(then, elifs, orelse)
            .into_iter()
            .enumerate()
        {
            if at == yield_arm {
                region[0].instrs.extend(tail.take().into_iter().flatten());
                region[0].term = MirTerm::Yield(operands.clone());
            } else {
                let operands = exit_of(&region[0].term).map(|(_, _, operands)| operands);
                region[0].term = MirTerm::Yield(operands.unwrap_or_default());
            }
        }
        let old_results = std::mem::replace(results, new_results.clone());
        for (result, value) in old_results.iter().zip(&yielded) {
            self.replaced.insert(result.0, *value);
        }
        block.term = match kind {
            ExitKind::Return => MirTerm::Return(new_results.first().copied()),
            ExitKind::Break | ExitKind::Continue => MirTerm::Break {
                label,
                values: new_results,
            },
        };
        self.counts.conditional_returns += 1;
        true
    }

    /// `HoistYieldResults`: when every body arm is a bare `yield`, a result
    /// all arms yield one value for becomes that value, and with two arms
    /// yielding constant true and false a `Bool` result becomes the
    /// condition. An `if` left without a read result goes, its elif
    /// condition regions with it, as upstream erases the op whole: the pin
    /// skips a call in such a condition.
    fn hoist_yield_results(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::If {
            results,
            cond,
            then,
            elifs,
            orelse,
            ..
        } = &block.instrs[index]
        else {
            return false;
        };
        let mut yields = Vec::new();
        for body in body_regions(then, elifs, orelse) {
            match body {
                [
                    MirBlock {
                        instrs,
                        term: MirTerm::Yield(values),
                    },
                ] if instrs.is_empty() => yields.push(values),
                _ => return false,
            }
        }
        let cond_ty = self.function.reg_types.get(&cond.0);
        let mut changed = false;
        let mut unread = true;
        for (at, result) in results.iter().enumerate() {
            if self.replaced.contains_key(&result.0) {
                continue;
            }
            let first = yields[0][at];
            let flag = |values: &Vec<Reg>| self.constants.get(&values[at].0).copied();
            let replacement = if yields.len() == 2
                && self.function.reg_types.get(&result.0) == cond_ty
                && flag(yields[0]) == Some(true)
                && flag(yields[1]) == Some(false)
            {
                Some(*cond)
            } else {
                yields
                    .iter()
                    .all(|values| values[at] == first)
                    .then_some(first)
            };
            match replacement {
                Some(value) => {
                    self.replaced.insert(result.0, value);
                    changed = true;
                }
                None => unread &= self.reads.get(result.0 as usize) == Some(&0),
            }
        }
        if unread {
            let removed = block.instrs.remove(index);
            self.erase_ops(std::slice::from_ref(&removed));
            self.counts.yield_results += 1;
            return true;
        }
        if changed {
            self.counts.yield_results += 1;
        }
        changed
    }

    /// `RemoveUnusedResults`: an `if` result nothing reads goes, with its
    /// operand of every yielding body arm.
    fn remove_unused_results(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::If {
            results,
            then,
            elifs,
            orelse,
            ..
        } = &mut block.instrs[index]
        else {
            return false;
        };
        let unused: Vec<bool> = results
            .iter()
            .map(|result| {
                self.reads.get(result.0 as usize) == Some(&0)
                    && !self.replaced.contains_key(&result.0)
            })
            .collect();
        if !unused.contains(&true) {
            return false;
        }
        for region in body_regions_mut(then, elifs, orelse) {
            for block in region {
                if let MirTerm::Yield(values) = &mut block.term
                    && values.len() == unused.len()
                {
                    let mut at = 0;
                    values.retain(|_| {
                        at += 1;
                        !unused[at - 1]
                    });
                }
            }
        }
        let mut at = 0;
        results.retain(|result| {
            at += 1;
            if unused[at - 1] {
                self.erased.insert(result.0);
            }
            !unused[at - 1]
        });
        self.counts.unused_results += 1;
        true
    }

    /// `RemoveDeadLoop`: a loop whose body is just a `break` goes. A break
    /// out of it replaces its results by the values it carries; one naming
    /// an outer loop replaces the loop and the rest of the block. Either
    /// way the body runs once, entered with the loop's operands, so each
    /// argument is replaced by its operand, as inlining the body block with
    /// them does.
    fn remove_dead_loop(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body,
        } = &block.instrs[index]
        else {
            return false;
        };
        let [
            MirBlock {
                instrs,
                term:
                    term @ MirTerm::Break {
                        label: target,
                        values,
                    },
            },
        ] = body.as_slice()
        else {
            return false;
        };
        if !instrs.is_empty() {
            return false;
        }
        for (arg, operand) in args.iter().zip(operands) {
            self.replaced.insert(arg.0, *operand);
        }
        if target.is_none() || target == label {
            for (result, value) in results.iter().zip(values) {
                self.replaced.insert(result.0, *value);
            }
            block.instrs.remove(index);
        } else {
            let term = term.clone();
            let tail = block.instrs.split_off(index);
            self.erase_ops(&tail);
            block.term = term;
        }
        self.counts.dead_loops += 1;
        true
    }

    /// `RemoveUnusedLoopResults`: a loop result nothing reads goes, with
    /// its operand of every `break` that leaves the loop.
    fn remove_unused_loop_results(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::Loop {
            label,
            results,
            body,
            ..
        } = &mut block.instrs[index]
        else {
            return false;
        };
        let unused: Vec<bool> = results
            .iter()
            .map(|result| {
                self.reads.get(result.0 as usize) == Some(&0)
                    && !self.replaced.contains_key(&result.0)
            })
            .collect();
        if !unused.contains(&true) {
            return false;
        }
        loop_breaks_mut(body, *label, &mut |values, _| {
            if values.len() == unused.len() {
                let mut at = 0;
                values.retain(|_| {
                    at += 1;
                    !unused[at - 1]
                });
            }
        });
        let mut at = 0;
        results.retain(|result| {
            at += 1;
            if unused[at - 1] {
                self.erased.insert(result.0);
            }
            !unused[at - 1]
        });
        self.counts.unused_loop_results += 1;
        true
    }

    /// `RemoveUnusedLoopArgs`: a loop argument nothing reads goes, with its
    /// operand and its value in every `continue` that resumes the loop.
    fn remove_unused_loop_args(&mut self, block: &mut MirBlock, index: usize) -> bool {
        let MirInstr::Loop {
            label,
            operands,
            args,
            body,
            ..
        } = &mut block.instrs[index]
        else {
            return false;
        };
        let unused: Vec<bool> = args
            .iter()
            .map(|arg| {
                self.reads.get(arg.0 as usize) == Some(&0) && !self.replaced.contains_key(&arg.0)
            })
            .collect();
        if !unused.contains(&true) || operands.len() != unused.len() {
            return false;
        }
        let keep = |values: &mut Vec<Reg>| {
            let mut at = 0;
            values.retain(|_| {
                at += 1;
                !unused[at - 1]
            });
        };
        loop_continues_mut(body, *label, &mut |values, _| {
            if values.len() == unused.len() {
                keep(values);
            }
        });
        keep(operands);
        let mut at = 0;
        args.retain(|arg| {
            at += 1;
            if unused[at - 1] {
                self.erased.insert(arg.0);
            }
            !unused[at - 1]
        });
        self.counts.unused_loop_args += 1;
        true
    }

    /// `ComptimeIfOp::canonicalize`. A `comptime if` without results whose
    /// regions are bare `comptime.yield`s goes, as does one whose regions
    /// are bare yields or `break`s when a `break` follows it
    /// (`checkTrivialBlocks`). Otherwise a single condition that is a
    /// constant `Bool` hoists the live arm's instructions before the op,
    /// stopping at a `comptime for`, which declares a parameter. A live arm
    /// left a bare yield replaces the op by its values; one left a bare
    /// `unreachable`, `break`, `continue`, or `raise` replaces the op and
    /// the rest of the block; otherwise the dead arm becomes one `unreachable` block and
    /// the op stays, a parameter scope.
    fn canonicalize_comptime_if(&mut self, block: &mut MirBlock, index: usize) -> bool {
        if trivial_comptime_if(block, index) {
            let removed = block.instrs.remove(index);
            self.erase_ops(std::slice::from_ref(&removed));
            self.counts.comptime_ifs += 1;
            return true;
        }
        let MirInstr::ComptimeIf {
            results,
            conds,
            arms,
            orelse,
        } = &mut block.instrs[index]
        else {
            return false;
        };
        let [cond] = conds.as_slice() else {
            return false;
        };
        let Some(value) = constant_condition(cond) else {
            return false;
        };
        let (live, dead) = if value {
            (&mut arms[0], orelse)
        } else {
            (orelse, &mut arms[0])
        };
        let ([live], [dead_block]) = (live.as_mut_slice(), dead.as_slice()) else {
            return false;
        };
        if dead_block.instrs.is_empty() && matches!(dead_block.term, MirTerm::Unreachable) {
            return false;
        }
        let stop = live
            .instrs
            .iter()
            .position(|instruction| matches!(instruction, MirInstr::ComptimeFor { .. }))
            .unwrap_or(live.instrs.len());
        let hoisted: Vec<MirInstr> = live.instrs.drain(..stop).collect();
        let bare = live.instrs.is_empty();
        let live_term = live.term.clone();
        let results = results.clone();
        let count = hoisted.len();
        block.instrs.splice(index..index, hoisted);
        let index = index + count;
        match live_term {
            MirTerm::ComptimeYield(values) if bare => {
                let removed = block.instrs.remove(index);
                for (result, value) in results.iter().zip(values) {
                    self.replaced.insert(result.0, value);
                }
                self.erase_ops(std::slice::from_ref(&removed));
            }
            term @ (MirTerm::Unreachable
            | MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::TryRaise { .. }
            | MirTerm::ErrorReturn)
                if bare =>
            {
                let tail = block.instrs.split_off(index);
                self.erase_ops(&tail);
                block.term = term;
            }
            _ => {
                if let MirInstr::ComptimeIf { arms, orelse, .. } = &mut block.instrs[index] {
                    let dead = if value { orelse } else { &mut arms[0] };
                    let purged = std::mem::replace(
                        dead,
                        vec![MirBlock {
                            instrs: Vec::new(),
                            term: MirTerm::Unreachable,
                        }],
                    );
                    self.erase_region(purged);
                }
            }
        }
        self.counts.comptime_ifs += 1;
        true
    }

    /// `replaceOpWithRegion`: splice the one block of `region` at `index`.
    /// A `yield` replaces `results` by its values; any other terminator
    /// ends `block`, whose rest (already removed by the caller when it
    /// differs) goes.
    fn replace_with_region(
        &mut self,
        block: &mut MirBlock,
        index: usize,
        results: &[Reg],
        region: Vec<MirBlock>,
    ) -> bool {
        let mut region = region.into_iter();
        let Some(MirBlock { instrs, term }) = region.next() else {
            return false;
        };
        self.erase_region(region.collect());
        match term {
            MirTerm::Yield(values) | MirTerm::ComptimeYield(values) => {
                for (result, value) in results.iter().zip(values) {
                    self.replaced.insert(result.0, value);
                }
                block.instrs.splice(index..index, instrs);
            }
            term => {
                let tail = block.instrs.split_off(index);
                self.erase_ops(&tail);
                block.instrs.extend(instrs);
                block.term = term;
            }
        }
        self.counts.static_conditions += 1;
        true
    }

    /// Record the results `instructions` and their regions define as
    /// erased.
    fn erase_ops(&mut self, instructions: &[MirInstr]) {
        for instruction in instructions {
            erased_results(instruction, self.erased);
        }
    }

    /// [`Self::erase_ops`] over a region's blocks.
    fn erase_region(&mut self, region: Vec<MirBlock>) {
        for block in region {
            self.erase_ops(&block.instrs);
        }
    }

    /// [`Self::erase_region`] over a dropped elif's regions, and its
    /// arguments with them.
    fn erase_elif(&mut self, elif: MirElif) {
        let args = elif.cond_args.iter().chain(&elif.then_args);
        self.erased.extend(args.map(|arg| arg.0));
        self.erase_region(elif.cond);
        self.erase_region(elif.then);
    }
}

/// `checkTrivialBlocks`: the `comptime if` at `index` has no results,
/// and each region is a bare `comptime.yield` (or `break`, when a
/// `break` follows the op).
fn trivial_comptime_if(block: &MirBlock, index: usize) -> bool {
    let MirInstr::ComptimeIf {
        results,
        arms,
        orelse,
        ..
    } = &block.instrs[index]
    else {
        return false;
    };
    if !results.is_empty() {
        return false;
    }
    let mut all_yield = true;
    for region in arms.iter().chain(std::iter::once(orelse)) {
        match region.as_slice() {
            [MirBlock { instrs, term }] if instrs.is_empty() => match term {
                MirTerm::ComptimeYield(values) if values.is_empty() => {}
                MirTerm::Break { values, .. } if values.is_empty() => all_yield = false,
                _ => return false,
            },
            _ => return false,
        }
    }
    all_yield || (index + 1 == block.instrs.len() && matches!(block.term, MirTerm::Break { .. }))
}

/// The exit a body arm ends in: its kind, its label, and its operands.
fn exit_of(term: &MirTerm) -> Option<(ExitKind, Option<LoopLabel>, Vec<Reg>)> {
    match term {
        MirTerm::Return(value) => Some((ExitKind::Return, None, value.iter().copied().collect())),
        MirTerm::Break { label, values } => Some((ExitKind::Break, *label, values.clone())),
        MirTerm::Continue { label, values } => Some((ExitKind::Continue, *label, values.clone())),
        _ => None,
    }
}

/// `findParentTermOp` from the terminator `term` of the block holding the
/// `if` being rewritten: a `return` or `break` is the exit, and a `yield`
/// climbs to the enclosing `if`, which must have no elifs and end its
/// block, its results remapped to the yielded values.
fn climb(term: &MirTerm, enclosing: &[Enclosing]) -> Option<(MirTerm, Vec<Reg>)> {
    match term {
        MirTerm::Return(value) => Some((term.clone(), value.iter().copied().collect())),
        MirTerm::Break { values, .. } => Some((term.clone(), values.clone())),
        MirTerm::Yield(values) => {
            let (parent, outer) = enclosing.split_last()?;
            if parent.has_elifs || !parent.last {
                return None;
            }
            let (found, mut operands) = climb(&parent.term, outer)?;
            for operand in &mut operands {
                if let Some(at) = parent.results.iter().position(|result| result == operand) {
                    *operand = *values.get(at)?;
                }
            }
            Some((found, operands))
        }
        _ => None,
    }
}

/// An `if`'s body regions: `then`, each elif's `then`, and `orelse`.
fn body_regions<'a>(
    then: &'a [MirBlock],
    elifs: &'a [MirElif],
    orelse: &'a [MirBlock],
) -> Vec<&'a [MirBlock]> {
    std::iter::once(then)
        .chain(elifs.iter().map(|elif| elif.then.as_slice()))
        .chain(std::iter::once(orelse))
        .collect()
}

/// [`body_regions`], mutably.
fn body_regions_mut<'a>(
    then: &'a mut Vec<MirBlock>,
    elifs: &'a mut [MirElif],
    orelse: &'a mut Vec<MirBlock>,
) -> Vec<&'a mut Vec<MirBlock>> {
    std::iter::once(then)
        .chain(elifs.iter_mut().map(|elif| &mut elif.then))
        .chain(std::iter::once(orelse))
        .collect()
}

/// The condition an elif condition region yields when the region is
/// trivial: one block of `const`s ending in its condition and carrying no
/// values, the shape MLIR's folder leaves once it materializes the
/// constants outside the region.
fn trivial_condition(region: &[MirBlock]) -> Option<Reg> {
    match region {
        [
            MirBlock {
                instrs,
                term: MirTerm::ElifCond { cond, values },
            },
        ] if values.is_empty()
            && instrs
                .iter()
                .all(|instruction| matches!(instruction, MirInstr::Const { .. })) =>
        {
            Some(*cond)
        }
        _ => None,
    }
}

/// The parts of an `if`.
struct IfParts {
    results: Vec<Reg>,
    then: Vec<MirBlock>,
    elifs: Vec<MirElif>,
    else_args: Vec<Reg>,
    orelse: Vec<MirBlock>,
}

/// Remove the `if` at `index` of `block` and return its parts; any other
/// instruction stays.
fn take_if(block: &mut MirBlock, index: usize) -> Option<IfParts> {
    match block.instrs.remove(index) {
        MirInstr::If {
            results,
            then,
            elifs,
            else_args,
            orelse,
            ..
        } => Some(IfParts {
            results,
            then,
            elifs,
            else_args,
            orelse,
        }),
        other => {
            block.instrs.insert(index, other);
            None
        }
    }
}

/// The value of a `comptime if` condition that is a constant `Bool`.
fn constant_condition(cond: &GenericConstraint) -> Option<bool> {
    match cond {
        GenericConstraint::Bool(value) => Some(*value),
        GenericConstraint::WithMessage(inner, _) => constant_condition(inner),
        _ => None,
    }
}

/// Collect the registers of `blocks`, at any depth, defined once by a
/// `const bool`.
fn bool_constants(blocks: &[MirBlock], definitions: &[u32], out: &mut HashMap<u32, bool>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::Const {
            dest,
            k: Const::Bool(value),
        } = instruction
            && definitions.get(dest.0 as usize) == Some(&1)
        {
            out.insert(dest.0, *value);
        }
        for region in instruction_regions(instruction) {
            bool_constants(region, definitions, out);
        }
    }
}

/// Whether some instruction or terminator of `blocks`, at any depth, reads
/// `reg`.
fn reads_register(blocks: &mut [MirBlock], reg: Reg) -> bool {
    let mut reads = vec![0_u32; reg.0 as usize + 1];
    count_register_reads(blocks, &mut reads);
    reads[reg.0 as usize] > 0
}
