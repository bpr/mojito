//! The per-instance judgement of a generator's `rebind`s.
//!
//! A template reads a `rebind[Dest](x)` operand at `Dest` through a value
//! rebind or a place whose terminal type is `Dest`. Once an instance's
//! bindings close both types and its compile-time branches are decided, the
//! elaborator asserts each surviving rebind's two types equal and erases it,
//! as upstream's `processRebindOp` does: an untaken arm's rebind is never
//! judged, and a mismatch fails the instance.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::verify::{instruction_places, types_agree};
use mojito_mir::mir::{instruction_regs_mut, terminator_regs_mut, terminator_targets};

impl Specializer<'_> {
    /// Judge every rebind reachable in the instance `function` of
    /// `template`: a value rebind's `dest` register becomes its operand's,
    /// and a rebound place keeps its terminal type, which now names the
    /// storage's own.
    pub(super) fn discharge_rebinds(
        &self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        let mut aliases = HashMap::new();
        judge_blocks(&mut function.blocks, &function.reg_types, &mut aliases)
            .map_err(|mismatch| self.rebind_mismatch(template, &mismatch.0, &mismatch.1))?;
        if !aliases.is_empty() {
            rename_blocks(&mut function.blocks, &aliases);
        }
        Ok(())
    }

    fn rebind_mismatch(&self, template: &str, input: &Ty, result: &Ty) -> MonoError {
        MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(
                Some(template),
                format!("rebind input type '{input}' does not match result type '{result}'"),
            )
        }
    }
}

/// The blocks of `blocks` its entry reaches: a decided `comptime if` leaves
/// its untaken arm behind in a `try` region, which no pruning renumbers.
pub(super) fn reachable(blocks: &[MirBlock]) -> Vec<bool> {
    let mut reached = vec![false; blocks.len()];
    let mut pending = vec![0usize];
    while let Some(block) = pending.pop() {
        if block >= blocks.len() || std::mem::replace(&mut reached[block], true) {
            continue;
        }
        pending.extend(terminator_targets(&blocks[block].term));
    }
    reached
}

/// A judged rebind's operand and target types where they disagree.
type Mismatch = Box<(Ty, Ty)>;

/// Judge the rebinds of the reached blocks of one block list and the regions
/// below it, recording each value rebind's `dest → value` and removing the
/// instruction, an unreached one's unjudged; the first mismatch is returned
/// as its `(input, result)` types.
fn judge_blocks(
    blocks: &mut [MirBlock],
    reg_types: &HashMap<u32, Ty>,
    aliases: &mut HashMap<u32, Reg>,
) -> Result<(), Mismatch> {
    let reached = reachable(blocks);
    for (block, reached) in blocks.iter_mut().zip(reached) {
        let mut judged = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = &mut instruction
            {
                let regions = std::iter::once(body)
                    .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                    .chain(orelse.iter_mut())
                    .chain(finalbody.iter_mut());
                for region in regions {
                    judge_blocks(region, reg_types, aliases)?;
                }
            }
            if reached {
                for place in instruction_places(&instruction) {
                    judge_place(place)?;
                }
            }
            if let MirInstr::Rebind { dest, value } = instruction {
                if reached
                    && let (Some(input), Some(result)) =
                        (reg_types.get(&value.0), reg_types.get(&dest.0))
                    && !types_agree(input, result)
                {
                    return Err(Box::new((input.clone(), result.clone())));
                }
                aliases.insert(dest.0, value);
                continue;
            }
            judged.push(instruction);
        }
        block.instrs = judged;
    }
    Ok(())
}

/// A rebound place's storage type, its projected one, must agree with the
/// terminal type the rebind names.
fn judge_place(place: &MirPlace) -> Result<(), Mismatch> {
    let projected = match (place.projection_tys.last(), &place.root_ty) {
        (Some(projected), _) => projected,
        // A materialized reference-result place keeps the referent's type
        // behind its hidden `ref` root, as the verifier reads it.
        (None, Some(Ty::Ref(reference)))
            if place.through.is_some() && place.ty.as_ref() != place.root_ty.as_ref() =>
        {
            &*reference.referent
        }
        (None, Some(root)) => root,
        (None, None) => return Ok(()),
    };
    match &place.ty {
        Some(terminal) if !types_agree(projected, terminal) => {
            Err(Box::new((projected.clone(), terminal.clone())))
        }
        _ => Ok(()),
    }
}

/// Read every use of a judged value rebind's `dest` as its operand.
fn rename_blocks(blocks: &mut [MirBlock], aliases: &HashMap<u32, Reg>) {
    let resolve = |reg: &mut Reg| {
        while let Some(value) = aliases.get(&reg.0) {
            *reg = *value;
        }
    };
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                let regions = std::iter::once(body)
                    .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                    .chain(orelse.iter_mut())
                    .chain(finalbody.iter_mut());
                for region in regions {
                    rename_blocks(region, aliases);
                }
            }
            instruction_regs_mut(instruction)
                .into_iter()
                .for_each(resolve);
        }
        terminator_regs_mut(&mut block.term)
            .into_iter()
            .for_each(resolve);
    }
}
