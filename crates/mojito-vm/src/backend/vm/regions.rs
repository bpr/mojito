//! The region cursor: where a frame executes next inside the `hlcf` region
//! ops it is in, and each region op's `interpret` and each region
//! terminator's transfer, as upstream's interpreter moves a frame's `pc`
//! with `InterpreterState::transferControlFlowTo` rather than recursing.
//! See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{LoopLabel, MirBlockId, RegionFrame, region_exit};

/// Where a frame executes next: upstream's interpreter `pc`, as the path
/// from a block list (the function's, or a `try` part's) through each
/// region op it is inside.
pub(super) struct RegionCursor {
    /// One position per list, the root list first. Every level but the
    /// innermost is at the region op the next level is a region of.
    levels: Vec<RegionLevel>,
}

impl Default for RegionCursor {
    fn default() -> Self {
        Self {
            levels: vec![RegionLevel::entry(RegionRef::Root)],
        }
    }
}

impl RegionCursor {
    /// The block of `root`'s lists the cursor is in.
    pub(super) fn block<'p>(&self, root: &'p [MirBlock]) -> Result<&'p MirBlock, RuntimeError> {
        let depth = self.levels.len() - 1;
        self.list(root, depth)?
            .get(self.levels[depth].block)
            .ok_or_else(lost_position)
    }

    /// The index of the instruction the cursor is at in its
    /// [`Self::block`]: the terminator once past its last instruction.
    pub(super) fn instruction(&self) -> usize {
        self.levels.last().map_or(0, |level| level.instruction)
    }

    /// Step past the instruction the cursor is at.
    pub(super) fn advance(&mut self) {
        if let Some(level) = self.levels.last_mut() {
            level.instruction += 1;
        }
    }

    /// The `interpret` of the region op `op`: enter the region it runs
    /// first, binding that region's arguments. `false` when `op` is no
    /// `if`, `loop`, or `match`.
    pub(super) fn enter(&mut self, op: &MirInstr, regs: &mut [Value]) -> bool {
        let region = match op {
            MirInstr::If { cond, elifs, .. } => {
                if is_true(&regs[cond.0 as usize]) {
                    RegionRef::Then
                } else if elifs.is_empty() {
                    RegionRef::Else
                } else {
                    RegionRef::ElifCond(0)
                }
            }
            MirInstr::Loop {
                label,
                operands,
                args,
                ..
            } => {
                let values = read(regs, operands);
                store(regs, args, values);
                RegionRef::Body(*label)
            }
            MirInstr::Match { cases, .. } if cases.is_empty() => RegionRef::Else,
            MirInstr::Match { .. } => RegionRef::Case(0),
            _ => return false,
        };
        self.levels.push(RegionLevel::entry(region));
        true
    }

    /// The transfer of the terminator `term` the cursor is at in `root`:
    /// into a sibling region, or out to the op it names, binding the values
    /// it carries. A terminator that leaves the
    /// root list comes back for the driver to run.
    pub(super) fn transfer<'p>(
        &mut self,
        root: &[MirBlock],
        term: &'p MirTerm,
        regs: &mut [Value],
    ) -> Result<Transfer<'p>, RuntimeError> {
        match term {
            MirTerm::Yield(values) => {
                let values = read(regs, values);
                self.complete(root, values, regs)?;
            }
            MirTerm::ElifCond { cond, values } => {
                let holds = is_true(&regs[cond.0 as usize]);
                let values = read(regs, values);
                self.elif(root, holds, values, regs)?;
            }
            MirTerm::Break { values, .. }
            | MirTerm::Continue { values, .. }
            | MirTerm::MatchNext(values)
            | MirTerm::MatchComplete(values) => {
                let (frame, resume) = region_exit(term).ok_or_else(lost_position)?;
                let leave = Leave {
                    frame,
                    resume,
                    values: read(regs, values),
                };
                if let Some(leave) = self.resolve_leave(root, leave, regs)? {
                    return Ok(Transfer::Leave(leave));
                }
            }
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } | MirTerm::TryYield => {
                return Ok(Transfer::Exit(term));
            }
            MirTerm::Unreachable => {
                return Err(RuntimeError::Unsupported(
                    "vm: control reached `hlcf.unreachable`".to_string(),
                ));
            }
            MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak(_)
            | MirTerm::ComptimeContinue(_) => {
                return Err(parametric_instruction(&format!(
                    "the region terminator `{}`",
                    mojito_mir::mir::text::terminator_mnemonic(term)
                )));
            }
        }
        Ok(Transfer::Moved)
    }

    /// Resolve `leave` to the innermost op the cursor is inside that it
    /// names: resume that op (the next iteration of a loop, the next case
    /// of a `match`) or leave it, binding the values it carries. `leave`
    /// comes back when no such op encloses the cursor, so it leaves the
    /// root list.
    pub(super) fn resolve_leave(
        &mut self,
        root: &[MirBlock],
        leave: Leave,
        regs: &mut [Value],
    ) -> Result<Option<Leave>, RuntimeError> {
        let Some(depth) = (1..self.levels.len()).rev().find(|&depth| {
            self.levels[depth]
                .region
                .frame()
                .is_some_and(|frame| leave.frame.names(frame))
        }) else {
            return Ok(Some(leave));
        };
        let op = self.op(root, depth)?;
        self.levels.truncate(depth + 1);
        if !leave.resume {
            self.levels.pop();
            store(regs, results(op)?, leave.values);
            self.advance();
            return Ok(None);
        }
        let (region, args) = match (op, self.levels[depth].region) {
            (MirInstr::Loop { args, .. }, region @ RegionRef::Body(_)) => (region, args),
            (
                MirInstr::Match {
                    cases, else_args, ..
                },
                RegionRef::Case(index),
            ) => cases
                .get(index + 1)
                .map_or((RegionRef::Else, else_args), |next| {
                    (RegionRef::Case(index + 1), &next.args)
                }),
            _ => return Err(lost_position()),
        };
        self.levels[depth] = RegionLevel::entry(region);
        store(regs, args, leave.values);
        Ok(None)
    }

    /// Leave the innermost region to its op, `values` defining the op's
    /// results: `hlcf.yield`.
    fn complete(
        &mut self,
        root: &[MirBlock],
        values: Vec<Value>,
        regs: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let depth = self.levels.len() - 1;
        if depth == 0 {
            return Err(lost_position());
        }
        let op = self.op(root, depth)?;
        self.levels.pop();
        store(regs, results(op)?, values);
        self.advance();
        Ok(())
    }

    /// End the innermost elif condition region: enter its arm when `holds`,
    /// else the next elif's condition, or the `if`'s else region after the
    /// last, `values` defining that region's arguments.
    fn elif(
        &mut self,
        root: &[MirBlock],
        holds: bool,
        values: Vec<Value>,
        regs: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let depth = self.levels.len() - 1;
        let RegionRef::ElifCond(index) = self.levels[depth].region else {
            return Err(lost_position());
        };
        let MirInstr::If {
            elifs, else_args, ..
        } = self.op(root, depth)?
        else {
            return Err(lost_position());
        };
        let (region, args) = if holds {
            let elif = elifs.get(index).ok_or_else(lost_position)?;
            (RegionRef::ElifThen(index), &elif.then_args)
        } else {
            elifs
                .get(index + 1)
                .map_or((RegionRef::Else, else_args), |next| {
                    (RegionRef::ElifCond(index + 1), &next.cond_args)
                })
        };
        self.levels[depth] = RegionLevel::entry(region);
        store(regs, args, values);
        Ok(())
    }

    /// The region op level `depth` (at least 1) is a region of.
    fn op<'p>(&self, root: &'p [MirBlock], depth: usize) -> Result<&'p MirInstr, RuntimeError> {
        op_at(self.list(root, depth - 1)?, &self.levels[depth - 1])
    }

    /// The block list level `depth` is in.
    fn list<'p>(&self, root: &'p [MirBlock], depth: usize) -> Result<&'p [MirBlock], RuntimeError> {
        let mut blocks = root;
        for pair in self.levels[..=depth].windows(2) {
            blocks = region(op_at(blocks, &pair[0])?, pair[1].region).ok_or_else(lost_position)?;
        }
        Ok(blocks)
    }
}

/// Where a terminator sent control.
pub(super) enum Transfer<'p> {
    /// The cursor moved to the terminator's target.
    Moved,
    /// A `return` or `lit.try.yield`, which leaves the root list: the
    /// driver's to run.
    Exit(&'p MirTerm),
    /// A region exit naming an op outside the root list.
    Leave(Leave),
}

/// A `break`, `continue`, `match.next`, or `match.complete` naming an op
/// outside the list it left — a `try` part's `break` out of a loop around
/// the `try` — with the values it carries. It propagates out of each `try`
/// (running its `finally`) until a cursor inside that op resolves it.
pub(super) struct Leave {
    frame: RegionFrame,
    resume: bool,
    values: Vec<Value>,
}

/// One list's position: a block and an instruction in it, and which region
/// of the enclosing level's op the list is.
#[derive(Clone, Copy)]
struct RegionLevel {
    region: RegionRef,
    block: MirBlockId,
    instruction: usize,
}

impl RegionLevel {
    const fn entry(region: RegionRef) -> Self {
        Self {
            region,
            block: 0,
            instruction: 0,
        }
    }
}

/// A region of a region op: an `if`'s `then`, an elif's condition or arm,
/// an `if`'s or `match`'s else region, a loop's body, or a `match` case.
#[derive(Clone, Copy)]
enum RegionRef {
    Root,
    Then,
    ElifCond(usize),
    ElifThen(usize),
    Else,
    Body(Option<LoopLabel>),
    Case(usize),
}

impl RegionRef {
    /// The frame a region exit names this region's op by.
    const fn frame(self) -> Option<RegionFrame> {
        match self {
            Self::Body(label) => Some(RegionFrame::Loop(label)),
            Self::Case(_) => Some(RegionFrame::MatchCase),
            Self::Root | Self::Then | Self::ElifCond(_) | Self::ElifThen(_) | Self::Else => None,
        }
    }
}

/// The error of a cursor naming no position: the verifier admits only
/// region terminators whose region and op exist.
fn lost_position() -> RuntimeError {
    RuntimeError::Unsupported("vm: a region transfer named no enclosing region".to_string())
}

/// The values of `sources`, all read before any is bound, since a
/// `continue` may permute its loop's own arguments.
fn read(regs: &[Value], sources: &[Reg]) -> Vec<Value> {
    sources
        .iter()
        .map(|source| regs[source.0 as usize].clone())
        .collect()
}

/// Bind each of `targets` to its value of `values`.
fn store(regs: &mut [Value], targets: &[Reg], values: Vec<Value>) {
    for (target, value) in targets.iter().zip(values) {
        regs[target.0 as usize] = value;
    }
}

/// The instruction `level` is at in `blocks`.
fn op_at<'p>(blocks: &'p [MirBlock], level: &RegionLevel) -> Result<&'p MirInstr, RuntimeError> {
    blocks
        .get(level.block)
        .and_then(|block| block.instrs.get(level.instruction))
        .ok_or_else(lost_position)
}

/// The region `region` of the op `op`.
fn region(op: &MirInstr, region: RegionRef) -> Option<&[MirBlock]> {
    match (op, region) {
        (MirInstr::If { then, .. }, RegionRef::Then) => Some(then),
        (MirInstr::If { elifs, .. }, RegionRef::ElifCond(index)) => {
            elifs.get(index).map(|elif| elif.cond.as_slice())
        }
        (MirInstr::If { elifs, .. }, RegionRef::ElifThen(index)) => {
            elifs.get(index).map(|elif| elif.then.as_slice())
        }
        (MirInstr::If { orelse, .. } | MirInstr::Match { orelse, .. }, RegionRef::Else) => {
            Some(orelse)
        }
        (MirInstr::Loop { body, .. }, RegionRef::Body(_)) => Some(body),
        (MirInstr::Match { cases, .. }, RegionRef::Case(index)) => {
            cases.get(index).map(|case| case.body.as_slice())
        }
        _ => None,
    }
}

/// The results of the region op `op`.
fn results(op: &MirInstr) -> Result<&[Reg], RuntimeError> {
    match op {
        MirInstr::If { results, .. }
        | MirInstr::Loop { results, .. }
        | MirInstr::Match { results, .. } => Ok(results),
        _ => Err(lost_position()),
    }
}
