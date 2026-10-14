//! Slot ends: a `drop.var` or `var.consume` of a slot that every path
//! reaching it has already ended — dropped, consumed, or moved — destroys a
//! value twice. Upstream's destructor insertion destroys each value once on
//! a live path; this check holds the elaborated MIR there.
//!
//! The analysis is a must-analysis over one bit per slot ("ended on every
//! path"), joined by AND, over the function's blocks, its `try` regions
//! (walked as the ownership analysis walks them), and the escape edges a
//! region's `break`/`continue` takes to the function's blocks. Anything that
//! names a slot revives it: over-reviving hides a finding, never invents one.
//! The implicit ends of a `return`'s, an escape's, and a `try`'s cleanup
//! list are not modelled, which under-approximates "ended" and stays sound.

use super::{MirBlock, MirFunction, MirInstr, MirTerm};
use crate::mir::{MirBlockId, MirCaptureMode, VarId, instruction_slots, terminator_targets};
use std::collections::HashMap;

/// Report every `drop.var` or `var.consume` in `function` whose slot every
/// path reaching it has already ended.
pub(super) fn verify_slot_ends(name: &str, function: &MirFunction, errors: &mut Vec<String>) {
    let mut refills = HashMap::new();
    collect_refills(&function.blocks, &mut refills);
    let mut walk = Walk {
        name,
        function,
        refills,
        findings: Vec::new(),
    };
    walk.region(&vec![false; function.n_vars], &function.blocks, true, true);
    errors.extend(walk.findings);
}

/// One bit per slot: whether every path reaching this point ended it.
type Ended = Vec<bool>;

/// The states a region (or one block) leaves by: falling off, raising,
/// leaving the function, and escaping to a function block.
#[derive(Default)]
struct Flow {
    normal: Option<Ended>,
    raises: Option<Ended>,
    exits: Option<Ended>,
    escapes: Vec<(MirBlockId, Ended)>,
}

impl Flow {
    /// Join `other`'s abnormal channels into this flow.
    fn absorb(&mut self, other: Self) {
        meet(&mut self.raises, other.raises.as_ref());
        meet(&mut self.exits, other.exits.as_ref());
        self.escapes.extend(other.escapes);
    }
}

struct Walk<'a> {
    name: &'a str,
    function: &'a MirFunction,
    /// The slot a reference register was made from: a write through it
    /// stores into that slot again.
    refills: HashMap<u32, VarId>,
    findings: Vec<String>,
}

impl Walk<'_> {
    /// Iterate `blocks` to a fixpoint from `entry`, then, when `report`, make
    /// one reporting pass over the reached blocks. At the function's `top`
    /// level the escapes of its regions feed their target blocks.
    fn region(&mut self, entry: &Ended, blocks: &[MirBlock], top: bool, report: bool) -> Flow {
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
                (MirTerm::FallOff, normal) => meet(&mut flow.normal, normal),
                (MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. }, exits) => {
                    meet(&mut flow.exits, exits);
                }
                (MirTerm::EscapeJump { target, .. }, Some(state)) if !top => {
                    flow.escapes.push((*target, state.clone()));
                }
                _ => {}
            }
            let escapes = if top { Vec::new() } else { out.escapes };
            flow.absorb(Flow { escapes, ..out });
        }
        flow
    }

    /// One block from `state`: its normal channel is the state after its
    /// terminator's own effect, and its raises, exits, and escapes are those
    /// of its raise points and the regions it holds.
    fn block(&mut self, index: usize, mut state: Ended, block: &MirBlock, report: bool) -> Flow {
        let mut flow = Flow::default();
        for instruction in &block.instrs {
            if matches!(instruction, MirInstr::Try { .. }) {
                let mut regions = self.try_regions(state, instruction, report);
                let normal = regions.normal.take();
                flow.absorb(regions);
                match normal {
                    Some(normal) => state = normal,
                    None => return flow,
                }
                continue;
            }
            self.step(index, &mut state, instruction, report);
            if matches!(instruction, MirInstr::Raise { .. }) || instruction.call_error().is_some() {
                meet(&mut flow.raises, Some(&state));
            }
            if matches!(instruction, MirInstr::Raise { .. }) {
                return flow;
            }
        }
        if let MirTerm::ComptimeFor { slot, .. } = &block.term {
            set(&mut state, *slot, false);
        }
        flow.normal = Some(state);
        flow
    }

    /// A `try` from `entry`, as the ownership analysis walks it: the handler
    /// starts from the body's raises, `orelse` from its normal fall-off, and
    /// `finally` runs on every channel. An escape leaves through the normal
    /// fall-off of the `finally` it crosses.
    fn try_regions(&mut self, entry: Ended, instruction: &MirInstr, report: bool) -> Flow {
        let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        else {
            return Flow {
                normal: Some(entry),
                ..Flow::default()
            };
        };
        let mut body_flow = self.region(&entry, body, false, report);
        let body_normal = body_flow.normal.take();
        let body_raises = body_flow.raises.take();
        let mut flow = Flow {
            exits: body_flow.exits,
            escapes: body_flow.escapes,
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
        out.normal = normal;
        meet(&mut out.raises, raises.as_ref());
        meet(&mut out.exits, exits.as_ref());
        out.escapes.extend(escapes);
        out
    }

    /// One instruction's effect, reporting a drop or consume of a slot
    /// already ended on every path when `report`.
    fn step(&mut self, index: usize, state: &mut Ended, instruction: &MirInstr, report: bool) {
        if let MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } = instruction {
            if report && state.get(*var as usize) == Some(&true) {
                let spelling = if matches!(instruction, MirInstr::DropVar { .. }) {
                    "drop.var"
                } else {
                    "var.consume"
                };
                let slot_name = self
                    .function
                    .var_names
                    .get(*var as usize)
                    .map_or("?", String::as_str);
                let finding = format!(
                    "MIR function '{}' block {index}: {spelling} of slot $v{var} ('{slot_name}') that every path has already ended",
                    self.name
                );
                if !self.findings.contains(&finding) {
                    self.findings.push(finding);
                }
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
            | MirInstr::DropPlace { place }
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

fn collect_refills(blocks: &[MirBlock], refills: &mut HashMap<u32, VarId>) {
    for block in blocks {
        for instruction in &block.instrs {
            match instruction {
                MirInstr::MakeRef { dest, place } => {
                    refills.insert(dest.0, place.root);
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    for region in std::iter::once(body)
                        .chain(handler.iter().map(|(_, blocks)| blocks))
                        .chain(orelse)
                        .chain(finalbody)
                    {
                        collect_refills(region, refills);
                    }
                }
                _ => {}
            }
        }
    }
}
