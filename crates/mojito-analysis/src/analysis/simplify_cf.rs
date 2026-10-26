//! `simplify-cf`: upstream's `SimplifyCF` (`lib/Transforms/SimplifyCF.cpp`),
//! which its `AutomaticInline` function pipeline runs first on each
//! function. It removes the structured control flow inlining leaves behind,
//! in upstream's three steps:
//!
//! - **Trivial loops** (`tryRemovingLoop`). One preorder walk counts, for
//!   each loop, the `break`s and `continue`s that resolve to it
//!   ([`RegionFrame::names`]); those of an inner loop do not count. Then, in
//!   reverse preorder, a loop whose body ends in its one `break` is replaced
//!   by its body, the body's arguments by the loop's operands and the
//!   loop's results by the break's values; a loop nothing jumps to is
//!   replaced by its body too, whose terminator then ends the enclosing
//!   block, and what followed the loop goes. An inlined body is such a
//!   loop: its `return`s are `break`s.
//! - **Trivial `try`s** (`removeTrivialTry`). A `try` no raise names (a
//!   raising call raises in its error region) is replaced by its body and,
//!   when the body completes (`try.yield`), its `else`, each `try.yield`'s
//!   values replacing the `else`'s arguments or the `try`'s results; a part
//!   that leaves instead ends the enclosing block. A raise names the
//!   nearest `try` carrying its label whose body holds it.
//! - **Implied conditions** (`propagateImpliedCondition`). After an `if`
//!   whose condition is not a constant and whose `then` arm cannot fall
//!   through (it ends in a `return`, `raise`, `break`, `continue`, or
//!   `unreachable`),
//!   the condition is `false` wherever the `if`'s block reads it later; with
//!   no elifs and an `else` arm that cannot fall through, it is `true`.
//!   `canonicalize` then folds what reads it.

use super::canonicalize::erased_results;
use super::pre_elaboration::BodyPass;
use mojito_common::timing;
use mojito_mir::mir::{
    Const, LoopLabel, MirBlock, MirDeclarations, MirFunction, MirInstr, MirTerm, Reg, RegionFrame,
    TryLabel, count_register_reads, fresh_register, instruction_regions, instruction_regions_mut,
    replace_register_reads, resolve_replacements,
};
use mojito_types::types::Ty;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// The pass, under upstream's flag spelling.
pub const SIMPLIFY_CF: BodyPass = BodyPass {
    name: "simplify-cf",
    run: simplify_cf,
};

/// Remove the trivial loops and `try`s of `function` and propagate the
/// conditions its `if`s imply, as upstream's `SimplifyCF` does; returns
/// whether any of these applied.
pub fn simplify_cf(_name: &str, function: &mut MirFunction, _: &MirDeclarations) -> bool {
    let mut census = Census::default();
    census.walk(&function.blocks);
    let mut removal = Removal {
        next_loop: census.jumps.len(),
        next_try: census.elidable.len(),
        erased_tries: vec![false; census.elidable.len()],
        census,
        replaced: HashMap::new(),
        erased: HashSet::new(),
        erased_loops: 0,
        erased_tries_count: 0,
    };
    removal.remove_loops(&mut function.blocks);
    let mut survivors: Vec<usize> = (0..removal.erased_tries.len())
        .filter(|&id| !removal.erased_tries[id])
        .collect();
    removal.remove_tries(&mut function.blocks, &mut survivors);
    let Removal {
        replaced,
        mut erased,
        erased_loops,
        erased_tries_count,
        ..
    } = removal;
    let resolved = resolve_replacements(&replaced);
    replace_register_reads(&mut function.blocks, &resolved);
    erased.extend(replaced.keys());
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    function.spans.0.retain(|reg, _| !erased.contains(reg));
    let implied = propagate_implied_conditions(function);
    timing::count("erased_loops", erased_loops as u64);
    timing::count("erased_tries", erased_tries_count as u64);
    timing::count("implied_conditions", implied as u64);
    erased_loops + erased_tries_count + implied > 0
}

/// Whether a region ending in `term` cannot fall through to its op's end:
/// it returns, raises, leaves or resumes a loop, or never ends.
const fn exits_early(term: &MirTerm) -> bool {
    matches!(
        term,
        MirTerm::Return(_)
            | MirTerm::TryRaise { .. }
            | MirTerm::ErrorReturn
            | MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::Unreachable
    )
}

/// Replace the instructions of `block` from `from` on, and its terminator,
/// by `term`, adding the registers the erased instructions define to
/// `erased`: a region inlined there leaves to an enclosing op.
fn end_block_at(block: &mut MirBlock, from: usize, term: MirTerm, erased: &mut HashSet<u32>) {
    for instruction in block.instrs.split_off(from) {
        erased_results(&instruction, erased);
    }
    block.term = term;
}

/// Upstream's `walkPreorder`: the jumps that resolve to each loop and
/// whether each `try` can be elided, both numbered in preorder.
#[derive(Default)]
struct Census {
    /// For each loop, the `break`s and `continue`s that resolve to it.
    jumps: Vec<usize>,
    /// For each `try`, whether it can be elided.
    elidable: Vec<bool>,
    /// The loops enclosing the walk, innermost last, with their numbers.
    parents: Vec<(Option<LoopLabel>, usize)>,
    /// The `try`s whose bodies hold the walk, innermost last, with their
    /// numbers: a raise naming one keeps it.
    tries: Vec<(TryLabel, usize)>,
}

impl Census {
    fn walk(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                match instruction {
                    MirInstr::Loop { label, body, .. } => {
                        let id = self.jumps.len();
                        self.jumps.push(0);
                        self.parents.push((*label, id));
                        self.walk(body);
                        self.parents.pop();
                    }
                    MirInstr::Try {
                        label,
                        body,
                        handler,
                        orelse,
                        ..
                    } => {
                        let id = self.elidable.len();
                        self.elidable.push(true);
                        self.tries.push((*label, id));
                        self.walk(body);
                        self.tries.pop();
                        for part in handler.iter().chain(orelse) {
                            self.walk(part);
                        }
                    }
                    _ => {
                        for region in instruction_regions(instruction) {
                            self.walk(region);
                        }
                    }
                }
            }
            if let MirTerm::TryRaise { label, .. } = block.term
                && let Some(&(_, id)) = self.tries.iter().rev().find(|(found, _)| *found == label)
            {
                self.elidable[id] = false;
            }
            if let MirTerm::Break { label, .. } | MirTerm::Continue { label, .. } = &block.term
                && let Some(&(_, id)) =
                    self.parents.iter().rev().find(|(found, _)| {
                        RegionFrame::Loop(*label).names(RegionFrame::Loop(*found))
                    })
            {
                self.jumps[id] += 1;
            }
        }
    }
}

/// The removals, each walk in reverse preorder, so that a removal moves
/// only what the walk has already visited.
struct Removal {
    census: Census,
    /// The preorder number of the loop the walk visits next, plus one.
    next_loop: usize,
    /// The preorder number of the `try` the loop walk visits next, plus one.
    next_try: usize,
    /// For each `try`, whether a loop's removal erased it.
    erased_tries: Vec<bool>,
    /// Each removed op's results and arguments, by the values replacing
    /// them.
    replaced: HashMap<u32, Reg>,
    /// The registers whose definitions went.
    erased: HashSet<u32>,
    erased_loops: usize,
    erased_tries_count: usize,
}

impl Removal {
    /// `tryRemovingLoop` over every loop of `blocks`; the `try`s a removal
    /// erases are recorded, so that the `try` walk numbers the rest.
    fn remove_loops(&mut self, blocks: &mut [MirBlock]) {
        for block in blocks.iter_mut().rev() {
            let block_end = self.next_try;
            let mut at = block.instrs.len();
            while at > 0 {
                at -= 1;
                let after_tail = self.next_try;
                for region in instruction_regions_mut(&mut block.instrs[at])
                    .into_iter()
                    .rev()
                {
                    self.remove_loops(region);
                }
                match block.instrs[at] {
                    MirInstr::Loop { .. } => {
                        self.next_loop -= 1;
                        self.try_removing_loop(block, at, after_tail..block_end);
                    }
                    MirInstr::Try { .. } => self.next_try -= 1,
                    _ => {}
                }
            }
        }
    }

    /// Remove the loop at `at` of `block` when it is trivial; `tail` holds
    /// the numbers of the `try`s after it in the block.
    fn try_removing_loop(&mut self, block: &mut MirBlock, at: usize, tail: Range<usize>) {
        let count = self.census.jumps[self.next_loop];
        let single_break = match &block.instrs[at] {
            MirInstr::Loop { label, body, .. } if body.len() == 1 => {
                count == 1
                    && matches!(&body[0].term, MirTerm::Break { label: named, .. }
                        if named.is_none() || named == label)
            }
            _ => return,
        };
        if !single_break && count != 0 {
            return;
        }
        let instruction = block.instrs.remove(at);
        let MirInstr::Loop {
            operands,
            args,
            results,
            mut body,
            ..
        } = instruction
        else {
            block.instrs.insert(at, instruction);
            return;
        };
        let arguments: HashMap<u32, Reg> = args.iter().map(|arg| arg.0).zip(operands).collect();
        replace_register_reads(&mut body, &arguments);
        self.erased.extend(arguments.keys());
        let Some(MirBlock { instrs, term }) = body.pop() else {
            return;
        };
        self.erased_loops += 1;
        let inlined = instrs.len();
        block.instrs.splice(at..at, instrs);
        match term {
            MirTerm::Break { values, .. } if single_break => {
                self.replaced
                    .extend(results.iter().map(|result| result.0).zip(values));
            }
            term => {
                self.erased.extend(results.iter().map(|result| result.0));
                for id in tail {
                    self.erased_tries[id] = true;
                }
                end_block_at(block, at + inlined, term, &mut self.erased);
            }
        }
    }

    /// `removeTrivialTry` over every elidable `try` of `blocks`, numbered
    /// by `survivors`, the `try`s the loop removals left, in preorder.
    fn remove_tries(&mut self, blocks: &mut [MirBlock], survivors: &mut Vec<usize>) {
        for block in blocks.iter_mut().rev() {
            let mut at = block.instrs.len();
            while at > 0 {
                at -= 1;
                for region in instruction_regions_mut(&mut block.instrs[at])
                    .into_iter()
                    .rev()
                {
                    self.remove_tries(region, survivors);
                }
                if matches!(block.instrs[at], MirInstr::Try { .. })
                    && let Some(id) = survivors.pop()
                    && self.census.elidable[id]
                {
                    self.remove_trivial_try(block, at);
                }
            }
        }
    }

    /// Replace the `try` at `at` of `block` by its body and, when the body
    /// completes, its `else`: each `try.yield`'s values replace the `else`'s
    /// arguments or the `try`'s results they define.
    fn remove_trivial_try(&mut self, block: &mut MirBlock, at: usize) {
        let instruction = block.instrs.remove(at);
        let MirInstr::Try {
            results,
            mut body,
            handler_args,
            handler,
            else_args,
            orelse,
            ..
        } = instruction
        else {
            block.instrs.insert(at, instruction);
            return;
        };
        self.erased.extend(handler_args.iter().map(|arg| arg.0));
        for handled in handler.iter().flatten() {
            for instruction in &handled.instrs {
                erased_results(instruction, &mut self.erased);
            }
        }
        self.erased_tries_count += 1;
        let completes = if orelse.is_some() {
            else_args
        } else {
            results.clone()
        };
        let parts = std::iter::once((body.pop(), completes))
            .chain(orelse.map(|mut part| (part.pop(), results.clone())));
        let mut end = at;
        for (part, defined) in parts {
            let Some(MirBlock { instrs, term }) = part else {
                return;
            };
            let inlined = instrs.len();
            block.instrs.splice(end..end, instrs);
            end += inlined;
            if let MirTerm::TryYield(values) = term {
                self.replaced
                    .extend(defined.iter().map(|defined| defined.0).zip(values));
            } else {
                let dead = defined.iter().chain(&results).map(|defined| defined.0);
                self.erased.extend(dead);
                end_block_at(block, end, term, &mut self.erased);
                return;
            }
        }
    }
}

/// `propagateImpliedCondition` over every `if` of `function`, in
/// postorder; returns how many implied a condition.
fn propagate_implied_conditions(function: &mut MirFunction) -> usize {
    let mut blocks = std::mem::take(&mut function.blocks);
    let mut constants = HashSet::new();
    constant_registers(&blocks, &mut constants);
    let mut implication = Implication {
        function,
        constants,
        implied: 0,
    };
    implication.walk(&mut blocks);
    let implied = implication.implied;
    function.blocks = blocks;
    implied
}

/// Collect the registers of `blocks`, at any depth, a `const` defines.
fn constant_registers(blocks: &[MirBlock], constants: &mut HashSet<u32>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::Const { dest, .. } = instruction {
            constants.insert(dest.0);
        }
        for region in instruction_regions(instruction) {
            constant_registers(region, constants);
        }
    }
}

/// The implied-condition walk: the body whose registers it adds, and the
/// registers a `const` defines.
struct Implication<'a> {
    function: &'a mut MirFunction,
    constants: HashSet<u32>,
    implied: usize,
}

impl Implication<'_> {
    fn walk(&mut self, blocks: &mut [MirBlock]) {
        for block in blocks {
            let mut at = 0;
            while at < block.instrs.len() {
                for region in instruction_regions_mut(&mut block.instrs[at]) {
                    self.walk(region);
                }
                if let Some((cond, value)) = self.implied_value(&block.instrs[at])
                    && self.imply(block, at, cond, value)
                {
                    at += 1;
                }
                at += 1;
            }
        }
    }

    /// The condition of the `if` `instruction` and the value it implies
    /// after it, if any.
    fn implied_value(&self, instruction: &MirInstr) -> Option<(Reg, bool)> {
        let MirInstr::If {
            cond,
            then,
            elifs,
            orelse,
            ..
        } = instruction
        else {
            return None;
        };
        if self.constants.contains(&cond.0) {
            return None;
        }
        let ends_early = |region: &[MirBlock]| region.first().is_some_and(|b| exits_early(&b.term));
        if ends_early(then) {
            Some((*cond, false))
        } else if elifs.is_empty() && ends_early(orelse) {
            Some((*cond, true))
        } else {
            None
        }
    }

    /// Replace the reads of `cond` after the `if` at `at` of `block`, at any
    /// depth and in its terminator, by a `const` of `value` inserted before
    /// the `if`; returns whether any read was.
    fn imply(&mut self, block: &mut MirBlock, at: usize, cond: Reg, value: bool) -> bool {
        let mut tail = [MirBlock {
            instrs: block.instrs.split_off(at + 1),
            term: std::mem::replace(&mut block.term, MirTerm::Unreachable),
        }];
        let mut reads = vec![0_u32; cond.0 as usize + 1];
        count_register_reads(&mut tail, &mut reads);
        let read = reads[cond.0 as usize] > 0;
        if read {
            let span = self
                .function
                .spans
                .0
                .get(&cond.0)
                .map(|(span, _)| span.clone());
            let ty = self
                .function
                .reg_types
                .get(&cond.0)
                .cloned()
                .unwrap_or_else(Ty::bool);
            let constant = fresh_register(self.function, ty, span.as_ref());
            self.constants.insert(constant.0);
            replace_register_reads(&mut tail, &HashMap::from([(cond.0, constant)]));
            block.instrs.insert(
                at,
                MirInstr::Const {
                    dest: constant,
                    k: Const::Bool(value),
                },
            );
            self.implied += 1;
        }
        let [MirBlock { instrs, term }] = tail;
        block.instrs.extend(instrs);
        block.term = term;
        read
    }
}
