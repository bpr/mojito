//! Slot ends: a `drop.var` or `var.consume` of a slot that every path
//! reaching it has already ended — dropped, consumed, or moved — destroys a
//! value twice. Upstream's destructor insertion destroys each value once on
//! a live path; this check holds the elaborated MIR there.
//!
//! The analysis is a must-analysis over one bit per slot ("ended on every
//! path"), joined by AND, over the function's structured body, as
//! upstream's `CheckLifetimes` walks `hlcf`: an `if` or `match` joins its
//! arms, a loop or `comptime for` iterates its body to a fixpoint and
//! leaves in the join of its `break`s, a `match` case enters the next as
//! its `match.next`s join, and a `try` is walked as the ownership analysis
//! walks it. A `break`, `continue`, or `match` exit resolves to its region
//! op through [`RegionFrame`] and [`region_exit`], running each `finally`
//! it crosses. Older MIR text whose block lists branch, and whose regions
//! escape to the function's blocks, is walked the same way, each list to
//! a fixpoint. Anything that names a slot revives it: over-reviving hides
//! a finding, never invents one.
//!
//! The three cleanup lists end what they name where they run, and a listed
//! slot every path has already ended is the same finding. A `try`'s list
//! runs on its body's raise edge, before the handler or the `finally`; an
//! escape's runs at the escape, before the `finally` blocks it crosses; a
//! return's runs after the last `finally` it crosses, as the function is
//! left. A `finally` that returns itself overrides a pending return, whose
//! list it then runs unchecked here.
//!
//! The same walk cuts a `try`'s list once its body's raise points change
//! ([`cut_try_cleanups`]): an entry every raise path has already ended is
//! one only an erased raise point held.

use super::{MirBlock, MirFunction, MirInstr, MirTerm};
use crate::mir::{
    CleanupDrop, MirBlockId, MirCaptureMode, RegionFrame, VarId, cleanup_slots,
    instruction_regions, instruction_regions_mut, instruction_slots, region_exit,
    terminator_targets,
};
use std::collections::HashMap;

/// Remove from each `try`'s raise-edge cleanup in `function` the slots
/// every path to its body's raise points has already ended, as erasing a
/// raise point erases the destructors on its edge.
pub fn cut_try_cleanups(function: &mut MirFunction) {
    if !function
        .blocks
        .iter()
        .any(|block| holds_cleanup(&block.instrs))
    {
        return;
    }
    let ended = Walk::new("", function).ended_cleanups();
    if !ended.is_empty() {
        cut_listed(&mut function.blocks, &ended);
    }
}

/// Report every `drop.var` or `var.consume` in `function` whose slot every
/// path reaching it has already ended. A finding names the block of the
/// block list holding it: the function's, or a region's.
pub(super) fn verify_slot_ends(name: &str, function: &MirFunction, errors: &mut Vec<String>) {
    if !ends_slots(&function.blocks) {
        return;
    }
    let mut walk = Walk::new(name, function);
    let flow = walk.region(&vec![false; function.n_vars], &function.blocks, true, true);
    for (index, cleanup, state) in flow.returns {
        walk.end_listed(index, "return.cleanup", &mut state.clone(), cleanup, true);
    }
    errors.extend(walk.findings);
}

/// One bit per slot: whether every path reaching this point ended it.
type Ended = Vec<bool>;

/// The states a region (or one block) leaves by: falling off, raising,
/// leaving the function, escaping to a function block, and leaving or
/// resuming an enclosing region op. A return carrying a cleanup list is
/// also kept apart, with its block and list, until it leaves the function.
#[derive(Default, Clone)]
struct Flow<'a> {
    normal: Option<Ended>,
    raises: Option<Ended>,
    exits: Option<Ended>,
    escapes: Vec<(MirBlockId, Ended)>,
    returns: Vec<(usize, &'a [CleanupDrop], Ended)>,
    /// The enclosing frames this flow leaves or resumes: the frame's depth
    /// in [`Walk::frames`], whether it resumes it, and the states joined
    /// there.
    frame_exits: Vec<(usize, bool, Ended)>,
}

impl Flow<'_> {
    /// Join `other`'s abnormal channels into this flow.
    fn absorb(&mut self, other: Self) {
        meet(&mut self.raises, other.raises.as_ref());
        meet(&mut self.exits, other.exits.as_ref());
        self.escapes.extend(other.escapes);
        self.returns.extend(other.returns);
        for (depth, resume, state) in other.frame_exits {
            self.leave(depth, resume, &state);
        }
    }

    /// Join `state` into the exit to the frame at `depth`, resuming it
    /// when `resume`.
    fn leave(&mut self, depth: usize, resume: bool, state: &Ended) {
        match self
            .frame_exits
            .iter_mut()
            .find(|(at, resumes, _)| *at == depth && *resumes == resume)
        {
            Some((.., joined)) => {
                for (ended, other) in joined.iter_mut().zip(state) {
                    *ended &= *other;
                }
            }
            None => self.frame_exits.push((depth, resume, state.clone())),
        }
    }

    /// Take the exits to the frame at `depth`: the states joined where it
    /// is left and where it is resumed.
    fn take_frame(&mut self, depth: usize) -> (Option<Ended>, Option<Ended>) {
        let mut leave = None;
        let mut resume = None;
        self.frame_exits.retain(|(at, resumes, state)| {
            if *at != depth {
                return true;
            }
            meet(if *resumes { &mut resume } else { &mut leave }, Some(state));
            false
        });
        (leave, resume)
    }
}

/// The non-reporting flows of the region ops one walk computed, by op (its
/// address in the body, which the walk does not change), entry state, and
/// the depth of the frames around it.
type Memo<'a> = HashMap<(*const MirInstr, Ended, usize), Flow<'a>>;

struct Walk<'a> {
    name: &'a str,
    function: &'a MirFunction,
    /// The slot a reference register was made from: a write through it
    /// stores into that slot again.
    refills: HashMap<u32, VarId>,
    findings: Vec<String>,
    /// Each `try`'s cleanup list, by its buffer, and a slot it lists that
    /// every path to its raise edge has already ended.
    ended_cleanups: Vec<(*const CleanupDrop, VarId)>,
    /// The region ops enclosing the region being walked, innermost last.
    frames: Vec<RegionFrame>,
    /// A loop's fixpoint walks its body again, and each walk reaches the
    /// region ops nested in it, so without the memo the walks multiply
    /// with each loop of the nest.
    memo: Memo<'a>,
}

impl<'a> Walk<'a> {
    fn new(name: &'a str, function: &'a MirFunction) -> Self {
        let mut refills = HashMap::new();
        collect_refills(&function.blocks, &mut refills);
        Self {
            name,
            function,
            refills,
            findings: Vec::new(),
            ended_cleanups: Vec::new(),
            frames: Vec::new(),
            memo: HashMap::new(),
        }
    }

    /// The cleanup entries of `function`, each by its list, that every path
    /// to their `try`'s raise edge has already ended.
    fn ended_cleanups(mut self) -> Vec<(*const CleanupDrop, VarId)> {
        let function = self.function;
        self.region(&vec![false; function.n_vars], &function.blocks, true, true);
        self.ended_cleanups
    }

    /// Iterate `blocks` to a fixpoint from `entry`, then, when `report`, make
    /// one reporting pass over the reached blocks. At the function's `top`
    /// level the escapes of its regions feed their target blocks.
    fn region(
        &mut self,
        entry: &Ended,
        blocks: &'a [MirBlock],
        top: bool,
        report: bool,
    ) -> Flow<'a> {
        let mut predecessors = vec![Vec::new(); blocks.len()];
        for (index, block) in blocks.iter().enumerate() {
            for target in terminator_targets(&block.term) {
                if let Some(list) = predecessors.get_mut(target) {
                    list.push(index);
                }
            }
        }
        let mut inputs: Vec<Option<Ended>> = vec![None; blocks.len()];
        let mut outputs: Vec<Option<Flow>> = (0..blocks.len()).map(|_| None).collect();
        loop {
            let mut changed = false;
            for index in 0..blocks.len() {
                let mut input = (index == 0).then(|| entry.clone());
                for &predecessor in &predecessors[index] {
                    let out = outputs[predecessor].as_ref();
                    meet(&mut input, out.and_then(|flow| flow.normal.as_ref()));
                }
                if top {
                    for (target, state) in outputs.iter().flatten().flat_map(|flow| &flow.escapes) {
                        if *target == index {
                            meet(&mut input, Some(state));
                        }
                    }
                }
                if input.is_none() || inputs[index] == input {
                    continue;
                }
                let state = input.clone().unwrap_or_default();
                outputs[index] = Some(self.block(index, state, &blocks[index], false));
                inputs[index] = input;
                changed = true;
            }
            if !changed {
                break;
            }
        }
        let mut flow = Flow::default();
        for (index, block) in blocks.iter().enumerate() {
            let Some(state) = inputs[index].clone() else {
                continue;
            };
            let out = if report {
                self.block(index, state, block, true)
            } else {
                outputs[index].take().unwrap_or_default()
            };
            match (&block.term, out.normal.as_ref()) {
                (
                    MirTerm::TryYield
                    | MirTerm::Yield(_)
                    | MirTerm::ComptimeYield(_)
                    | MirTerm::ElifCond { .. },
                    normal,
                ) => meet(&mut flow.normal, normal),
                (MirTerm::ReturnWithCleanup { cleanup, .. }, Some(state))
                    if !cleanup.is_empty() =>
                {
                    meet(&mut flow.exits, Some(state));
                    flow.returns.push((index, cleanup, state.clone()));
                }
                (MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. }, exits) => {
                    meet(&mut flow.exits, exits);
                }
                (MirTerm::EscapeJump { target, cleanup }, Some(state)) if !top => {
                    let mut state = state.clone();
                    self.end_listed(index, "escape cleanup", &mut state, cleanup, report);
                    flow.escapes.push((*target, state));
                }
                (term, Some(state)) => {
                    if let Some((target, resume)) = region_exit(term)
                        && let Some(depth) =
                            self.frames.iter().rposition(|&frame| target.names(frame))
                    {
                        flow.leave(depth, resume, state);
                    }
                }
                _ => {}
            }
            let escapes = if top { Vec::new() } else { out.escapes };
            flow.absorb(Flow { escapes, ..out });
        }
        flow
    }

    /// One block from `state`: its normal channel is the state after its
    /// terminator's own effect, and its other channels are those of its
    /// raise points and the regions it holds.
    fn block(
        &mut self,
        index: usize,
        mut state: Ended,
        block: &'a MirBlock,
        report: bool,
    ) -> Flow<'a> {
        let mut flow = Flow::default();
        for instruction in &block.instrs {
            if !instruction_regions(instruction).is_empty() {
                let mut regions = self.region_op(index, state, instruction, report);
                let normal = regions.normal.take();
                flow.absorb(regions);
                match normal {
                    Some(normal) => state = normal,
                    None => return flow,
                }
                continue;
            }
            self.step(index, &mut state, instruction, report);
            if instruction.is_raise_point() {
                meet(&mut flow.raises, Some(&state));
            }
            if matches!(instruction, MirInstr::Raise { .. }) {
                return flow;
            }
        }
        flow.normal = Some(state);
        flow
    }

    /// [`Self::region_op_unmemoized`], computed once per op, entry state,
    /// and frame depth while not reporting. A reporting walk is made once
    /// per region, from the join of every state entering it.
    fn region_op(
        &mut self,
        index: usize,
        state: Ended,
        instruction: &'a MirInstr,
        report: bool,
    ) -> Flow<'a> {
        if report {
            return self.region_op_unmemoized(index, state, instruction, true);
        }
        let key = (std::ptr::from_ref(instruction), state, self.frames.len());
        if let Some(known) = self.memo.get(&key) {
            return known.clone();
        }
        let flow = self.region_op_unmemoized(index, key.1.clone(), instruction, false);
        self.memo.insert(key, flow.clone());
        flow
    }

    /// The region op `instruction` from `state`, as upstream's
    /// `CheckLifetimes` walks it: its normal channel is the state after
    /// the op.
    ///
    /// - An `if` enters its then arm and its first condition as the op is,
    ///   each arm after a condition and the next condition (or the else
    ///   arm) as that condition yields, and completes as its arms join.
    /// - A `comptime if` enters every arm and its else arm as the op is.
    /// - A loop or `comptime for` enters its body as the op is, joined with
    ///   the states its `continue`s carry, until that join is stable, and
    ///   completes as its `break`s join; a `comptime for`'s else region is
    ///   never entered.
    /// - A `match` enters its first case as the op is, each later case and
    ///   the else region as the previous case's `match.next`s join, and
    ///   completes as its `match.complete`s and the else region join.
    /// - A `try` is [`Self::try_regions`].
    fn region_op_unmemoized(
        &mut self,
        index: usize,
        state: Ended,
        instruction: &'a MirInstr,
        report: bool,
    ) -> Flow<'a> {
        let mut flow = Flow::default();
        match instruction {
            MirInstr::If {
                then,
                elifs,
                orelse,
                ..
            } => {
                let arm = self.enter(&mut flow, &state, then, report);
                meet(&mut flow.normal, arm.as_ref());
                let mut rest = Some(state);
                for elif in elifs {
                    let Some(start) = rest.take() else {
                        break;
                    };
                    rest = self.enter(&mut flow, &start, &elif.cond, report);
                    if let Some(cond) = &rest {
                        let arm = self.enter(&mut flow, cond, &elif.then, report);
                        meet(&mut flow.normal, arm.as_ref());
                    }
                }
                if let Some(rest) = rest {
                    let arm = self.enter(&mut flow, &rest, orelse, report);
                    meet(&mut flow.normal, arm.as_ref());
                }
            }
            MirInstr::ComptimeIf { arms, orelse, .. } => {
                for arm in arms.iter().chain(std::iter::once(orelse)) {
                    let arm = self.enter(&mut flow, &state, arm, report);
                    meet(&mut flow.normal, arm.as_ref());
                }
            }
            MirInstr::Loop { label, body, .. } => {
                return self.loop_body(state, RegionFrame::Loop(*label), body, report);
            }
            MirInstr::ComptimeFor { body, .. } => {
                return self.loop_body(state, RegionFrame::ComptimeFor, body, report);
            }
            MirInstr::Match { cases, orelse, .. } => {
                let depth = self.frames.len();
                let mut next = Some(state);
                for case in cases {
                    let Some(start) = next.take() else {
                        break;
                    };
                    self.frames.push(RegionFrame::MatchCase);
                    let mut case = self.region(&start, &case.body, false, report);
                    self.frames.pop();
                    let (complete, resume) = case.take_frame(depth);
                    meet(&mut flow.normal, complete.as_ref());
                    next = resume;
                    case.normal = None;
                    flow.absorb(case);
                }
                if let Some(next) = next {
                    let arm = self.enter(&mut flow, &next, orelse, report);
                    meet(&mut flow.normal, arm.as_ref());
                }
            }
            MirInstr::Try { .. } => return self.try_regions(index, state, instruction, report),
            _ => flow.normal = Some(state),
        }
        flow
    }

    /// Walk the region `blocks` from `entry`, joining its abnormal channels
    /// into `flow`; the state it completes in.
    fn enter(
        &mut self,
        flow: &mut Flow<'a>,
        entry: &Ended,
        blocks: &'a [MirBlock],
        report: bool,
    ) -> Option<Ended> {
        let mut region = self.region(entry, blocks, false, report);
        let normal = region.normal.take();
        flow.absorb(region);
        normal
    }

    /// A loop or `comptime for` whose body is `body`, entered in `state`:
    /// the body is walked from the join of the entry and its `continue`s
    /// until that join is stable, and once more to report, and the op
    /// completes as its `break`s join.
    fn loop_body(
        &mut self,
        state: Ended,
        frame: RegionFrame,
        body: &'a [MirBlock],
        report: bool,
    ) -> Flow<'a> {
        let depth = self.frames.len();
        self.frames.push(frame);
        let mut resume = state;
        let (mut flow, broken) = loop {
            let mut flow = self.region(&resume, body, false, false);
            let (broken, continued) = flow.take_frame(depth);
            let mut next = Some(resume.clone());
            meet(&mut next, continued.as_ref());
            let next = next.unwrap_or_default();
            if next == resume {
                break (flow, broken);
            }
            resume = next;
        };
        if report {
            flow = self.region(&resume, body, false, true);
            flow.take_frame(depth);
        }
        self.frames.pop();
        flow.normal = broken;
        flow
    }
    /// A `try` from `entry`, as the ownership analysis walks it: the handler
    /// starts from the body's raises past the raise-edge cleanup, `orelse`
    /// from its normal fall-off, and `finally` runs on every channel. An
    /// escape, a region exit, or a return leaves through the normal
    /// fall-off of the `finally` it crosses.
    fn try_regions(
        &mut self,
        index: usize,
        entry: Ended,
        instruction: &'a MirInstr,
        report: bool,
    ) -> Flow<'a> {
        let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } = instruction
        else {
            return Flow {
                normal: Some(entry),
                ..Flow::default()
            };
        };
        let mut body_flow = self.region(&entry, body, false, report);
        let body_normal = body_flow.normal.take();
        let body_raises = body_flow.raises.take().map(|mut state| {
            if report {
                self.ended_cleanups.extend(
                    cleanup_slots(cleanup)
                        .filter(|&var| state.get(var as usize) == Some(&true))
                        .map(|var| (cleanup.as_ptr(), var)),
                );
            }
            self.end_listed(index, "try.cleanup", &mut state, cleanup, report);
            state
        });
        let mut flow = Flow {
            exits: body_flow.exits,
            escapes: body_flow.escapes,
            returns: body_flow.returns,
            frame_exits: body_flow.frame_exits,
            ..Flow::default()
        };
        match orelse {
            Some(orelse) => {
                if let Some(start) = body_normal {
                    let mut else_flow = self.region(&start, orelse, false, report);
                    meet(&mut flow.normal, else_flow.normal.take().as_ref());
                    flow.absorb(else_flow);
                }
            }
            None => meet(&mut flow.normal, body_normal.as_ref()),
        }
        match handler {
            Some((binding, handler)) => {
                if let Some(mut start) = body_raises {
                    if let Some(binding) = binding {
                        set(&mut start, *binding, false);
                    }
                    let mut handler_flow = self.region(&start, handler, false, report);
                    meet(&mut flow.normal, handler_flow.normal.take().as_ref());
                    flow.absorb(handler_flow);
                }
            }
            None => meet(&mut flow.raises, body_raises.as_ref()),
        }
        let Some(finalbody) = finalbody else {
            return flow;
        };
        if report {
            let mut all = flow.normal.clone();
            meet(&mut all, flow.raises.as_ref());
            meet(&mut all, flow.exits.as_ref());
            for (_, state) in &flow.escapes {
                meet(&mut all, Some(state));
            }
            let returns = flow.returns.iter().map(|(.., state)| state);
            for state in returns.chain(flow.frame_exits.iter().map(|(.., state)| state)) {
                meet(&mut all, Some(state));
            }
            if let Some(all) = all {
                self.region(&all, finalbody, false, true);
            }
        }
        let mut out = Flow::default();
        let mut through = |walk: &mut Self, state: Option<Ended>| {
            let mut finally = walk.region(&state?, finalbody, false, false);
            let normal = finally.normal.take();
            out.absorb(finally);
            normal
        };
        let normal = through(self, flow.normal);
        let raises = through(self, flow.raises);
        let exits = through(self, flow.exits);
        let escapes: Vec<_> = flow
            .escapes
            .into_iter()
            .filter_map(|(target, state)| Some((target, through(self, Some(state))?)))
            .collect();
        let returns: Vec<_> = flow
            .returns
            .into_iter()
            .filter_map(|(block, cleanup, state)| {
                Some((block, cleanup, through(self, Some(state))?))
            })
            .collect();
        let frame_exits: Vec<_> = flow
            .frame_exits
            .into_iter()
            .filter_map(|(depth, resume, state)| Some((depth, resume, through(self, Some(state))?)))
            .collect();
        out.normal = normal;
        meet(&mut out.raises, raises.as_ref());
        meet(&mut out.exits, exits.as_ref());
        out.escapes.extend(escapes);
        out.returns.extend(returns);
        for (depth, resume, state) in frame_exits {
            out.leave(depth, resume, &state);
        }
        out
    }

    /// A cleanup list's run: each listed slot ends, and one every path has
    /// already ended is reported when `report`.
    fn end_listed(
        &mut self,
        index: usize,
        list: &str,
        state: &mut Ended,
        cleanup: &[CleanupDrop],
        report: bool,
    ) {
        for var in cleanup_slots(cleanup) {
            if report && state.get(var as usize) == Some(&true) {
                self.report(index, list, var);
            }
            set(state, var, true);
        }
    }

    /// Record one finding: `what` ends slot `var`, which every path reaching
    /// it has already ended.
    fn report(&mut self, index: usize, what: &str, var: VarId) {
        let slot_name = self
            .function
            .var_names
            .get(var as usize)
            .map_or("?", String::as_str);
        let finding = format!(
            "MIR function '{}' block {index}: {what} of slot $v{var} ('{slot_name}') that every path has already ended",
            self.name
        );
        if !self.findings.contains(&finding) {
            self.findings.push(finding);
        }
    }

    /// One instruction's effect, reporting a drop or consume of a slot
    /// already ended on every path when `report`.
    fn step(&mut self, index: usize, state: &mut Ended, instruction: &MirInstr, report: bool) {
        if let MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } = instruction {
            if report && state.get(*var as usize) == Some(&true) {
                let spelling = if matches!(instruction, MirInstr::DropVar { .. }) {
                    "drop.var"
                } else {
                    "var.consume"
                };
                self.report(index, spelling, *var);
            }
            set(state, *var, true);
            return;
        }
        for slot in instruction_slots(instruction) {
            set(state, slot, false);
        }
        if let MirInstr::WriteRef { reference, .. } = instruction
            && let Some(&slot) = self.refills.get(&reference.0)
        {
            set(state, slot, false);
        }
        match instruction {
            MirInstr::UseVar { var, mode, .. } if mode.moves() => set(state, *var, true),
            MirInstr::MovePlace { place, .. }
            | MirInstr::ConsumePlace { place, .. }
            | MirInstr::DropPlace { place, .. }
            | MirInstr::MarkDestroyed { place }
                if place.is_whole_root() =>
            {
                set(state, place.root, true);
            }
            MirInstr::MakeClosure { captures, .. } => {
                for capture in captures {
                    if capture.mode == MirCaptureMode::Move && capture.place.is_whole_root() {
                        set(state, capture.place.root, true);
                    }
                }
            }
            _ => {}
        }
    }
}

/// AND `source` into `target`; an unreached `target` takes `source` whole.
fn meet(target: &mut Option<Ended>, source: Option<&Ended>) {
    let Some(source) = source else {
        return;
    };
    match target {
        Some(target) => {
            for (ended, other) in target.iter_mut().zip(source) {
                *ended &= *other;
            }
        }
        None => *target = Some(source.clone()),
    }
}

/// Set one slot's bit; a slot out of range is the bounds check's finding.
fn set(state: &mut Ended, slot: VarId, ended: bool) {
    if let Some(bit) = state.get_mut(slot as usize) {
        *bit = ended;
    }
}

/// Whether some block of `blocks`, at any depth, ends a slot: a `drop.var`,
/// a `var.consume`, or a cleanup list naming one.
fn ends_slots(blocks: &[MirBlock]) -> bool {
    blocks.iter().any(|block| {
        matches!(&block.term, MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } if !cleanup.is_empty())
            || block.instrs.iter().any(|instruction| {
                matches!(instruction, MirInstr::DropVar { .. } | MirInstr::ConsumeVar { .. })
                    || matches!(instruction, MirInstr::Try { cleanup, .. } if !cleanup.is_empty())
                    || instruction_regions(instruction)
                        .into_iter()
                        .any(|region| ends_slots(region))
            })
    })
}

/// Whether `instructions` hold a `try` with a raise-edge cleanup, at any
/// depth.
fn holds_cleanup(instructions: &[MirInstr]) -> bool {
    instructions.iter().any(|instruction| {
        matches!(instruction, MirInstr::Try { cleanup, .. } if !cleanup.is_empty())
            || instruction_regions(instruction)
                .into_iter()
                .flatten()
                .any(|block| holds_cleanup(&block.instrs))
    })
}

/// Drop from each `try` of `blocks`, at any depth, the cleanup entries
/// `ended` names for its list.
fn cut_listed(blocks: &mut [MirBlock], ended: &[(*const CleanupDrop, VarId)]) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        if let MirInstr::Try { cleanup, .. } = instruction {
            let list = cleanup.as_ptr();
            cleanup.retain(|drop| !ended.contains(&(list, drop.var)));
        }
        for region in instruction_regions_mut(instruction) {
            cut_listed(region, ended);
        }
    }
}

/// The slot each reference register of `blocks`, at any depth, was made
/// from.
fn collect_refills(blocks: &[MirBlock], refills: &mut HashMap<u32, VarId>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::MakeRef { dest, place } = instruction {
            refills.insert(dest.0, place.root);
        }
        for region in instruction_regions(instruction) {
            collect_refills(region, refills);
        }
    }
}
