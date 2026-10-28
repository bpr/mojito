//! The per-instance report of an instance's failures: a `comptime assert`
//! that failed, and a parameter constant an unrolled iteration could not
//! compute. Upstream instantiates a function only where the program reaches
//! it, so each is reported here, per reached instance once its
//! compile-time branches are decided: an untaken arm's failure is never
//! reported.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Const;

/// Report the first `comptime assert` the instance `function` keeps — one
/// [`Specializer::decide_comptime_op`] left, so one that failed — as
/// upstream's `constraint failed: MESSAGE`.
pub(super) fn discharge_comptime_asserts(function: &mut MirFunction) -> Result<(), String> {
    if let Some(failed) = function
        .blocks
        .iter()
        .find_map(|block| reached_assert(&block.instrs))
    {
        return Err(failed);
    }
    for block in &mut function.blocks {
        erase_asserts(&mut block.instrs);
    }
    Ok(())
}

/// The failure of a parameter constant an unrolled iteration could not
/// compute ([`Specializer::answer_param_constants`]) whose read the
/// instance still holds once its branches are decided.
pub(super) fn reached_failure(
    blocks: &[MirBlock],
    mut failures: HashMap<u32, MonoError>,
) -> Result<(), MonoError> {
    if failures.is_empty() {
        return Ok(());
    }
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .try_for_each(|instruction| match instruction {
            MirInstr::Const {
                dest,
                k: Const::Param(_),
            } => failures.remove(&dest.0).map_or(Ok(()), Err),
            instruction => mojito_mir::mir::instruction_regions(instruction)
                .into_iter()
                .try_for_each(|region| reached_failure(region, failures.clone())),
        })
}

/// The failure an assertion in `instructions`, or in a region below them,
/// reports.
fn reached_assert(instructions: &[MirInstr]) -> Option<String> {
    instructions
        .iter()
        .find_map(|instruction| match instruction {
            MirInstr::ComptimeAssert { cond } => Some(match &**cond {
                GenericConstraint::WithMessage(_, message) => {
                    format!("constraint failed: {message}")
                }
                _ => "constraint failed: ".to_string(),
            }),
            instruction => mojito_mir::mir::instruction_regions(instruction)
                .into_iter()
                .flatten()
                .find_map(|block| reached_assert(&block.instrs)),
        })
}

/// Remove every assertion from `instructions` and the regions below them.
fn erase_asserts(instructions: &mut Vec<MirInstr>) {
    instructions.retain(|instruction| !matches!(instruction, MirInstr::ComptimeAssert { .. }));
    for instruction in instructions {
        for region in mojito_mir::mir::instruction_regions_mut(instruction) {
            for block in region {
                erase_asserts(&mut block.instrs);
            }
        }
    }
}
