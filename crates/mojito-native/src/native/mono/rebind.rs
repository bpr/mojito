//! The per-instance judgement of a generator's `rebind`s.
//!
//! A template reads a `rebind[Dest](x)` operand at `Dest` through a value
//! rebind or a place's [`Proj::Rebind`] step. Once an instance's bindings
//! close both types and its compile-time branches are decided, the
//! elaborator asserts each surviving rebind's two types equal and erases it,
//! as upstream's `processRebindOp` does: an untaken arm's rebind is never
//! judged, and a mismatch fails the instance.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Proj;
use mojito_mir::mir::verify::{instruction_places_mut, place_base_ty, types_agree};
use mojito_mir::mir::{instruction_regions_mut, instruction_regs_mut, terminator_regs_mut};

impl Specializer<'_> {
    /// Judge every rebind of the instance `function` of
    /// `template`: a value rebind's `dest` register becomes its operand's,
    /// and a rebound place loses its step, whose type now names the
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

/// A judged rebind's operand and target types where they disagree.
type Mismatch = Box<(Ty, Ty)>;

/// Judge the rebinds of one block list and the regions below it, recording
/// each value rebind's `dest → value` and removing the instruction or place
/// step; the first mismatch is returned as its `(input, result)` types.
fn judge_blocks(
    blocks: &mut [MirBlock],
    reg_types: &HashMap<u32, Ty>,
    aliases: &mut HashMap<u32, Reg>,
) -> Result<(), Mismatch> {
    for block in blocks {
        let mut judged = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            for region in instruction_regions_mut(&mut instruction) {
                judge_blocks(region, reg_types, aliases)?;
            }
            for place in instruction_places_mut(&mut instruction) {
                discharge_place(place)?;
            }
            if let MirInstr::Rebind { dest, value } = instruction {
                if let (Some(input), Some(result)) =
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

/// Erase a place's rebind steps, judging each against the type it applies
/// to: the storage's own type must agree with the one the rebind names.
fn discharge_place(place: &mut MirPlace) -> Result<(), Mismatch> {
    if !place.proj.iter().any(|step| matches!(step, Proj::Rebind)) {
        return Ok(());
    }
    let mut applied = place_base_ty(place);
    for (step, ty) in place.proj.iter().zip(&place.projection_tys) {
        if let (Proj::Rebind, Some(input)) = (step, applied)
            && !types_agree(input, ty)
        {
            return Err(Box::new((input.clone(), ty.clone())));
        }
        applied = Some(ty);
    }
    *place = place.storage().into_owned();
    Ok(())
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
            for region in instruction_regions_mut(instruction) {
                rename_blocks(region, aliases);
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
