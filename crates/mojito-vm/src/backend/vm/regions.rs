//! The region cursor: where a frame executes next inside the `hlcf` and
//! `lit.try` region ops it is in, and each region op's `interpret` and each
//! region terminator's transfer, as upstream's interpreter moves a frame's
//! `pc` with `InterpreterState::transferControlFlowTo` rather than
//! recursing.
//! See `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{LoopLabel, MirBlockId, RegionFrame, TryLabel, region_exit};

/// Where a frame executes next: upstream's interpreter `pc`, as the path
/// from the function's block list through each region op it is inside.
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
    /// `if`, `loop`, `match`, or `try`.
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
            MirInstr::Try { label, .. } => RegionRef::TryBody(*label),
            _ => return false,
        };
        self.levels.push(RegionLevel::entry(region));
        true
    }

    /// The transfer of the terminator `term` the cursor is at in `root`:
    /// into a sibling region, or out to the op it names, binding the values
    /// it carries. A `return` or `raise` comes back for the driver to run.
    pub(super) fn transfer(
        &mut self,
        root: &[MirBlock],
        term: &MirTerm,
        regs: &mut [Value],
    ) -> Result<Transfer, RuntimeError> {
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
                let values = read(regs, values);
                self.resolve_leave(root, frame, resume, values, regs)?;
            }
            MirTerm::TryYield(values) => {
                let values = read(regs, values);
                self.try_yield(root, values, regs)?;
            }
            MirTerm::Return(value) => return Ok(Transfer::Return(*value)),
            MirTerm::TryRaise { label, values } => {
                return Ok(Transfer::TryRaise {
                    label: *label,
                    values: read(regs, values),
                });
            }
            MirTerm::ErrorReturn { src } => return Ok(Transfer::ErrorReturn(*src)),
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

    /// `lit.try.raise`: unwind to the body of the `try` labelled `label`
    /// around the cursor and enter its `except` region, `values` defining
    /// its arguments. The raise stored the error into the `try`'s slot.
    pub(super) fn catch(
        &mut self,
        root: &[MirBlock],
        label: TryLabel,
        values: Vec<Value>,
        regs: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let depth = (1..self.levels.len())
            .rev()
            .find(|&depth| matches!(self.levels[depth].region, RegionRef::TryBody(found) if found == label))
            .ok_or_else(|| {
                RuntimeError::Unsupported(format!(
                    "vm: a raise named try{}, which does not enclose it",
                    label.0
                ))
            })?;
        let MirInstr::Try {
            handler,
            handler_args,
            ..
        } = self.op(root, depth)?
        else {
            return Err(lost_position());
        };
        if handler.is_none() {
            return Err(RuntimeError::Unsupported(format!(
                "vm: a raise named try{}, which has no handler",
                label.0
            )));
        }
        self.levels.truncate(depth + 1);
        self.levels[depth] = RegionLevel::entry(RegionRef::TryHandler);
        store(regs, handler_args, values);
        Ok(())
    }

    /// Resolve a region exit leaving to `frame` to the innermost op the
    /// cursor is inside that it names: resume that op (the next iteration
    /// of a loop, the next case of a `match`) or leave it, binding the
    /// values it carries.
    fn resolve_leave(
        &mut self,
        root: &[MirBlock],
        frame: RegionFrame,
        resume: bool,
        values: Vec<Value>,
        regs: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let depth = (1..self.levels.len())
            .rev()
            .find(|&depth| {
                self.levels[depth]
                    .region
                    .frame()
                    .is_some_and(|found| frame.names(found))
            })
            .ok_or_else(|| {
                RuntimeError::Unsupported("vm: a region exit left its function".to_string())
            })?;
        let op = self.op(root, depth)?;
        self.levels.truncate(depth + 1);
        if !resume {
            self.levels.pop();
            store(regs, results(op)?, values);
            self.advance();
            return Ok(());
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
        store(regs, args, values);
        Ok(())
    }

    /// `lit.try.yield`: end the innermost `try` part, entering the `try`'s
    /// `else` region from its body when it has one, else leaving the `try`,
    /// `values` defining the `else`'s arguments or the `try`'s results.
    fn try_yield(
        &mut self,
        root: &[MirBlock],
        values: Vec<Value>,
        regs: &mut [Value],
    ) -> Result<(), RuntimeError> {
        let depth = self.levels.len() - 1;
        let MirInstr::Try {
            results,
            else_args,
            orelse,
            ..
        } = self.op(root, depth)?
        else {
            return Err(lost_position());
        };
        match self.levels[depth].region {
            RegionRef::TryBody(_) if orelse.is_some() => {
                self.levels[depth] = RegionLevel::entry(RegionRef::TryElse);
                store(regs, else_args, values);
            }
            RegionRef::TryBody(_) | RegionRef::TryHandler | RegionRef::TryElse => {
                self.levels.pop();
                store(regs, results, values);
                self.advance();
            }
            _ => return Err(lost_position()),
        }
        Ok(())
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
pub(super) enum Transfer {
    /// The cursor moved to the terminator's target.
    Moved,
    /// A `return` of the register it names: the driver's to run.
    Return(Option<Reg>),
    /// A `try.raise` to the `try` labelled `label`, with the values it
    /// carries into that `try`'s handler: the driver's to run.
    TryRaise { label: TryLabel, values: Vec<Value> },
    /// A `lit.error_return` of the register it names: the driver's to run.
    ErrorReturn(Reg),
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
/// an `if`'s or `match`'s else region, a loop's body, a `match` case, or a
/// `try`'s body, `except`, or `else`.
#[derive(Clone, Copy)]
enum RegionRef {
    Root,
    Then,
    ElifCond(usize),
    ElifThen(usize),
    Else,
    Body(Option<LoopLabel>),
    Case(usize),
    TryBody(TryLabel),
    TryHandler,
    TryElse,
}

impl RegionRef {
    /// The frame a region exit names this region's op by.
    const fn frame(self) -> Option<RegionFrame> {
        match self {
            Self::Body(label) => Some(RegionFrame::Loop(label)),
            Self::Case(_) => Some(RegionFrame::MatchCase),
            Self::Root
            | Self::Then
            | Self::ElifCond(_)
            | Self::ElifThen(_)
            | Self::Else
            | Self::TryBody(_)
            | Self::TryHandler
            | Self::TryElse => None,
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
        (MirInstr::Try { body, .. }, RegionRef::TryBody(_)) => Some(body),
        (MirInstr::Try { handler, .. }, RegionRef::TryHandler) => handler.as_deref(),
        (MirInstr::Try { orelse, .. }, RegionRef::TryElse) => orelse.as_deref(),
        _ => None,
    }
}

/// The results of the region op `op`.
fn results(op: &MirInstr) -> Result<&[Reg], RuntimeError> {
    match op {
        MirInstr::If { results, .. }
        | MirInstr::Loop { results, .. }
        | MirInstr::Match { results, .. }
        | MirInstr::Try { results, .. } => Ok(results),
        _ => Err(lost_position()),
    }
}
