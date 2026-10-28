//! Deciding an instance's `comptime if`s on their regions, as upstream's
//! `processComptimeIfOp` does: the first condition that holds picks its arm,
//! else the else region, and the arm's instructions replace the op. An arm
//! that completes yields the op's results; one that leaves (a `return`, a
//! `break`) ends the block, and what followed the op goes with the untaken
//! arms, never elaborated.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::{MirTerm, SpanTable, defined_registers, resolve_replacements};

impl Specializer<'_> {
    /// Decide the `comptime if` or `comptime assert` at `index` of `block`
    /// under `bindings`, as upstream's work list reaches the op, recording
    /// in `decided` what the decision replaces: an `if` is replaced by its
    /// taken arm, whose instructions the walk reaches next, so an `elif`
    /// past a taken arm, or an op after an arm that leaves, never runs its
    /// condition's applications. The taken arm keeps the destroys drop
    /// elaboration placed in it; no last use is recomputed. An assertion
    /// that holds goes, and one that fails stays, its condition folded to
    /// `False`, for [`failure::discharge_comptime_asserts`] to report. How
    /// far the walk moves past the op, or `None` when the instruction there
    /// is another.
    pub(super) fn decide_comptime_op(
        &mut self,
        template: &str,
        block: &mut MirBlock,
        index: usize,
        bindings: &Bindings,
        decided: &mut Decided,
    ) -> Result<Option<usize>, MonoError> {
        match &mut block.instrs[index] {
            MirInstr::ComptimeIf { conds, .. } => {
                let mut taken = None;
                for (arm, cond) in conds.iter().enumerate() {
                    if self.decide_comptime_condition(template, cond, bindings)? {
                        taken = Some(arm);
                        break;
                    }
                }
                splice_comptime_arm(block, index, taken, decided)
                    .map_err(|construct| self.error(Some(template), construct))?;
                Ok(Some(0))
            }
            MirInstr::ComptimeAssert { cond } => {
                if self.decide_comptime_condition(template, cond, bindings)? {
                    block.instrs.remove(index);
                    Ok(Some(0))
                } else {
                    **cond = folded_false(cond);
                    Ok(Some(1))
                }
            }
            _ => Ok(None),
        }
    }
}

/// What deciding `comptime if`s and folding unrolled loops leaves for the
/// function's tables: each taken arm's or folded loop's result and the
/// value its `comptime.yield` or break carries, and every register the
/// dropped code defined.
#[derive(Default)]
pub(super) struct Decided {
    pub(super) replaced: HashMap<u32, Reg>,
    dead: Vec<Reg>,
}

impl Decided {
    /// Read each replaced result in `blocks` as its value, and drop the
    /// tables of every register no instruction defines any more.
    pub(super) fn apply(
        self,
        blocks: &mut [MirBlock],
        reg_types: &mut HashMap<u32, Ty>,
        spans: &mut SpanTable,
    ) {
        mojito_mir::mir::replace_register_reads(blocks, &resolve_replacements(&self.replaced));
        for reg in self
            .replaced
            .into_keys()
            .chain(self.dead.into_iter().map(|reg| reg.0))
        {
            reg_types.remove(&reg);
            spans.0.remove(&reg);
        }
    }
}

/// Replace the `comptime if` at `index` of `block` with its arm `taken`, or
/// its else region when `None`, as `processComptimeIfOp` splices the region
/// it elaborated: an arm ending in `comptime.yield` stands where the op
/// stood, its values replacing the op's results, while an arm ending in any
/// other terminator ends the block, dropping what followed the op. The
/// registers of every dropped region and instruction are recorded dead.
pub(super) fn splice_comptime_arm(
    block: &mut MirBlock,
    index: usize,
    taken: Option<usize>,
    decided: &mut Decided,
) -> Result<(), &'static str> {
    let (results, mut regions) = match block.instrs.remove(index) {
        MirInstr::ComptimeIf {
            results,
            mut arms,
            orelse,
            ..
        } => {
            arms.push(orelse);
            (results, arms)
        }
        other => {
            block.instrs.insert(index, other);
            return Err("a decided compile-time branch is no `comptime if`");
        }
    };
    let chosen = taken.unwrap_or(regions.len() - 1);
    let arm = regions.swap_remove(chosen);
    for region in &regions {
        defined_registers(region, &mut decided.dead);
    }
    let [arm] =
        <[MirBlock; 1]>::try_from(arm).map_err(|_| "a `comptime if` arm is not one block")?;
    match arm.term {
        MirTerm::ComptimeYield(values) => {
            decided
                .replaced
                .extend(results.iter().map(|reg| reg.0).zip(values));
            block.instrs.splice(index..index, arm.instrs);
        }
        term => {
            let tail = MirBlock {
                instrs: block.instrs.split_off(index),
                term: std::mem::replace(&mut block.term, term),
            };
            defined_registers(std::slice::from_ref(&tail), &mut decided.dead);
            decided.dead.extend(results);
            block.instrs.extend(arm.instrs);
        }
    }
    Ok(())
}

/// A failed assertion's decided condition: `False`, keeping its message.
fn folded_false(cond: &GenericConstraint) -> GenericConstraint {
    match cond {
        GenericConstraint::WithMessage(_, message) => GenericConstraint::WithMessage(
            Box::new(GenericConstraint::Bool(false)),
            message.clone(),
        ),
        _ => GenericConstraint::Bool(false),
    }
}
