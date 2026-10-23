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
//! its `match.next`s join, and a `try`'s handler enters as the raises
//! naming it join. A `break`, `continue`, or `match` exit resolves to its
//! region op through [`RegionFrame`] and [`region_exit`], and a labelled
//! `raise` to its `try`'s body frame. Anything that names a slot revives
//! it: over-reviving hides a finding, never invents one.

use super::{MirBlock, MirFunction, MirInstr, MirTerm};
use crate::mir::{
    MirCaptureMode, RegionFrame, VarId, instruction_regions, instruction_slots, region_exit,
};
use std::collections::HashMap;

/// Report every `drop.var` or `var.consume` in `function` whose slot every
/// path reaching it has already ended. A finding names the block of the
/// block list holding it: the function's, or a region's.
pub(super) fn verify_slot_ends(name: &str, function: &MirFunction, errors: &mut Vec<String>) {
    if !ends_slots(&function.blocks) {
        return;
    }
    let mut walk = Walk::new(name, function);
    walk.region(&vec![false; function.n_vars], &function.blocks, true);
    errors.extend(walk.findings);
}

/// One bit per slot: whether every path reaching this point ended it.
type Ended = Vec<bool>;

/// The states a region (or one block) leaves by: falling off, raising out
/// of the function, leaving the function, and leaving or resuming an
/// enclosing region op (a raise to a `try` leaves its body's frame).
#[derive(Default, Clone)]
struct Flow {
    normal: Option<Ended>,
    raises: Option<Ended>,
    exits: Option<Ended>,
    /// The enclosing frames this flow leaves or resumes: the frame's depth
    /// in [`Walk::frames`], whether it resumes it, and the states joined
    /// there.
    frame_exits: Vec<(usize, bool, Ended)>,
}

impl Flow {
    /// Join `other`'s abnormal channels into this flow.
    fn absorb(&mut self, other: Self) {
        meet(&mut self.raises, other.raises.as_ref());
        meet(&mut self.exits, other.exits.as_ref());
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
type Memo = HashMap<(*const MirInstr, Ended, usize), Flow>;

struct Walk<'a> {
    name: &'a str,
    function: &'a MirFunction,
    /// The slot a reference register was made from: a write through it
    /// stores into that slot again.
    refills: HashMap<u32, VarId>,
    findings: Vec<String>,
    /// The region ops enclosing the region being walked, innermost last.
    frames: Vec<RegionFrame>,
    /// A loop's fixpoint walks its body again, and each walk reaches the
    /// region ops nested in it, so without the memo the walks multiply
    /// with each loop of the nest.
    memo: Memo,
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
            frames: Vec::new(),
            memo: HashMap::new(),
        }
    }

    /// Walk `blocks`, a region's one block, from `entry`, reporting when
    /// `report`, and sort how it leaves by its terminator.
    fn region(&mut self, entry: &Ended, blocks: &'a [MirBlock], report: bool) -> Flow {
        let mut flow = Flow::default();
        let Some(block) = blocks.first() else {
            return flow;
        };
        let out = self.block(0, entry.clone(), block, report);
        match (&block.term, out.normal.as_ref()) {
            (
                MirTerm::TryYield(_)
                | MirTerm::Yield(_)
                | MirTerm::ComptimeYield(_)
                | MirTerm::ElifCond { .. },
                normal,
            ) => meet(&mut flow.normal, normal),
            (MirTerm::Return(_), exits) => meet(&mut flow.exits, exits),
            (MirTerm::Raise { label: None, .. }, raises) => meet(&mut flow.raises, raises),
            (
                MirTerm::Raise {
                    label: Some(label), ..
                },
                Some(state),
            ) => {
                if let Some(depth) = self
                    .frames
                    .iter()
                    .rposition(|&frame| RegionFrame::Try(*label).names(frame))
                {
                    flow.leave(depth, false, state);
                }
            }
            (term, Some(state)) => {
                if let Some((target, resume)) = region_exit(term)
                    && let Some(depth) = self.frames.iter().rposition(|&frame| target.names(frame))
                {
                    flow.leave(depth, resume, state);
                }
            }
            _ => {}
        }
        flow.absorb(out);
        flow
    }

    /// One block from `state`: its normal channel is the state after its
    /// terminator's own effect, and its other channels are those of its
    /// regions, a raising call's error region among them.
    fn block(&mut self, index: usize, mut state: Ended, block: &'a MirBlock, report: bool) -> Flow {
        let mut flow = Flow::default();
        for instruction in &block.instrs {
            if !instruction_regions(instruction).is_empty() {
                let mut regions = self.region_op(state, instruction, report);
                let normal = regions.normal.take();
                flow.absorb(regions);
                match normal {
                    Some(normal) => state = normal,
                    None => return flow,
                }
                continue;
            }
            self.step(index, &mut state, instruction, report);
        }
        flow.normal = Some(state);
        flow
    }

    /// [`Self::region_op_unmemoized`], computed once per op, entry state,
    /// and frame depth while not reporting. A reporting walk is made once
    /// per region, from the join of every state entering it.
    fn region_op(&mut self, state: Ended, instruction: &'a MirInstr, report: bool) -> Flow {
        if report {
            return self.region_op_unmemoized(state, instruction, true);
        }
        let key = (std::ptr::from_ref(instruction), state, self.frames.len());
        if let Some(known) = self.memo.get(&key) {
            return known.clone();
        }
        let flow = self.region_op_unmemoized(key.1.clone(), instruction, false);
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
        state: Ended,
        instruction: &'a MirInstr,
        report: bool,
    ) -> Flow {
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
                    let mut case = self.region(&start, &case.body, report);
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
            MirInstr::Try { .. } => return self.try_regions(state, instruction, report),
            _ => flow.normal = Some(state),
        }
        flow
    }

    /// Walk the region `blocks` from `entry`, joining its abnormal channels
    /// into `flow`; the state it completes in.
    fn enter(
        &mut self,
        flow: &mut Flow,
        entry: &Ended,
        blocks: &'a [MirBlock],
        report: bool,
    ) -> Option<Ended> {
        let mut region = self.region(entry, blocks, report);
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
    ) -> Flow {
        let depth = self.frames.len();
        self.frames.push(frame);
        let mut resume = state;
        let (mut flow, broken) = loop {
            let mut flow = self.region(&resume, body, false);
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
            flow = self.region(&resume, body, true);
            flow.take_frame(depth);
        }
        self.frames.pop();
        flow.normal = broken;
        flow
    }

    /// A `try` from `entry`, as `checkTryOp` walks it: the body under its
    /// own frame, the handler from the raises naming it, its binding
    /// refilled, and `orelse` from the body's fall-off; the `try`
    /// completes as the handler's and `orelse`'s (or the body's) fall-offs
    /// join.
    fn try_regions(&mut self, entry: Ended, instruction: &'a MirInstr, report: bool) -> Flow {
        let MirInstr::Try {
            label,
            body,
            handler,
            orelse,
            ..
        } = instruction
        else {
            return Flow {
                normal: Some(entry),
                ..Flow::default()
            };
        };
        let depth = self.frames.len();
        self.frames.push(RegionFrame::Try(*label));
        let mut body_flow = self.region(&entry, body, report);
        self.frames.pop();
        let (body_raises, _) = body_flow.take_frame(depth);
        let body_normal = body_flow.normal.take();
        let mut flow = body_flow;
        match orelse {
            Some(orelse) => {
                if let Some(start) = body_normal {
                    let arm = self.enter(&mut flow, &start, orelse, report);
                    meet(&mut flow.normal, arm.as_ref());
                }
            }
            None => meet(&mut flow.normal, body_normal.as_ref()),
        }
        if let (Some((binding, handler)), Some(mut start)) = (handler, body_raises) {
            if let Some(binding) = binding {
                set(&mut start, *binding, false);
            }
            let arm = self.enter(&mut flow, &start, handler, report);
            meet(&mut flow.normal, arm.as_ref());
        }
        flow
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

/// Whether some block of `blocks`, at any depth, ends a slot: a `drop.var`
/// or a `var.consume`.
fn ends_slots(blocks: &[MirBlock]) -> bool {
    blocks.iter().any(|block| {
        block.instrs.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::DropVar { .. } | MirInstr::ConsumeVar { .. }
            ) || instruction_regions(instruction)
                .into_iter()
                .any(|region| ends_slots(region))
        })
    })
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
