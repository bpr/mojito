//! Unrolling a `comptime for`, as upstream's `processComptimeForOp` does:
//! the op becomes a loop labelled `param_for_outer` holding one unlabelled
//! loop per iterator the protocol steps to, the exhausted one included, each
//! holding a clone of the body that keeps the arm of the body's `has_next`
//! test its iterator takes; `simplifyParamForLoop` then folds the loops the
//! clones leave trivial.

use super::select::{Decided, splice_comptime_arm};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::ComptimeSequence;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    LoopLabel, MirTerm, SpanTable, defined_registers, for_each_register, instruction_regions_mut,
    loop_breaks_mut, renumber_block_slots, replace_register_reads, resolve_replacements,
};

impl Specializer<'_> {
    /// Unroll every `comptime for` of the instance under its bindings, as
    /// the elaborator does for upstream's `ComptimeForOp`: the iterators are
    /// stepped, and the body is cloned N+1 times, once per iterator
    /// `has_next` accepts and once over the exhausted one, each clone with
    /// fresh registers for what it defines and the binder bound to its
    /// iterator. Each clone decides the body's leading `has_next` test
    /// first and keeps the taken arm alone; the suite's clone then has its
    /// reads folded, its types substituted, the `comptime if`s over the
    /// binder decided, and the loops nested in it unrolled in turn, while
    /// the exhausted clone keeps only the stop arm, which leaves the loop.
    /// Each clone sits in a loop of its own, which its `comptime.for.continue`
    /// leaves, inside the loop its `comptime.for.break` leaves, and the
    /// loops a clone leaves trivial fold away. A slot whose type names the
    /// binder is a fresh slot in each clone that addresses it, as each clone
    /// of the region allocates its own `var`, and the template's slot leaves
    /// the function. The drops the analysis placed in the body come with
    /// each clone; no last use is recomputed.
    pub(super) fn unroll_comptime_loops(
        &mut self,
        template: &str,
        function: &mut MirFunction,
        scope: &[ParamDecl],
        bindings: &Bindings,
    ) -> Result<(), MonoError> {
        if !mojito_mir::mir::nested_instructions(&function.blocks)
            .iter()
            .any(|instruction| matches!(instruction, MirInstr::ComptimeFor { .. }))
        {
            return Ok(());
        }
        let dangling = dangling_registers(function);
        let MirFunction {
            blocks,
            n_regs,
            n_vars,
            var_names,
            var_tys,
            spans,
            reg_types,
            ..
        } = function;
        let mut retired = BTreeSet::new();
        let mut tables = FrameTables {
            n_regs,
            reg_types,
            spans,
            n_vars,
            var_names,
            var_tys,
            retired: &mut retired,
            dangling,
        };
        let frame = LoopFrame { template, scope };
        self.unroll_in(blocks, &frame, bindings, &mut tables)?;
        if retired.is_empty() {
            return Ok(());
        }
        // The index is in scope in the body alone, so once the clones stand
        // nothing addresses a slot typed over it.
        if let Some(&slot) = addressed_slots(&function.blocks)
            .intersection(&retired)
            .next()
        {
            let name = function
                .var_names
                .get(slot as usize)
                .map_or("?", String::as_str);
            return Err(self.error(
                Some(template),
                format!("slot `{name}` typed over a comptime for index is used outside its loop"),
            ));
        }
        retire_slots(function, &retired);
        Ok(())
    }

    /// Unroll every `comptime for` of `blocks` and the regions below them, in
    /// order: a loop's clones are finished, the loops nested in them
    /// unrolled, before the walk resumes after its replacement. The results
    /// a folded loop leaves are read as the values its break carried, which
    /// only `blocks` read.
    fn unroll_in(
        &mut self,
        blocks: &mut [MirBlock],
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
    ) -> Result<(), MonoError> {
        let mut folded = HashMap::new();
        for block in blocks.iter_mut() {
            let mut index = 0;
            while index < block.instrs.len() {
                let Some(op) = ComptimeForOp::take(&mut block.instrs, index) else {
                    for region in instruction_regions_mut(&mut block.instrs[index]) {
                        self.unroll_in(region, frame, bindings, tables)?;
                    }
                    index += 1;
                    continue;
                };
                let replacement = self.unroll_op(op, frame, bindings, tables, &mut folded)?;
                let count = replacement.len();
                block.instrs.splice(index..index, replacement);
                index += count;
            }
        }
        if !folded.is_empty() {
            replace_register_reads(blocks, &resolve_replacements(&folded));
            for reg in folded.keys() {
                tables.reg_types.remove(reg);
                tables.spans.0.remove(reg);
            }
        }
        Ok(())
    }

    /// The instructions that replace the `comptime for` `op`: its
    /// `param_for_outer` loop of iteration loops, folded where trivial
    /// ([`simplify_param_for_loop`]), each fold of a loop's results recorded
    /// in `folded`.
    fn unroll_op(
        &mut self,
        op: ComptimeForOp,
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
        folded: &mut HashMap<u32, Reg>,
    ) -> Result<Vec<MirInstr>, MonoError> {
        let iterators = self.trip_elements(frame.template, &op.binder, &op.source, bindings)?;
        let [mut body] = <[MirBlock; 1]>::try_from(op.body).map_err(|_| {
            self.error(
                Some(frame.template),
                "comptime for body is not one block".to_string(),
            )
        })?;
        let mut indexed: BTreeSet<VarId> = addressed_slots(std::slice::from_ref(&body))
            .into_iter()
            .filter(|slot| {
                tables
                    .var_tys
                    .get(slot)
                    .is_some_and(|ty| mojito_types::types::names_binder(ty, &op.binder))
            })
            .collect();
        indexed.extend(self.region_generator_slots(
            frame.template,
            std::iter::once(&body),
            &op.binder,
        ));
        tables.retired.extend(indexed.iter().copied());
        let stop = op.source.stop_test();
        let mut operands = op.operands;
        let mut iterations = Vec::new();
        for iterator in iterators {
            let mut iteration = bindings.clone();
            iteration.values.insert(op.binder.clone(), iterator);
            let looped = LoopBody {
                body: &body,
                stop: &stop,
                element: &op.source.next_value,
                args: &op.args,
                indexed: &indexed,
            };
            let clone =
                self.clone_body(&looped, frame, tables, (&iteration, bindings), &operands)?;
            let results: Vec<Reg> = op
                .results
                .iter()
                .map(|&result| tables.fresh_register(result, bindings))
                .collect();
            iterations.push(MirInstr::Loop {
                label: None,
                operands: Vec::new(),
                args: Vec::new(),
                results: results.clone(),
                body: vec![clone],
            });
            operands = results;
        }
        let mut dead = op.args;
        defined_registers(std::slice::from_ref(&body), &mut dead);
        for_each_register(std::slice::from_mut(&mut body), &mut |reg| {
            if tables.dangling.contains(&reg.0) {
                dead.push(*reg);
            }
        });
        defined_registers(&op.orelse, &mut dead);
        tables.forget(&dead);
        // `simplifyParamForLoop` on each iteration loop in order, until one
        // ends the outer body, then on the outer loop.
        let mut outer = MirBlock {
            instrs: Vec::new(),
            term: MirTerm::Break {
                label: Some(LoopLabel::PARAM_FOR_OUTER),
                values: operands,
            },
        };
        let mut iterations = iterations.into_iter();
        for iteration in iterations.by_ref() {
            match simplify_param_for_loop(iteration, folded) {
                Simplified::Kept(iteration) => outer.instrs.push(*iteration),
                Simplified::Spliced(instrs) => outer.instrs.extend(instrs),
                Simplified::Breakless { body, results } => {
                    tables.forget(&results);
                    outer.instrs.extend(body.instrs);
                    outer.term = body.term;
                    break;
                }
            }
        }
        for erased in iterations {
            let mut dead = Vec::new();
            defined_registers(
                &[MirBlock {
                    instrs: vec![erased],
                    term: MirTerm::Unreachable,
                }],
                &mut dead,
            );
            tables.forget(&dead);
        }
        let outer = MirInstr::Loop {
            label: Some(LoopLabel::PARAM_FOR_OUTER),
            operands: Vec::new(),
            args: Vec::new(),
            results: op.results,
            body: vec![outer],
        };
        Ok(match simplify_param_for_loop(outer, folded) {
            Simplified::Kept(outer) => vec![*outer],
            Simplified::Spliced(instrs) => instrs,
            // A loop no break leaves stays, as upstream keeps it.
            Simplified::Breakless { body, results } => vec![MirInstr::Loop {
                label: Some(LoopLabel::PARAM_FOR_OUTER),
                operands: Vec::new(),
                args: Vec::new(),
                results,
                body: vec![body],
            }],
        })
    }

    /// One clone of the loop's body for `iteration`, which binds the loop's
    /// binder: its leading `has_next` test decided, keeping the suite
    /// (whose element application is answered first) or the stop arm; each
    /// register it defines, or reads with no definition, fresh and typed
    /// under the iteration, and each read of a body argument its value in
    /// `operands`, as upstream's `IRMapping` and `replaceArgs` remap a clone;
    /// its `comptime.for.continue`s and `comptime.for.break`s breaks of its
    /// iteration loop and of the outer loop; each slot typed over the binder
    /// it addresses fresh at its type for the iteration; then the loops nested
    /// in it unrolled, reads of the binder folded, types substituted, and
    /// its compile-time branches decided.
    fn clone_body(
        &mut self,
        looped: &LoopBody<'_>,
        frame: &LoopFrame<'_>,
        tables: &mut FrameTables<'_>,
        (iteration, bindings): (&Bindings, &Bindings),
        operands: &[Reg],
    ) -> Result<MirBlock, MonoError> {
        let LoopBody {
            body,
            stop,
            element,
            args,
            indexed,
        } = *looped;
        let template = frame.template;
        let mut clone = body.clone();
        let suite = self.decide_comptime_condition(template, stop, iteration)?;
        if suite {
            self.answer_element(template, element, iteration)?;
        }
        let opens_with_test = matches!(
            clone.instrs.first(),
            Some(MirInstr::ComptimeIf { conds, .. }) if conds.as_slice() == std::slice::from_ref(stop)
        );
        if !opens_with_test {
            return Err(self.error(
                Some(template),
                "comptime for body does not open with its has_next test".to_string(),
            ));
        }
        // The untaken arm is the template's, whose tables go with the body's.
        splice_comptime_arm(&mut clone, 0, suite.then_some(0), &mut Decided::default())
            .map_err(|construct| self.error(Some(template), construct))?;
        let mut local = Vec::new();
        defined_registers(std::slice::from_ref(&clone), &mut local);
        for_each_register(std::slice::from_mut(&mut clone), &mut |reg| {
            if tables.dangling.contains(&reg.0) {
                local.push(*reg);
            }
        });
        let mut registers: HashMap<u32, Reg> = args
            .iter()
            .map(|arg| arg.0)
            .zip(operands.iter().copied())
            .collect();
        let mut fresh = Vec::with_capacity(local.len());
        for reg in local {
            if registers.contains_key(&reg.0) {
                continue;
            }
            let new = tables.fresh_register(reg, iteration);
            if tables.dangling.contains(&reg.0) {
                tables.dangling.insert(new.0);
            }
            registers.insert(reg.0, new);
            fresh.push(new);
        }
        for_each_register(std::slice::from_mut(&mut clone), &mut |reg| {
            if let Some(&to) = registers.get(&reg.0) {
                *reg = to;
            }
        });
        retarget_comptime_exits(std::slice::from_mut(&mut clone));
        let addressed: BTreeSet<VarId> = addressed_slots(std::slice::from_ref(&clone))
            .intersection(indexed)
            .copied()
            .collect();
        let slots = fresh_slots(&addressed, tables, iteration);
        if !slots.is_empty() {
            let renumber = |var: VarId| slots.get(&var).copied().unwrap_or(var);
            renumber_block_slots(std::slice::from_mut(&mut clone), &renumber);
            for register in &fresh {
                if let Some((_, Some(origin))) = tables.spans.0.get_mut(&register.0) {
                    *origin = renumber(*origin);
                }
            }
        }
        let blocks = std::slice::from_mut(&mut clone);
        // The loops nested in the clone first: their bounds and their bodies
        // are over this iteration's binding and their own, which nothing
        // else of the clone may be substituted without.
        self.unroll_in(blocks, frame, iteration, tables)?;
        let locals = bound_parameter_locals(frame.scope, bindings);
        substitute_value_parameter_reads(
            blocks,
            tables.var_names,
            tables.var_tys,
            &locals,
            &bindings.callables,
        )?;
        default_construct_parameters(blocks, tables.n_regs, tables.reg_types, iteration)?;
        substitute_blocks_metadata(blocks, iteration)?;
        self.answer_param_constants(
            template,
            blocks,
            iteration,
            tables.n_regs,
            tables.reg_types,
            false,
        )?;
        let mut decided = Decided::default();
        self.select_comptime_ifs(template, blocks, iteration, &mut decided)?;
        decided.apply(blocks, tables.reg_types, tables.spans);
        let mut dropped = Vec::new();
        self.instantiate_region_references(template, blocks, iteration, &mut dropped)?;
        // A dropped operand's defining chain was erased with the removed
        // argument, so its fresh register is defined by nothing.
        dropped.retain(|reg| tables.dangling.contains(&reg.0));
        tables.forget(&dropped);
        Ok(clone)
    }

    /// Point each direct call, closure, and function value of a region copy
    /// that names a body nested in `template` inside the region, or in a
    /// body nested there ([`Self::region_binders_up_to`]), at that body's
    /// instance under the copy's `iteration`: the nested body is a
    /// generator over the region's binders, as over its enclosing
    /// declaration's, so each clone of the region declares its own. A
    /// direct call names the generator with the body's unused arguments
    /// removed and passes none for them, as a call outside the region does,
    /// while a closure or function value names the full one. A generic
    /// nested body names the copy's generator instead, whose calls
    /// instantiate it with their own arguments beside the copy's. One inside
    /// a loop nested in this copy was pointed at its instance when that
    /// loop's copies were. Each operand a reduced call stops passing joins
    /// `dropped`.
    fn instantiate_region_references(
        &mut self,
        template: &str,
        blocks: &mut [MirBlock],
        iteration: &Bindings,
        dropped: &mut Vec<Reg>,
    ) -> Result<(), MonoError> {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            let (target, call) = match instruction {
                MirInstr::MakeClosure {
                    function: target, ..
                }
                | MirInstr::Const {
                    k: Const::Function(target),
                    ..
                } => (target, None),
                MirInstr::Call {
                    func: mojito_mir::mir::FuncRef(target),
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    ..
                } => (target, Some((args, arg_places, kwargs, kwarg_places))),
                instruction => {
                    for blocks in instruction_regions_mut(instruction) {
                        self.instantiate_region_references(template, blocks, iteration, dropped)?;
                    }
                    continue;
                }
            };
            let Some(generic) = self
                .region_binders_up_to(target, template)
                .filter(|binders| {
                    !binders.is_empty()
                        && binders
                            .iter()
                            .all(|binder| iteration.values.contains_key(binder))
                })
                .and_then(|_| self.declarations.get(target.as_str()))
                .map(|declaration| !declaration.param_decls.is_empty())
            else {
                continue;
            };
            if generic {
                let (bindings, arguments) = self.nested_bindings(target, iteration, false)?;
                let generator = mojito_symbol::symbol::instance_symbol(target, &arguments);
                self.region_generators
                    .insert(generator.clone(), (target.clone(), bindings));
                *target = generator;
                continue;
            }
            let reduced = call.is_some()
                && self
                    .functions
                    .get(target.as_str())
                    .is_some_and(|function| !function.removed_args.is_empty());
            let (bindings, arguments) = self.nested_bindings(target, iteration, reduced)?;
            let instance = self.enqueue(target, bindings, arguments)?;
            let generator = std::mem::replace(target, instance);
            if reduced && let Some((args, arg_places, kwargs, kwarg_places)) = call {
                dropped.extend(self.drop_removed_arguments(
                    template,
                    &generator,
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                )?);
            }
        }
        Ok(())
    }

    /// The binders of the regions body `target` and each body it is nested
    /// in below `template` are declared in, when `template` encloses it: a
    /// body nested in a region body that was inlined into `template` is a
    /// generator over both regions' binders, as one nested in `template`'s
    /// own region is over that region's.
    fn region_binders_up_to(&self, target: &str, template: &str) -> Option<Vec<ParamRef>> {
        let mut binders = Vec::new();
        let mut link = self.declarations.get(target)?;
        loop {
            binders.extend(link.region_binders.iter().map(ParamDecl::binder));
            let enclosing = link.enclosing.as_deref()?;
            if enclosing == template {
                return Some(binders);
            }
            link = self.declarations.get(enclosing)?;
        }
    }

    /// The slots of a loop body that bind a closure or function value
    /// naming a body nested in `template` over the loop's `binder`: each
    /// copy binds the instance of its own iteration, so each copy's slot is
    /// fresh, as a slot typed over the binder is.
    fn region_generator_slots<'b>(
        &self,
        template: &str,
        blocks: impl Iterator<Item = &'b MirBlock>,
        binder: &ParamRef,
    ) -> BTreeSet<VarId> {
        let instructions: Vec<&MirInstr> = blocks
            .flat_map(|block| nested_instructions(std::slice::from_ref(block)))
            .collect();
        let generators: HashSet<u32> = instructions
            .iter()
            .filter_map(|instruction| match instruction {
                MirInstr::MakeClosure { dest, function, .. }
                | MirInstr::Const {
                    dest,
                    k: Const::Function(function),
                } => self
                    .region_binders_up_to(function, template)
                    .is_some_and(|binders| binders.contains(binder))
                    .then_some(dest.0),
                _ => None,
            })
            .collect();
        instructions
            .iter()
            .filter_map(|instruction| match instruction {
                MirInstr::DefVar { var, src, .. } if generators.contains(&src.0) => Some(*var),
                _ => None,
            })
            .collect()
    }

    /// The iterators the loop binds under the bindings, in order, as
    /// upstream's `processComptimeForOp` steps them: from the iterable's
    /// `__iter__()`, each iterator `paramfor_has_next` accepts, and
    /// `paramfor_next_iter` gives the next, then the exhausted one, so the
    /// body is unrolled N+1 times and runs its stop arm on the last. The
    /// element is the body's own `paramfor_next_value` read.
    fn trip_elements(
        &mut self,
        template: &str,
        binder: &ParamRef,
        source: &ComptimeSequence,
        bindings: &Bindings,
    ) -> Result<Vec<CtValue>, MonoError> {
        let mut iterator = self.evaluated(template, &source.initial, bindings)?;
        let mut iterators = Vec::new();
        loop {
            let mut stepping = bindings.clone();
            stepping.values.insert(binder.clone(), iterator.clone());
            match self.evaluated(template, &source.has_next, &stepping)? {
                CtValue::Bool(false) => {
                    iterators.push(iterator);
                    return Ok(iterators);
                }
                CtValue::Bool(true) => {}
                other => {
                    return Err(self.error(
                        Some(template),
                        format!("comptime for over `{binder}`: paramfor_has_next gave `{other}`"),
                    ));
                }
            }
            self.spend_unroll_fuel(template)?;
            let next = self.evaluated(template, &source.next_iter, &stepping)?;
            iterators.push(std::mem::replace(&mut iterator, next));
        }
    }

    /// Run an iterator loop copy's element application (`paramfor_next_value`
    /// over the copy's iterator) once, so that every type and parameter
    /// expression of the copy reading the variable folds from its answer
    /// without substituting the instance again.
    fn answer_element(
        &mut self,
        template: &str,
        element: &ParamExpr,
        iteration: &Bindings,
    ) -> Result<(), MonoError> {
        let outer = std::mem::take(&mut *self.applications.pending.borrow_mut());
        // Folding the element records its application as pending.
        let _ = eval_ct(element, iteration);
        let answered = self.answer_pending(template, iteration);
        self.applications.pending.borrow_mut().extend(outer);
        answered.map(drop)
    }

    /// The value of a loop header's expression under the bindings: the
    /// application it is, demanded and run, or its fold.
    fn evaluated(
        &mut self,
        template: &str,
        expression: &ParamExpr,
        bindings: &Bindings,
    ) -> Result<CtValue, MonoError> {
        self.resolve_application(template, expression, bindings)?
            .map_or_else(|| eval_ct(expression, bindings).ok(), Some)
            .ok_or_else(|| {
                self.error(
                    Some(template),
                    format!("comptime for header `{expression}` is not decided by the instance"),
                )
            })
    }

    /// Spend one unit of compile-time fuel on one unrolled iteration.
    fn spend_unroll_fuel(&mut self, template: &str) -> Result<(), MonoError> {
        self.fuel = self.fuel.checked_sub(1).ok_or_else(|| {
            self.error(
                Some(template),
                "comptime for unrolling exceeded the compile-time fuel quota".to_string(),
            )
        })?;
        Ok(())
    }
}

/// A `comptime for` taken out of its block.
struct ComptimeForOp {
    binder: ParamRef,
    source: ComptimeSequence,
    operands: Vec<Reg>,
    args: Vec<Reg>,
    results: Vec<Reg>,
    body: Vec<MirBlock>,
    orelse: Vec<MirBlock>,
}

impl ComptimeForOp {
    /// The op at `index` of `instrs`, removed from them; `None`, leaving
    /// them alone, when the instruction there is another.
    fn take(instrs: &mut Vec<MirInstr>, index: usize) -> Option<Self> {
        let MirInstr::ComptimeFor {
            binder,
            source,
            operands,
            args,
            results,
            body,
            orelse,
        } = instrs.get_mut(index)?
        else {
            return None;
        };
        let op = Self {
            binder: binder.clone(),
            source: source.clone(),
            operands: std::mem::take(operands),
            args: std::mem::take(args),
            results: std::mem::take(results),
            body: std::mem::take(body),
            orelse: std::mem::take(orelse),
        };
        instrs.remove(index);
        Some(op)
    }
}

/// What each clone of a `comptime for` body is cloned from: the template's
/// body, its leading `has_next` test `stop`, the iteration's `element`
/// application, the body's arguments, and the slots typed over the binder.
struct LoopBody<'b> {
    body: &'b MirBlock,
    stop: &'b GenericConstraint,
    element: &'b ParamExpr,
    args: &'b [Reg],
    indexed: &'b BTreeSet<VarId>,
}

/// What one clone needs of the function it is cloned into.
struct LoopFrame<'a> {
    template: &'a str,
    scope: &'a [ParamDecl],
}

/// The function-wide register and slot tables a clone extends.
struct FrameTables<'a> {
    n_regs: &'a mut u32,
    reg_types: &'a mut HashMap<u32, Ty>,
    spans: &'a mut SpanTable,
    n_vars: &'a mut usize,
    var_names: &'a mut Vec<String>,
    var_tys: &'a mut HashMap<VarId, Ty>,
    /// The template slots a clone replaced, removed once every loop is
    /// unrolled.
    retired: &'a mut BTreeSet<VarId>,
    /// The registers a call keeps as the operand of an argument its callee
    /// removed, which no instruction defines
    /// ([`mojito_mir::mir::with_removed_argument_forwards_erased`]): one a
    /// body reads is the body's own, renumbered and retyped in each clone.
    dangling: HashSet<u32>,
}

impl FrameTables<'_> {
    /// A fresh register standing for `reg` in a clone, typed as it is under
    /// `bindings` and spanned as it is. A type over a nested loop's index
    /// keeps its spelling until that loop's own clones retype it.
    fn fresh_register(&mut self, reg: Reg, bindings: &Bindings) -> Reg {
        let new = Reg(*self.n_regs);
        *self.n_regs += 1;
        if let Some(ty) = self.reg_types.get(&reg.0) {
            let ty = substitute_ty(ty, bindings).unwrap_or_else(|_| ty.clone());
            self.reg_types.insert(new.0, ty);
        }
        if let Some(span) = self.spans.0.get(&reg.0).cloned() {
            self.spans.0.insert(new.0, span);
        }
        new
    }

    /// Drop the tables of registers no instruction defines any more, so a
    /// type over a loop's index never reaches the function's substitution.
    fn forget(&mut self, dead: &[Reg]) {
        for reg in dead {
            self.reg_types.remove(&reg.0);
            self.spans.0.remove(&reg.0);
        }
    }
}

/// What `simplifyParamForLoop` makes of a loop.
enum Simplified {
    /// The loop stays.
    Kept(Box<MirInstr>),
    /// One break leaves the loop, and it ends the loop's body: the body
    /// without it stands where the loop stood, the loop's results read as
    /// the break's values.
    Spliced(Vec<MirInstr>),
    /// No break leaves the loop: its body, with its terminator, may stand
    /// for the loop and all that follows it, and its `results` are defined
    /// by nothing.
    Breakless { body: MirBlock, results: Vec<Reg> },
}

/// Upstream's `simplifyParamForLoop` on the loop `instruction`, counting the
/// breaks that leave it as [`loop_breaks_mut`] resolves them: a loop whose
/// one break ends its body is spliced, recording each result's value in
/// `folded`, and one no break leaves is breakless; any other stays.
fn simplify_param_for_loop(instruction: MirInstr, folded: &mut HashMap<u32, Reg>) -> Simplified {
    let MirInstr::Loop {
        label,
        operands,
        args,
        results,
        mut body,
    } = instruction
    else {
        return Simplified::Kept(Box::new(instruction));
    };
    let mut breaks = 0;
    loop_breaks_mut(&mut body, label, &mut |_, _| breaks += 1);
    match <[MirBlock; 1]>::try_from(body) {
        Ok([body]) if args.is_empty() && breaks == 0 => Simplified::Breakless { body, results },
        Ok(
            [
                MirBlock {
                    instrs,
                    term:
                        MirTerm::Break {
                            label: named,
                            values,
                        },
                },
            ],
        ) if args.is_empty() && breaks == 1 && (named.is_none() || named == label) => {
            folded.extend(results.iter().map(|reg| reg.0).zip(values));
            Simplified::Spliced(instrs)
        }
        Ok(body) => Simplified::Kept(Box::new(MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body: body.into(),
        })),
        Err(body) => Simplified::Kept(Box::new(MirInstr::Loop {
            label,
            operands,
            args,
            results,
            body,
        })),
    }
}

/// The registers `function` reads that no instruction of it defines.
fn dangling_registers(function: &mut MirFunction) -> HashSet<u32> {
    let mut defined = Vec::new();
    defined_registers(&function.blocks, &mut defined);
    let defined: HashSet<u32> = defined.into_iter().map(|reg| reg.0).collect();
    let mut dangling = HashSet::new();
    for_each_register(&mut function.blocks, &mut |reg| {
        if !defined.contains(&reg.0) {
            dangling.insert(reg.0);
        }
    });
    dangling
}

/// Turn a clone's `comptime.for.continue`s into breaks of its iteration loop
/// and its `comptime.for.break`s into breaks of the outer loop, as
/// `makeCompletion` does, skipping the `comptime for`s nested in it, whose
/// own they are.
fn retarget_comptime_exits(blocks: &mut [MirBlock]) {
    for block in blocks {
        for instruction in &mut block.instrs {
            if !matches!(instruction, MirInstr::ComptimeFor { .. }) {
                for region in instruction_regions_mut(instruction) {
                    retarget_comptime_exits(region);
                }
            }
        }
        let (label, values) = match &mut block.term {
            MirTerm::ComptimeContinue(values) => (None, std::mem::take(values)),
            MirTerm::ComptimeBreak(values) => {
                (Some(LoopLabel::PARAM_FOR_OUTER), std::mem::take(values))
            }
            _ => continue,
        };
        block.term = MirTerm::Break { label, values };
    }
}

/// Mint one clone's slot for each of the `indexed` slots, typed under the
/// iteration's bindings: the map from the template's slot to the clone's. A
/// type over a nested loop's index keeps its spelling until that loop's own
/// clones give it a slot of their own.
fn fresh_slots(
    indexed: &BTreeSet<VarId>,
    tables: &mut FrameTables<'_>,
    iteration: &Bindings,
) -> HashMap<VarId, VarId> {
    indexed
        .iter()
        .map(|&slot| {
            let fresh = *tables.n_vars as VarId;
            *tables.n_vars += 1;
            let name = tables
                .var_names
                .get(slot as usize)
                .map_or("$slot", String::as_str);
            tables.var_names.push(format!("{name}$unroll{fresh}"));
            if let Some(ty) = tables.var_tys.get(&slot) {
                let ty = substitute_ty(ty, iteration).unwrap_or_else(|_| ty.clone());
                tables.var_tys.insert(fresh, ty);
            }
            (slot, fresh)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(dest: u32) -> MirInstr {
        MirInstr::Const {
            dest: Reg(dest),
            k: Const::Int(1),
        }
    }

    fn breaking(label: Option<LoopLabel>, values: Vec<Reg>) -> MirTerm {
        MirTerm::Break { label, values }
    }

    fn looped(label: Option<LoopLabel>, results: Vec<Reg>, body: MirBlock) -> MirInstr {
        MirInstr::Loop {
            label,
            operands: Vec::new(),
            args: Vec::new(),
            results,
            body: vec![body],
        }
    }

    #[test]
    fn a_loop_its_one_break_ends_is_spliced() {
        let body = MirBlock {
            instrs: vec![constant(2)],
            term: breaking(None, vec![Reg(2)]),
        };
        let mut folded = HashMap::new();
        let Simplified::Spliced(instrs) =
            simplify_param_for_loop(looped(None, vec![Reg(1)], body), &mut folded)
        else {
            panic!("the loop is spliced");
        };
        assert!(matches!(
            instrs.as_slice(),
            [MirInstr::Const { dest: Reg(2), .. }]
        ));
        assert_eq!(folded, HashMap::from([(1, Reg(2))]));
    }

    #[test]
    fn a_loop_no_break_leaves_is_breakless() {
        // The exhausted iteration's stop arm leaves the outer loop, not its
        // own: its body, terminator and all, stands for what follows.
        let outer = Some(LoopLabel::PARAM_FOR_OUTER);
        let body = MirBlock {
            instrs: vec![constant(2)],
            term: breaking(outer, Vec::new()),
        };
        let mut folded = HashMap::new();
        let Simplified::Breakless { body, results } =
            simplify_param_for_loop(looped(None, vec![Reg(1)], body), &mut folded)
        else {
            panic!("no break leaves the loop");
        };
        assert_eq!(results, [Reg(1)]);
        assert!(matches!(body.term, MirTerm::Break { label, .. } if label == outer));
        assert!(folded.is_empty());
    }

    #[test]
    fn a_loop_broken_inside_a_runtime_if_stays() {
        let arm = |term| {
            vec![MirBlock {
                instrs: Vec::new(),
                term,
            }]
        };
        let guarded = MirInstr::If {
            results: Vec::new(),
            cond: Reg(0),
            then: arm(breaking(None, Vec::new())),
            elifs: Vec::new(),
            else_args: Vec::new(),
            orelse: arm(MirTerm::Yield(Vec::new())),
        };
        let body = MirBlock {
            instrs: vec![guarded],
            term: breaking(None, Vec::new()),
        };
        let mut folded = HashMap::new();
        assert!(matches!(
            simplify_param_for_loop(looped(None, Vec::new(), body), &mut folded),
            Simplified::Kept(kept) if matches!(*kept, MirInstr::Loop { .. })
        ));
    }

    #[test]
    fn a_nested_loops_break_does_not_leave_the_outer() {
        // The inner iteration loop's unlabelled break is its own, so the
        // outer loop's one break is its terminator's.
        let outer = Some(LoopLabel::PARAM_FOR_OUTER);
        let inner = looped(
            None,
            Vec::new(),
            MirBlock {
                instrs: Vec::new(),
                term: breaking(None, Vec::new()),
            },
        );
        let body = MirBlock {
            instrs: vec![inner],
            term: breaking(outer, Vec::new()),
        };
        let mut folded = HashMap::new();
        let Simplified::Spliced(instrs) =
            simplify_param_for_loop(looped(outer, Vec::new(), body), &mut folded)
        else {
            panic!("the outer loop is spliced");
        };
        assert!(matches!(
            instrs.as_slice(),
            [MirInstr::Loop { label: None, .. }]
        ));
    }

    #[test]
    fn retargeting_leaves_a_nested_comptime_for_its_own_exits() {
        let nested = MirInstr::ComptimeFor {
            binder: ParamRef {
                id: mojito_types::param_expr::ParamId::new("$comptime_for@f:0..1", 0),
                name: "j".into(),
            },
            source: ComptimeSequence {
                initial: ParamContext::detached()
                    .constant(CtValue::Int(0))
                    .expect("a constant"),
                has_next: ParamContext::detached()
                    .constant(CtValue::Bool(false))
                    .expect("a constant"),
                next_iter: ParamContext::detached()
                    .constant(CtValue::Int(1))
                    .expect("a constant"),
                next_value: ParamContext::detached()
                    .constant(CtValue::Int(0))
                    .expect("a constant"),
            },
            operands: Vec::new(),
            args: Vec::new(),
            results: Vec::new(),
            body: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::ComptimeContinue(Vec::new()),
            }],
            orelse: vec![MirBlock {
                instrs: Vec::new(),
                term: MirTerm::Unreachable,
            }],
        };
        let mut blocks = vec![MirBlock {
            instrs: vec![nested],
            term: MirTerm::ComptimeBreak(vec![Reg(3)]),
        }];
        retarget_comptime_exits(&mut blocks);
        assert!(matches!(
            &blocks[0].term,
            MirTerm::Break { label: Some(LoopLabel::PARAM_FOR_OUTER), values } if values == &[Reg(3)]
        ));
        let MirInstr::ComptimeFor { body, .. } = &blocks[0].instrs[0] else {
            panic!("the nested op stays");
        };
        assert!(matches!(body[0].term, MirTerm::ComptimeContinue(_)));
    }

    #[test]
    fn a_continue_breaks_its_iteration_loop() {
        let mut blocks = vec![MirBlock {
            instrs: Vec::new(),
            term: MirTerm::ComptimeContinue(vec![Reg(1)]),
        }];
        retarget_comptime_exits(&mut blocks);
        assert!(matches!(
            &blocks[0].term,
            MirTerm::Break { label: None, values } if values == &[Reg(1)]
        ));
    }
}
