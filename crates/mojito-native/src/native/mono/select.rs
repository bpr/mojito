//! Deciding an instance's `comptime if`s on their regions, as upstream's
//! `processComptimeIfOp` does: the first condition that holds picks its arm,
//! else the else region, and the arm's instructions replace the op. An arm
//! that completes yields the op's results; one that leaves (a `return`, a
//! `break`) ends the block, and what followed the op goes with the untaken
//! arms, never elaborated, save its assertions, which the instance still
//! judges.

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
            MirInstr::ComptimeIf { values, .. } => {
                let mut taken = None;
                for (arm, value) in values.clone().iter().enumerate() {
                    if self.decide_comptime_test(template, value, bindings)? {
                        taken = Some(arm);
                        break;
                    }
                }
                let tail = splice_comptime_arm(block, index, taken, decided)
                    .map_err(|construct| self.error(Some(template), construct))?;
                if let Some(tail) = tail {
                    let mut asserts = Vec::new();
                    tail_asserts(&tail.instrs, &mut asserts);
                    block.instrs.extend(asserts);
                }
                Ok(Some(0))
            }
            MirInstr::ComptimeAssert { value, message } => {
                if self.decide_comptime_test(template, value, bindings)? {
                    block.instrs.remove(index);
                    return Ok(Some(0));
                }
                *value = Const::Bool(false);
                if let Some(Const::Param(expression)) = message {
                    *message = Some(Const::Str(
                        self.assertion_message(template, expression, bindings)?,
                    ));
                }
                Ok(Some(1))
            }
            _ => Ok(None),
        }
    }

    /// The text of a failed assertion's message expression at the
    /// instance, as upstream evaluates the `kgen.param.assert` message
    /// once its condition fails: the compile-time `String` or
    /// `StaticString` the expression's applications compute.
    fn assertion_message(
        &mut self,
        template: &str,
        expression: &ParamExpr,
        bindings: &Bindings,
    ) -> Result<String, MonoError> {
        let applied = self.applied(template, expression, bindings)?;
        let value = symbolic::eval_reached_ct(&applied, bindings)?;
        message_text(&value).ok_or_else(|| {
            self.error(
                Some(template),
                format!("a `comptime assert` message closes to `{value}`, not a string"),
            )
        })
    }
}

/// What deciding `comptime if`s and folding unrolled loops leaves for the
/// function's tables: each taken arm's or folded loop's result and the
/// value its `comptime.yield` or break carries, every register the
/// dropped code defined, and the binder of every `comptime for` the
/// dropped code held, whose slots no instance holds.
#[derive(Default)]
pub(super) struct Decided {
    pub(super) replaced: HashMap<u32, Reg>,
    pub(super) dead_binders: Vec<mojito_types::param_expr::ParamRef>,
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
/// other terminator ends the block, dropping what followed the op, which is
/// returned. The registers of every dropped region and instruction are
/// recorded dead.
pub(super) fn splice_comptime_arm(
    block: &mut MirBlock,
    index: usize,
    taken: Option<usize>,
    decided: &mut Decided,
) -> Result<Option<MirBlock>, &'static str> {
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
        dead_loop_binders(region, &mut decided.dead_binders);
    }
    let [arm] =
        <[MirBlock; 1]>::try_from(arm).map_err(|_| "a `comptime if` arm is not one block")?;
    match arm.term {
        MirTerm::ComptimeYield(values) => {
            decided
                .replaced
                .extend(results.iter().map(|reg| reg.0).zip(values));
            block.instrs.splice(index..index, arm.instrs);
            Ok(None)
        }
        term => {
            let tail = MirBlock {
                instrs: block.instrs.split_off(index),
                term: std::mem::replace(&mut block.term, term),
            };
            defined_registers(std::slice::from_ref(&tail), &mut decided.dead);
            dead_loop_binders(std::slice::from_ref(&tail), &mut decided.dead_binders);
            decided.dead.extend(results);
            block.instrs.extend(arm.instrs);
            Ok(Some(tail))
        }
    }
}

/// Push the binder of every `comptime for` in the dropped `blocks`, at any
/// depth: upstream never elaborates the dropped code, so a slot typed over
/// one of those loop indices is no slot of the instance.
fn dead_loop_binders(blocks: &[MirBlock], binders: &mut Vec<mojito_types::param_expr::ParamRef>) {
    for instruction in mojito_mir::mir::nested_instructions(blocks) {
        if let MirInstr::ComptimeFor { binder, .. } = instruction {
            binders.push(binder.clone());
        }
    }
}

/// The assertions of a tail an arm that leaves dropped, which the instance
/// still judges as the pin does: those of the tail and of its runtime
/// regions, never those under a `comptime if` or `comptime for` the tail
/// holds, which are never elaborated, nor those of a callee
/// `inline-parametric` spliced into it, whose dead call the pin never
/// instantiates.
fn tail_asserts(instructions: &[MirInstr], asserts: &mut Vec<MirInstr>) {
    for instruction in instructions {
        match instruction {
            MirInstr::ComptimeAssert { .. } => asserts.push(instruction.clone()),
            MirInstr::ComptimeIf { .. }
            | MirInstr::ComptimeFor { .. }
            | MirInstr::Loop {
                label: Some(mojito_mir::mir::LoopLabel::INLINED_CF_SCOPE),
                ..
            } => {}
            instruction => {
                for block in mojito_mir::mir::instruction_regions(instruction)
                    .into_iter()
                    .flatten()
                {
                    tail_asserts(&block.instrs, asserts);
                }
            }
        }
    }
}

/// The text a compile-time string value holds: a `String`'s, a
/// `!kgen.string`'s, or, as upstream's `data_to_str` reads the message's
/// `StringSpan`, the bytes a view addresses.
fn message_text(value: &CtValue) -> Option<String> {
    match value {
        CtValue::Str(text) | CtValue::KgenString(text) => Some(text.clone()),
        view => {
            match mojito_types::param_expr::fold::fold_data_to_str(view, &CtValue::List(Vec::new()))
            {
                Ok(CtValue::KgenString(text)) => Some(text),
                _ => None,
            }
        }
    }
}
