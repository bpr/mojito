//! Unrolling a `comptime for`: each instance replaces the loop the template
//! keeps with one copy of its body per iterator the protocol steps to, the
//! exhausted one included, each copy keeping the arm of the body's
//! `has_next` test its iterator takes.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::ComptimeSequence;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MirBlockId, SpanTable, comptime_for_body, instruction_regs_mut, renumber_block_slots,
    terminator_regs_mut, terminator_targets, terminator_targets_mut,
};

impl Specializer<'_> {
    /// Unroll every `comptime for` of the instance under its bindings, as
    /// the elaborator does for upstream's `ComptimeForOp`: the iterators are
    /// stepped, and the body is copied N+1 times, once per iterator
    /// `has_next` accepts and once over the exhausted one, each with fresh
    /// registers and the binder bound to its iterator. Each copy decides
    /// the body's leading `has_next` test first and keeps the taken arm
    /// alone; the suite's copy then has its reads folded, its types
    /// substituted, the `comptime if`s over the binder decided, and the
    /// loops nested in it unrolled in turn, while the exhausted copy keeps
    /// only the stop arm, which leaves for the exit. The copies are chained
    /// where the loop stood, a `continue` reaching the next copy and a
    /// `break` the exit. A slot whose type names the binder is a fresh slot
    /// in each copy that addresses it, as each clone of the region
    /// allocates its own `var`, and the template's slot leaves the
    /// function. The drops the analysis placed in the body come with each
    /// copy; no last use is recomputed. An empty range makes the one copy
    /// that takes the stop arm.
    pub(super) fn unroll_comptime_loops(
        &mut self,
        template: &str,
        function: &mut MirFunction,
        scope: &[ParamDecl],
        bindings: &Bindings,
    ) -> Result<(), MonoError> {
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
        };
        let frame = LoopFrame {
            template,
            scope,
            function_level: true,
        };
        let unrolled = self.unroll_in(blocks, 0, &frame, bindings, &mut tables)?;
        if unrolled {
            mojito_mir::mir::prune_unreachable_blocks(function);
        }
        if retired.is_empty() {
            return Ok(());
        }
        // The index is in scope in the body alone, so once the copies stand
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

    /// Unroll the loops headed in `blocks[from..]` as they stand on entry,
    /// outermost first, then those of the `try` regions the surviving blocks
    /// hold; whether any loop was unrolled. A copy a loop appends is
    /// finished under its own bindings before the scan resumes.
    fn unroll_in(
        &mut self,
        blocks: &mut Vec<MirBlock>,
        from: usize,
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
    ) -> Result<bool, MonoError> {
        let end = blocks.len();
        let mut unrolled = false;
        while let Some(header) = outermost_loop(&blocks[..end], from, frame.function_level) {
            self.unroll_loop(blocks, header, frame, bindings, tables)?;
            unrolled = true;
        }
        let regions = LoopFrame {
            function_level: false,
            ..*frame
        };
        for block in &mut blocks[from..end] {
            for instruction in &mut block.instrs {
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    for region in std::iter::once(body)
                        .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                        .chain(orelse.iter_mut())
                        .chain(finalbody.iter_mut())
                    {
                        unrolled |= self.unroll_in(region, 0, &regions, bindings, tables)?;
                    }
                }
            }
        }
        Ok(unrolled)
    }

    /// Unroll the loop headed at `header`: the body blocks each iterator's
    /// copy keeps are copied at the end of `blocks`, the originals left
    /// unreachable for the final pruning.
    fn unroll_loop(
        &mut self,
        blocks: &mut Vec<MirBlock>,
        header: MirBlockId,
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
    ) -> Result<(), MonoError> {
        let MirTerm::ComptimeFor {
            binder,
            source,
            body,
            exit,
        } = blocks[header].term.clone()
        else {
            return Ok(());
        };
        let iterators = self.trip_elements(frame.template, &binder, &source, bindings)?;
        let members = comptime_for_body(blocks, header, body, exit, frame.function_level);
        let test = self.stop_test(frame.template, blocks, body, &members, &source)?;
        let mut indexed: BTreeSet<VarId> =
            addressed_slots(members.iter().map(|&member| &blocks[member]))
                .into_iter()
                .filter(|slot| {
                    tables
                        .var_tys
                        .get(slot)
                        .is_some_and(|ty| mojito_types::types::names_binder(ty, &binder))
                })
                .collect();
        indexed.extend(self.region_generator_slots(
            frame.template,
            members.iter().map(|&member| &blocks[member]),
            &binder,
        ));
        tables.retired.extend(indexed.iter().copied());
        // Each copy's blocks come first in what its iteration appends; the
        // loops nested in it follow.
        let mut copies: Vec<(MirBlockId, MirBlockId)> = Vec::new();
        for iterator in iterators {
            let mut iteration = bindings.clone();
            iteration.values.insert(binder.clone(), iterator);
            let suite = self.decide_comptime_condition(frame.template, &test.cond, &iteration)?;
            let taken = if suite { test.then_b } else { test.else_b };
            let kept = kept_blocks(blocks, body, &members, (test.block, taken), frame);
            let first = blocks.len();
            let entry = first + kept.iter().position(|&member| member == body).unwrap_or(0);
            copies.push((first, entry));
            self.copy_body(
                blocks,
                (&kept, test.block, taken),
                frame,
                tables,
                (&iteration, bindings, suite.then_some(&source.next_value)),
                &indexed,
            )?;
        }
        // Chain the copies: a back edge to the header reaches the next copy,
        // and the exhausted one's stop arm leaves for the exit. The back
        // edges are those of everything a copy appended: a nested loop's
        // stop arm threaded past its exit reaches this header directly.
        let ends: Vec<MirBlockId> = copies
            .iter()
            .skip(1)
            .map(|&(first, _)| first)
            .chain(std::iter::once(blocks.len()))
            .collect();
        for (k, (&(first, _), &end)) in copies.iter().zip(&ends).enumerate() {
            let next = copies.get(k + 1).map_or(exit, |&(_, entry)| entry);
            for block in &mut blocks[first..end] {
                retarget(block, header, next, frame.function_level);
            }
        }
        blocks[header].term = MirTerm::Jump(copies.first().map_or(exit, |&(_, entry)| entry));
        for &member in &members {
            let dead = std::mem::replace(
                &mut blocks[member],
                MirBlock {
                    instrs: Vec::new(),
                    term: MirTerm::Return(None),
                },
            );
            forget_registers(dead, tables);
        }
        Ok(())
    }

    /// Append one copy of the loop body's `kept` blocks for `iteration`,
    /// which binds the loop's binder, finished under it: the stop test at
    /// `test` a jump to its `taken` arm, the element answered when the arm
    /// is the suite, registers fresh, each of the `indexed` slots the copy
    /// addresses fresh at its type for the iteration, reads of the binder
    /// folded, types substituted, nested loops unrolled, and the
    /// compile-time branches over the binder decided.
    fn copy_body(
        &mut self,
        blocks: &mut Vec<MirBlock>,
        (kept, test, taken): (&[MirBlockId], MirBlockId, MirBlockId),
        frame: &LoopFrame<'_>,
        tables: &mut FrameTables<'_>,
        (iteration, bindings, element): (&Bindings, &Bindings, Option<&ParamExpr>),
        indexed: &BTreeSet<VarId>,
    ) -> Result<(), MonoError> {
        let first = blocks.len();
        if let Some(element) = element {
            self.answer_element(frame.template, element, iteration)?;
        }
        let mapping: HashMap<MirBlockId, MirBlockId> = kept
            .iter()
            .enumerate()
            .map(|(k, &member)| (member, first + k))
            .collect();
        let mut registers = HashMap::new();
        for &member in kept {
            let mut block = blocks[member].clone();
            if member == test {
                block.term = MirTerm::Jump(taken);
            }
            for target in terminator_targets_mut(&mut block.term) {
                if let Some(&to) = mapping.get(target) {
                    *target = to;
                }
            }
            if frame.function_level {
                remap_escapes(&mut block, &mapping);
            }
            renumber_block(&mut block, &mut registers, tables, iteration)?;
            blocks.push(block);
        }
        let addressed: BTreeSet<VarId> = addressed_slots(&blocks[first..])
            .intersection(indexed)
            .copied()
            .collect();
        let slots = fresh_slots(&addressed, tables, iteration);
        if !slots.is_empty() {
            let renumber = |var: VarId| slots.get(&var).copied().unwrap_or(var);
            renumber_block_slots(&mut blocks[first..], &renumber);
            for register in registers.values() {
                if let Some((_, Some(origin))) = tables.spans.0.get_mut(register) {
                    *origin = renumber(*origin);
                }
            }
        }
        // The loops nested in the copy first: their bounds and their bodies
        // are over this iteration's binding and their own, which nothing
        // else of the copy may be substituted without.
        self.unroll_in(blocks, first, frame, iteration, tables)?;
        let locals = bound_parameter_locals(frame.scope, bindings);
        substitute_value_parameter_reads(
            &mut blocks[first..],
            tables.var_names,
            tables.var_tys,
            &locals,
            &bindings.callables,
        )?;
        default_construct_parameters(
            &mut blocks[first..],
            tables.n_regs,
            tables.reg_types,
            iteration,
        )?;
        substitute_blocks_metadata(&mut blocks[first..], iteration)?;
        self.answer_param_constants(
            frame.template,
            &mut blocks[first..],
            iteration,
            tables.n_regs,
            tables.reg_types,
            false,
        )?;
        self.select_comptime_branches_in(frame.template, &mut blocks[first..], iteration)?;
        self.instantiate_region_references(frame.template, &mut blocks[first..], iteration)
    }

    /// Point each closure and function value of a region copy that names a
    /// body nested in `template` inside the region at that body's instance
    /// under the copy's `iteration`: the nested body is a generator over the
    /// region's binders, as over its enclosing declaration's, so each clone
    /// of the region declares its own. A generic nested body names the
    /// copy's generator instead, whose calls instantiate it with their own
    /// arguments beside the copy's. One inside a loop nested in this copy
    /// was pointed at its instance when that loop's copies were.
    fn instantiate_region_references(
        &mut self,
        template: &str,
        blocks: &mut [MirBlock],
        iteration: &Bindings,
    ) -> Result<(), MonoError> {
        for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
            match instruction {
                MirInstr::MakeClosure {
                    function: target, ..
                }
                | MirInstr::Const {
                    k: Const::Function(target),
                    ..
                } => {
                    let Some(generic) =
                        self.declarations
                            .get(target.as_str())
                            .and_then(|declaration| {
                                (declaration.enclosing.as_deref() == Some(template)
                                    && !declaration.region_binders.is_empty()
                                    && declaration
                                        .region_binders
                                        .iter()
                                        .all(|decl| iteration.values.contains_key(&decl.binder())))
                                .then_some(!declaration.param_decls.is_empty())
                            })
                    else {
                        continue;
                    };
                    let (bindings, arguments) = self.nested_bindings(target, iteration, false)?;
                    *target = if generic {
                        let generator = mojito_symbol::symbol::instance_symbol(target, &arguments);
                        self.region_generators
                            .insert(generator.clone(), (target.clone(), bindings));
                        generator
                    } else {
                        self.enqueue(target, bindings, arguments)?
                    };
                }
                MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } => {
                    for blocks in std::iter::once(body)
                        .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                        .chain(orelse.iter_mut())
                        .chain(finalbody.iter_mut())
                    {
                        self.instantiate_region_references(template, blocks, iteration)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
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
                    .declarations
                    .get(function.as_str())
                    .is_some_and(|declaration| {
                        declaration.enclosing.as_deref() == Some(template)
                            && declaration
                                .region_binders
                                .iter()
                                .any(|decl| decl.binder() == *binder)
                    })
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

    /// The body's `has_next` test: the block among `members` the body
    /// enters by jumps alone — drop elaboration may split the header's edge
    /// to it — whose `comptime if` decides the stop test of `source`.
    fn stop_test(
        &self,
        template: &str,
        blocks: &[MirBlock],
        body: MirBlockId,
        members: &[MirBlockId],
        source: &ComptimeSequence,
    ) -> Result<StopTest, MonoError> {
        let stop = source.stop_test();
        let mut block = body;
        let mut seen = HashSet::new();
        while members.contains(&block) && seen.insert(block) {
            match &blocks[block].term {
                MirTerm::Jump(next) => block = *next,
                MirTerm::ComptimeBranch {
                    cond,
                    then_b,
                    else_b,
                } if **cond == stop => {
                    return Ok(StopTest {
                        block,
                        then_b: *then_b,
                        else_b: *else_b,
                        cond: stop,
                    });
                }
                _ => break,
            }
        }
        Err(self.error(
            Some(template),
            "comptime for body does not open with its has_next test".to_string(),
        ))
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

/// A `comptime for` body's leading `has_next` test: the block that
/// branches on it, its suite and stop arms, and its condition.
struct StopTest {
    block: MirBlockId,
    then_b: MirBlockId,
    else_b: MirBlockId,
    cond: GenericConstraint,
}

/// What one body copy needs of the function it is copied into.
struct LoopFrame<'a> {
    template: &'a str,
    scope: &'a [ParamDecl],
    /// Whether the block list is the function's own, where a `try` region's
    /// escape targets name blocks of this list.
    function_level: bool,
}

/// The function-wide register and slot tables a copy extends.
struct FrameTables<'a> {
    n_regs: &'a mut u32,
    reg_types: &'a mut HashMap<u32, Ty>,
    spans: &'a mut SpanTable,
    n_vars: &'a mut usize,
    var_names: &'a mut Vec<String>,
    var_tys: &'a mut HashMap<VarId, Ty>,
    /// The template slots a copy replaced, removed once every loop is
    /// unrolled.
    retired: &'a mut BTreeSet<VarId>,
}

/// The header of a loop no other loop of `blocks[from..]` encloses, lowest
/// first; `None` once every loop is unrolled.
fn outermost_loop(blocks: &[MirBlock], from: usize, function_level: bool) -> Option<MirBlockId> {
    let headers: Vec<(MirBlockId, Vec<MirBlockId>)> = blocks
        .iter()
        .enumerate()
        .skip(from)
        .filter_map(|(id, block)| match &block.term {
            MirTerm::ComptimeFor { body, exit, .. } => Some((
                id,
                comptime_for_body(blocks, id, *body, *exit, function_level),
            )),
            _ => None,
        })
        .collect();
    headers
        .iter()
        .find(|(header, _)| {
            !headers
                .iter()
                .any(|(other, members)| other != header && members.contains(header))
        })
        .map(|(header, _)| *header)
}

/// The blocks of `members` one copy keeps, in index order: those reached
/// from `body` once the stop test's block `test` leaves for the arm `taken`
/// alone.
fn kept_blocks(
    blocks: &[MirBlock],
    body: MirBlockId,
    members: &[MirBlockId],
    (test, taken): (MirBlockId, MirBlockId),
    frame: &LoopFrame<'_>,
) -> Vec<MirBlockId> {
    let mut reached = BTreeSet::from([body]);
    let mut stack = vec![body];
    while let Some(block) = stack.pop() {
        let successors = if block == test {
            vec![taken]
        } else if frame.function_level {
            mojito_mir::mir::block_successors(&blocks[block])
        } else {
            terminator_targets(&blocks[block].term)
        };
        for successor in successors {
            if members.contains(&successor) && reached.insert(successor) {
                stack.push(successor);
            }
        }
    }
    reached.into_iter().collect()
}

/// Point every edge of a copied block that returned to the loop header at
/// `next`: its own terminator, and at function level the escapes of the
/// regions it holds.
fn retarget(block: &mut MirBlock, header: MirBlockId, next: MirBlockId, function_level: bool) {
    for target in terminator_targets_mut(&mut block.term) {
        if *target == header {
            *target = next;
        }
    }
    if function_level {
        let mut mapping = HashMap::new();
        mapping.insert(header, next);
        remap_escapes(block, &mapping);
    }
}

/// Rewrite the escape targets of every region below `block` through
/// `mapping`.
fn remap_escapes(block: &mut MirBlock, mapping: &HashMap<MirBlockId, MirBlockId>) {
    fn in_blocks(blocks: &mut [MirBlock], mapping: &HashMap<MirBlockId, MirBlockId>) {
        for block in blocks {
            if let MirTerm::EscapeJump { target, .. } = &mut block.term
                && let Some(&to) = mapping.get(target)
            {
                *target = to;
            }
            remap_escapes(block, mapping);
        }
    }
    for instruction in &mut block.instrs {
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            for region in std::iter::once(body)
                .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                .chain(orelse.iter_mut())
                .chain(finalbody.iter_mut())
            {
                in_blocks(region, mapping);
            }
        }
    }
}

/// Give every register of a copied block — the regions it holds included —
/// a fresh number, typed as the original under the iteration's bindings. A
/// type over a nested loop's index keeps its spelling until that loop's
/// own copies retype it.
fn renumber_block(
    block: &mut MirBlock,
    registers: &mut HashMap<u32, u32>,
    tables: &mut FrameTables<'_>,
    iteration: &Bindings,
) -> Result<(), MonoError> {
    for instruction in &mut block.instrs {
        for reg in instruction_regs_mut(instruction) {
            fresh_register(reg, registers, tables, iteration);
        }
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            for region in std::iter::once(body)
                .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                .chain(orelse.iter_mut())
                .chain(finalbody.iter_mut())
            {
                for inner in region.iter_mut() {
                    renumber_block(inner, registers, tables, iteration)?;
                }
            }
        }
    }
    for reg in terminator_regs_mut(&mut block.term) {
        fresh_register(reg, registers, tables, iteration);
    }
    Ok(())
}

/// Renumber one register through `registers`, minting its fresh number and
/// tables on first sight.
fn fresh_register(
    reg: &mut Reg,
    registers: &mut HashMap<u32, u32>,
    tables: &mut FrameTables<'_>,
    iteration: &Bindings,
) {
    let old = reg.0;
    if let Some(&new) = registers.get(&old) {
        reg.0 = new;
        return;
    }
    let new = *tables.n_regs;
    *tables.n_regs += 1;
    registers.insert(old, new);
    if let Some(ty) = tables.reg_types.get(&old) {
        let ty = substitute_ty(ty, iteration).unwrap_or_else(|_| ty.clone());
        tables.reg_types.insert(new, ty);
    }
    if let Some(span) = tables.spans.0.get(&old).cloned() {
        tables.spans.0.insert(new, span);
    }
    reg.0 = new;
}

/// Mint one copy's slot for each of the `indexed` slots, typed under the
/// iteration's bindings: the map from the template's slot to the copy's. A
/// type over a nested loop's index keeps its spelling until that loop's own
/// copies give it a slot of their own.
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

/// Drop the register tables of a block left unreachable, so a type over
/// the loop's index never reaches the function's substitution.
fn forget_registers(mut block: MirBlock, tables: &mut FrameTables<'_>) {
    for instruction in &mut block.instrs {
        for reg in instruction_regs_mut(instruction) {
            tables.reg_types.remove(&reg.0);
            tables.spans.0.remove(&reg.0);
        }
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            for region in std::iter::once(body)
                .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                .chain(orelse.iter_mut())
                .chain(finalbody.iter_mut())
            {
                for inner in std::mem::take(region) {
                    forget_registers(inner, tables);
                }
            }
        }
    }
    for reg in terminator_regs_mut(&mut block.term) {
        tables.reg_types.remove(&reg.0);
        tables.spans.0.remove(&reg.0);
    }
}
