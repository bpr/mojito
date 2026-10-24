//! Slot ends: a `drop.var`, a `var.consume`, or a `deinit`-receiver call
//! consuming a whole slot (upstream's `memConsume` operand) must end a slot
//! every path reaching it holds, as upstream's `UninitializedValueScan` rejects a use,
//! a destroy among them, of a value some path has not initialized. So a
//! slot's state is static at every end, and a backend destroys it with no
//! drop flag; a slot some path has ended — dropped, consumed, or moved — or
//! never initialized would be destroyed twice, or uninitialized.
//!
//! A `drop.var`, a `drop.place`, or a `deinit`-receiver call destroys
//! what it names whole, so it must also find every part of it held, as
//! upstream's `ValueSet` proves field by field that a destroyed value has
//! no uninitialized field. A `var.consume` destroys nothing — drop
//! elaboration has destroyed every part the value still held — so it ends
//! a slot whatever parts some path has ended.
//!
//! An owned register ends at most once on every path, as `CheckLifetimes`
//! rejects a use of an owned SSA value after its consumption: an operand
//! that takes ownership of it ([`RegisterEnds::consumed`]), a terminator
//! carrying it out of its region, a `drop.reg`, or a `deinit`-receiver call
//! on it ends it, and its definition (each time a loop reaches it, a
//! region argument at its region's entry) starts it again. The walk keeps
//! the owned registers some path has ended beside the slots, joined by
//! union.
//!
//! The analysis is a must-analysis over one bit per slot ("held on every
//! path"), joined by AND as upstream's `tracked &=`, and the parts of held
//! slots some path has ended (a `place.move`, a `drop.place`, a
//! `deinit`-receiver call, or a `move` capture of a field or tuple
//! element), joined by union, over the function's
//! structured body, as `CheckLifetimes` walks `hlcf`: an `if` or `match` joins its
//! arms, a loop or `comptime for` iterates its body to a fixpoint and
//! leaves in the join of its `break`s, a `match` case enters the next as
//! its `match.next`s join, and a `try`'s handler enters as the raises
//! naming it join. A `break`, `continue`, or `match` exit resolves to its
//! region op through [`RegionFrame`] and [`region_exit`], and a labelled
//! `raise` to its `try`'s body frame. A function enters holding its
//! parameters, a `var.declare` empties a slot, and anything else that names
//! a slot fills it, and fills the part its place names: a read of an empty
//! slot or an ended part is the checker's error, and over-filling hides a
//! finding, never invents one.

use super::{
    MirBlock, MirFunction, MirInstr, MirTerm, instruction_places, instruction_result_regs,
};
use crate::mir::text::instruction_mnemonic;
use crate::mir::{
    MirCaptureMode, MirDeclarations, MirPlace, OwnedStep, Proj, Reg, RegionFrame, RegisterEnds,
    VarId, instruction_regions, instruction_slots, region_exit,
};
use std::collections::{BTreeSet, HashMap};

/// Report every slot end in `function` (a `drop.var`, a `var.consume`, or
/// a `deinit`-receiver call) whose slot some path reaching it does not
/// hold, and every destroy (a `drop.var`, a `drop.place`, or a
/// `deinit`-receiver call) of a value a part of which some path reaching
/// it has ended, and every end of an owned register some path reaching it
/// has ended. A finding names the block of the block list holding it: the
/// function's, or a region's.
pub(super) fn verify_slot_ends(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    let owned = RegisterEnds::new(function, declarations);
    if owned.is_empty() && !ends_slots(&function.blocks) {
        return;
    }
    let mut walk = Walk::new(name, function, owned);
    let entry = Held {
        slots: (0..function.n_vars)
            .map(|v| v < function.n_params)
            .collect(),
        ended: BTreeSet::new(),
        ended_regs: BTreeSet::new(),
    };
    walk.region(&entry, &function.blocks, true);
    errors.extend(walk.findings);
}

/// What every path reaching a point holds.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
struct Held {
    /// One bit per slot: whether every path reaching this point holds it.
    slots: Vec<bool>,
    /// The parts of slots some path reaching this point has ended, each
    /// the path of fields and tuple elements from its slot
    /// ([`MirPlace::owned_path`]); no entry lies inside another.
    ended: BTreeSet<(VarId, Vec<OwnedStep>)>,
    /// The owned registers some path reaching this point has ended since
    /// it last defined them.
    ended_regs: BTreeSet<u32>,
}

impl Held {
    /// Join `other` into this state: a slot is held where both hold it,
    /// and a part is ended where either ended it.
    fn meet(&mut self, other: &Self) {
        for (held, other) in self.slots.iter_mut().zip(&other.slots) {
            *held &= *other;
        }
        self.ended.extend(other.ended.iter().cloned());
        self.ended_regs.extend(other.ended_regs.iter().copied());
    }

    /// Start the registers `regs` again: their definition is reached.
    fn define(&mut self, regs: &[Reg]) {
        for reg in regs {
            self.ended_regs.remove(&reg.0);
        }
    }

    /// Whether every path holds `slot`; a slot out of range is the bounds
    /// check's finding.
    fn holds(&self, slot: VarId) -> bool {
        self.slots.get(slot as usize) != Some(&false)
    }

    /// Fill the part of `slot` at `path` (the whole slot when empty), and
    /// every part inside it.
    fn fill(&mut self, slot: VarId, path: &[OwnedStep]) {
        if let Some(bit) = self.slots.get_mut(slot as usize) {
            *bit = true;
        }
        self.ended
            .retain(|(at, part)| *at != slot || !part.starts_with(path));
    }

    /// End `slot` whole: no path holds it or any part of it.
    fn end(&mut self, slot: VarId) {
        if let Some(bit) = self.slots.get_mut(slot as usize) {
            *bit = false;
        }
        self.ended.retain(|(at, _)| *at != slot);
    }

    /// End the part of `slot` at `path`, unless a part holding it is
    /// already ended.
    fn end_part(&mut self, slot: VarId, path: Vec<OwnedStep>) {
        if self.parts(slot).any(|part| path.starts_with(part)) {
            return;
        }
        self.ended
            .retain(|(at, part)| *at != slot || !part.starts_with(&path));
        self.ended.insert((slot, path));
    }

    /// An ended part of `slot` that overlaps its part at `path`: one
    /// inside it, or one holding it.
    fn ended_within(&self, slot: VarId, path: &[OwnedStep]) -> Option<&[OwnedStep]> {
        self.parts(slot)
            .find(|part| part.starts_with(path) || path.starts_with(part))
    }

    /// The ended parts of `slot`.
    fn parts(&self, slot: VarId) -> impl Iterator<Item = &[OwnedStep]> {
        self.ended
            .iter()
            .filter(move |(at, _)| *at == slot)
            .map(|(_, part)| part.as_slice())
    }
}

/// The states a region (or one block) leaves by: falling off, raising out
/// of the function, leaving the function, and leaving or resuming an
/// enclosing region op (a raise to a `try` leaves its body's frame).
#[derive(Default, Clone)]
struct Flow {
    normal: Option<Held>,
    raises: Option<Held>,
    exits: Option<Held>,
    /// The enclosing frames this flow leaves or resumes: the frame's depth
    /// in [`Walk::frames`], whether it resumes it, and the states joined
    /// there.
    frame_exits: Vec<(usize, bool, Held)>,
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
    fn leave(&mut self, depth: usize, resume: bool, state: &Held) {
        match self
            .frame_exits
            .iter_mut()
            .find(|(at, resumes, _)| *at == depth && *resumes == resume)
        {
            Some((.., joined)) => joined.meet(state),
            None => self.frame_exits.push((depth, resume, state.clone())),
        }
    }

    /// Take the exits to the frame at `depth`: the states joined where it
    /// is left and where it is resumed.
    fn take_frame(&mut self, depth: usize) -> (Option<Held>, Option<Held>) {
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
type Memo = HashMap<(*const MirInstr, Held, usize), Flow>;

struct Walk<'a> {
    name: &'a str,
    function: &'a MirFunction,
    /// The registers holding an owned value whose end matters, and the
    /// operands that end them.
    owned: RegisterEnds<'a>,
    /// The place a reference register was made from: a write through it
    /// stores into that place again.
    refills: HashMap<u32, &'a MirPlace>,
    findings: Vec<String>,
    /// The region ops enclosing the region being walked, innermost last.
    frames: Vec<RegionFrame>,
    /// A loop's fixpoint walks its body again, and each walk reaches the
    /// region ops nested in it, so without the memo the walks multiply
    /// with each loop of the nest.
    memo: Memo,
}

impl<'a> Walk<'a> {
    fn new(name: &'a str, function: &'a MirFunction, owned: RegisterEnds<'a>) -> Self {
        let mut refills = HashMap::new();
        collect_refills(&function.blocks, &mut refills);
        Self {
            name,
            function,
            owned,
            refills,
            findings: Vec::new(),
            frames: Vec::new(),
            memo: HashMap::new(),
        }
    }

    /// Walk `blocks`, a region's one block, from `entry`, reporting when
    /// `report`, and sort how it leaves by its terminator.
    fn region(&mut self, entry: &Held, blocks: &'a [MirBlock], report: bool) -> Flow {
        let mut flow = Flow::default();
        let Some(block) = blocks.first() else {
            return flow;
        };
        let mut out = self.block(0, entry.clone(), block, report);
        if let Some(state) = out.normal.as_mut() {
            let carried = self.owned.consumed_by(&block.term);
            self.end_registers(0, state, &carried, "terminator", report);
        }
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
    fn block(&mut self, index: usize, mut state: Held, block: &'a MirBlock, report: bool) -> Flow {
        let mut flow = Flow::default();
        for instruction in &block.instrs {
            if !instruction_regions(instruction).is_empty() {
                self.instruction_registers(index, &mut state, instruction, report);
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
    fn region_op(&mut self, state: Held, instruction: &'a MirInstr, report: bool) -> Flow {
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
        state: Held,
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
            MirInstr::Loop {
                label, args, body, ..
            } => {
                return self.loop_body(state, RegionFrame::Loop(*label), args, body, report);
            }
            MirInstr::ComptimeFor { args, body, .. } => {
                return self.loop_body(state, RegionFrame::ComptimeFor, args, body, report);
            }
            MirInstr::Match {
                cases,
                else_args,
                orelse,
                ..
            } => {
                let depth = self.frames.len();
                let mut next = Some(state);
                for case in cases {
                    let Some(mut start) = next.take() else {
                        break;
                    };
                    start.define(&case.args);
                    self.frames.push(RegionFrame::MatchCase);
                    let mut case = self.region(&start, &case.body, report);
                    self.frames.pop();
                    let (complete, resume) = case.take_frame(depth);
                    meet(&mut flow.normal, complete.as_ref());
                    next = resume;
                    case.normal = None;
                    flow.absorb(case);
                }
                if let Some(mut next) = next {
                    next.define(else_args);
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
        entry: &Held,
        blocks: &'a [MirBlock],
        report: bool,
    ) -> Option<Held> {
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
        state: Held,
        frame: RegionFrame,
        args: &[Reg],
        body: &'a [MirBlock],
        report: bool,
    ) -> Flow {
        let depth = self.frames.len();
        self.frames.push(frame);
        let entered = |resume: &Held| {
            let mut entry = resume.clone();
            entry.define(args);
            entry
        };
        let mut resume = state;
        let (mut flow, broken) = loop {
            let mut flow = self.region(&entered(&resume), body, false);
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
            flow = self.region(&entered(&resume), body, true);
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
    fn try_regions(&mut self, entry: Held, instruction: &'a MirInstr, report: bool) -> Flow {
        let MirInstr::Try {
            label,
            body,
            handler_args,
            handler,
            else_args,
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
                if let Some(mut start) = body_normal {
                    start.define(else_args);
                    let arm = self.enter(&mut flow, &start, orelse, report);
                    meet(&mut flow.normal, arm.as_ref());
                }
            }
            None => meet(&mut flow.normal, body_normal.as_ref()),
        }
        if let (Some((binding, handler)), Some(mut start)) = (handler, body_raises) {
            if let Some(binding) = binding {
                start.fill(*binding, &[]);
            }
            start.define(handler_args);
            let arm = self.enter(&mut flow, &start, handler, report);
            meet(&mut flow.normal, arm.as_ref());
        }
        flow
    }

    /// The owned registers `instruction` ends, reported when some path
    /// reaching it has ended one already, then the registers it defines,
    /// which start again.
    fn instruction_registers(
        &mut self,
        index: usize,
        state: &mut Held,
        instruction: &MirInstr,
        report: bool,
    ) {
        let mut consumed = Vec::new();
        self.owned.consumed(instruction, &mut consumed);
        self.end_registers(
            index,
            state,
            &consumed,
            instruction_mnemonic(instruction),
            report,
        );
        let mut defined = Vec::new();
        instruction_result_regs(instruction, &mut defined);
        state.define(&defined);
    }

    /// End each owned register of `regs`, which `what` takes ownership of,
    /// reporting one some path reaching it has ended already.
    fn end_registers(
        &mut self,
        index: usize,
        state: &mut Held,
        regs: &[Reg],
        what: &str,
        report: bool,
    ) {
        for reg in regs {
            if !self.owned.owned(*reg) {
                continue;
            }
            if !state.ended_regs.insert(reg.0) && report {
                let finding = format!(
                    "MIR function '{}' block {index}: {what} ends owned register %r{}, which some path reaching it has ended",
                    self.name, reg.0
                );
                if !self.findings.contains(&finding) {
                    self.findings.push(finding);
                }
            }
        }
    }

    /// Record one finding: `what` ends slot `var`, which some path reaching
    /// it does not hold whole: not at all when `part` is `None`, else not
    /// the part at that path.
    fn report(&mut self, index: usize, what: &str, var: VarId, part: Option<&[OwnedStep]>) {
        let slot_name = self
            .function
            .var_names
            .get(var as usize)
            .map_or("?", String::as_str);
        let missing = match part {
            None => String::from("that some path reaching it does not hold"),
            Some(part) => format!(
                "whose part `{}` some path reaching it has ended",
                part_text(part)
            ),
        };
        let finding = format!(
            "MIR function '{}' block {index}: {what} of slot $v{var} ('{slot_name}') {missing}",
            self.name
        );
        if !self.findings.contains(&finding) {
            self.findings.push(finding);
        }
    }

    /// Report a destroy, spelled `what`, of the value `place` names, which
    /// some path reaching it does not hold whole. A place through a
    /// reference, or past a step that is no field or tuple element, names
    /// no part of its slot.
    fn report_destroy(&mut self, index: usize, state: &Held, what: &str, place: &MirPlace) {
        let path = if place.is_whole_root() {
            Vec::new()
        } else {
            match place.owned_path() {
                Some(path) => path,
                None => return,
            }
        };
        if !state.holds(place.root) {
            self.report(index, what, place.root, None);
        } else if let Some(part) = state.ended_within(place.root, &path) {
            let part = part.to_vec();
            self.report(index, what, place.root, Some(&part));
        }
    }

    /// One instruction's effect, reporting when `report` an end of a slot
    /// some path does not hold, or a destroy of a value a part of which
    /// some path has ended.
    fn step(&mut self, index: usize, state: &mut Held, instruction: &'a MirInstr, report: bool) {
        self.instruction_registers(index, state, instruction, report);
        if let MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } = instruction {
            if report {
                let drop = matches!(instruction, MirInstr::DropVar { .. });
                let spelling = if drop { "drop.var" } else { "var.consume" };
                if !state.holds(*var) {
                    self.report(index, spelling, *var, None);
                } else if drop && let Some(part) = state.ended_within(*var, &[]) {
                    let part = part.to_vec();
                    self.report(index, spelling, *var, Some(&part));
                }
            }
            state.end(*var);
            return;
        }
        if let MirInstr::DeclareVar { var } = instruction {
            state.end(*var);
            return;
        }
        // A `deinit`-receiver call ends its receiver as a `drop.place`
        // does, after the call has read it.
        let consumed = instruction.consumed_receiver();
        if report {
            if let MirInstr::DropPlace { place, .. } = instruction {
                self.report_destroy(index, state, "drop.place", place);
            }
            if let Some(place) = consumed {
                self.report_destroy(index, state, "call.method deinit receiver", place);
            }
        }
        let places = instruction_places(instruction);
        for place in &places {
            state.fill(place.root, &owned_prefix(place));
        }
        for slot in instruction_slots(instruction) {
            if !places.iter().any(|place| place.root == slot) {
                state.fill(slot, &[]);
            }
        }
        if let MirInstr::WriteRef { reference, .. } = instruction
            && let Some(place) = self.refills.get(&reference.0)
        {
            state.fill(place.root, &owned_prefix(place));
        }
        if let Some(source) = instruction.consumed_iteration_source() {
            state.end(source);
        }
        let ended: Vec<&MirPlace> = match instruction {
            MirInstr::UseVar { var, mode, .. } if mode.moves() => {
                state.end(*var);
                Vec::new()
            }
            MirInstr::MovePlace { place, .. }
            | MirInstr::DropPlace { place, .. }
            | MirInstr::MarkDestroyed { place } => vec![place],
            MirInstr::MakeClosure { captures, .. } => captures
                .iter()
                .filter(|capture| capture.mode == MirCaptureMode::Move)
                .map(|capture| &capture.place)
                .collect(),
            _ => Vec::new(),
        };
        for place in ended.into_iter().chain(consumed) {
            if place.is_whole_root() {
                state.end(place.root);
            } else if let Some(path) = place.owned_path() {
                state.end_part(place.root, path);
            }
        }
    }
}

/// AND `source` into `target`; an unreached `target` takes `source` whole.
fn meet(target: &mut Option<Held>, source: Option<&Held>) {
    let Some(source) = source else {
        return;
    };
    match target {
        Some(target) => target.meet(source),
        None => *target = Some(source.clone()),
    }
}

/// The part of its slot a place reaches through its leading fields and
/// tuple elements: what a write to it fills, at the least.
fn owned_prefix(place: &MirPlace) -> Vec<OwnedStep> {
    let mut path = Vec::new();
    for step in &place.proj {
        match step {
            Proj::Field(name) => path.push(OwnedStep::Field(name.clone())),
            Proj::ConstIndex(index) => path.push(OwnedStep::Element(*index)),
            Proj::Rebind => {}
            Proj::Index(_) | Proj::Variant(_) | Proj::UninitPayload => break,
        }
    }
    path
}

/// A part's path as source spells it: its fields and elements, dotted.
fn part_text(part: &[OwnedStep]) -> String {
    part.iter()
        .map(|step| match step {
            OwnedStep::Field(name) => name.clone(),
            OwnedStep::Element(index) => index.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Whether some block of `blocks`, at any depth, ends a slot or destroys a
/// part: a `drop.var`, a `var.consume`, a `drop.place`, or a
/// `deinit`-receiver call.
fn ends_slots(blocks: &[MirBlock]) -> bool {
    blocks.iter().any(|block| {
        block.instrs.iter().any(|instruction| {
            matches!(
                instruction,
                MirInstr::DropVar { .. } | MirInstr::ConsumeVar { .. } | MirInstr::DropPlace { .. }
            ) || instruction.consumed_receiver().is_some()
                || instruction_regions(instruction)
                    .into_iter()
                    .any(|region| ends_slots(region))
        })
    })
}

/// The place each reference register of `blocks`, at any depth, was made
/// from.
fn collect_refills<'a>(blocks: &'a [MirBlock], refills: &mut HashMap<u32, &'a MirPlace>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::MakeRef { dest, place } = instruction {
            refills.insert(dest.0, place);
        }
        for region in instruction_regions(instruction) {
            collect_refills(region, refills);
        }
    }
}
