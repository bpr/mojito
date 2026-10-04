//! Unrolling a `comptime for`: each instance replaces the loop the template
//! keeps with one copy of its body per iteration, the index bound in each.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    MirBlockId, SpanTable, instruction_regs_mut, terminator_regs_mut, terminator_targets,
    terminator_targets_mut,
};

impl Specializer<'_> {
    /// Unroll every `comptime for` of the instance under its bindings, as
    /// the elaborator does for upstream's `ComptimeForOp`: the range is
    /// evaluated, the body is copied once per iteration with fresh registers
    /// and the index bound to the iteration's value — its reads folded, its
    /// types substituted, the `comptime if`s over it decided, the loops
    /// nested in it unrolled in turn — and the copies are chained where the
    /// loop stood, a `continue` reaching the next copy and a `break` the
    /// exit. A slot whose type names the index is a fresh slot in each copy,
    /// as each clone of the region allocates its own `var`, and the
    /// template's slot leaves the function. The drops the analysis placed in
    /// the body come with each copy; no last use is recomputed. An empty
    /// range leaves the exit alone.
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

    /// Unroll the loop headed at `header`: its body blocks are copied per
    /// iteration at the end of `blocks`, the originals left unreachable for
    /// the final pruning.
    fn unroll_loop(
        &mut self,
        blocks: &mut Vec<MirBlock>,
        header: MirBlockId,
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
    ) -> Result<(), MonoError> {
        let MirTerm::ComptimeFor {
            index,
            slot,
            start,
            stop,
            step,
            body,
            exit,
        } = blocks[header].term.clone()
        else {
            return Ok(());
        };
        let values = self.trip_values(frame.template, &index, [&start, &stop, &step], bindings)?;
        let members = loop_body(blocks, header, body, exit, frame.function_level);
        let width = members.len();
        let indexed: BTreeSet<VarId> =
            addressed_slots(members.iter().map(|&member| &blocks[member]))
                .into_iter()
                .filter(|slot| {
                    tables
                        .var_tys
                        .get(slot)
                        .is_some_and(|ty| mojito_types::types::names_binder(ty, &index))
                })
                .collect();
        tables.retired.extend(indexed.iter().copied());
        let entry = members
            .iter()
            .position(|&member| member == body)
            .unwrap_or(0);
        // Each copy's blocks come first in what its iteration appends; the
        // loops nested in it follow.
        let mut firsts = Vec::with_capacity(values.len());
        for value in &values {
            firsts.push(blocks.len());
            self.copy_body(
                blocks,
                &members,
                frame,
                bindings,
                tables,
                (&index, slot, *value),
                &indexed,
            )?;
        }
        // Chain the copies: a back edge to the header reaches the next copy,
        // and the last one the exit.
        for (k, &first) in firsts.iter().enumerate() {
            let next = firsts.get(k + 1).map_or(exit, |first| first + entry);
            for block in &mut blocks[first..first + width] {
                retarget(block, header, next, frame.function_level);
            }
        }
        blocks[header].term = MirTerm::Jump(firsts.first().map_or(exit, |first| first + entry));
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

    /// Append one copy of the loop body for the iteration binding `index`
    /// to `value`, finished under that binding: registers fresh, each of the
    /// `indexed` slots fresh at its type for the iteration, reads of the
    /// index folded, types substituted, nested loops unrolled, and the
    /// compile-time branches over the index decided.
    #[allow(clippy::too_many_arguments, reason = "one copy's parameters")]
    fn copy_body(
        &mut self,
        blocks: &mut Vec<MirBlock>,
        members: &[MirBlockId],
        frame: &LoopFrame<'_>,
        bindings: &Bindings,
        tables: &mut FrameTables<'_>,
        (index, slot, value): (&ParamRef, VarId, i64),
        indexed: &BTreeSet<VarId>,
    ) -> Result<(), MonoError> {
        let first = blocks.len();
        let mut iteration = bindings.clone();
        iteration.values.insert(index.clone(), CtValue::Int(value));
        let mapping: HashMap<MirBlockId, MirBlockId> = members
            .iter()
            .enumerate()
            .map(|(k, &member)| (member, first + k))
            .collect();
        let mut registers = HashMap::new();
        for &member in members {
            let mut block = blocks[member].clone();
            for target in terminator_targets_mut(&mut block.term) {
                if let Some(&to) = mapping.get(target) {
                    *target = to;
                }
            }
            if frame.function_level {
                remap_escapes(&mut block, &mapping);
            }
            renumber_block(&mut block, &mut registers, tables, &iteration)?;
            blocks.push(block);
        }
        let slots = fresh_slots(indexed, tables, &iteration);
        if !slots.is_empty() {
            let renumber = |var: VarId| slots.get(&var).copied().unwrap_or(var);
            renumber_blocks(&mut blocks[first..], &renumber);
            for register in registers.values() {
                if let Some((_, Some(origin))) = tables.spans.0.get_mut(register) {
                    *origin = renumber(*origin);
                }
            }
        }
        // The loops nested in the copy first: their bounds and their bodies
        // are over this iteration's binding and their own, which nothing
        // else of the copy may be substituted without.
        self.unroll_in(blocks, first, frame, &iteration, tables)?;
        let bound = CtValue::Int(value);
        let mut locals = bound_parameter_locals(frame.scope, bindings);
        if let Some(name) = tables.var_names.get(slot as usize) {
            locals.insert(name.clone(), &bound);
        }
        substitute_value_parameter_reads(
            &mut blocks[first..],
            tables.var_names,
            tables.var_tys,
            &locals,
            &bindings.callables,
        )?;
        substitute_blocks_metadata(&mut blocks[first..], &iteration)?;
        self.select_comptime_branches_in(frame.template, &mut blocks[first..], &iteration)?;
        Ok(())
    }

    /// The index values the range spans under the bindings, in order: a
    /// zero step is an empty range, as `range` is.
    fn trip_values(
        &mut self,
        template: &str,
        index: &ParamRef,
        bounds: [&ParamExpr; 3],
        bindings: &Bindings,
    ) -> Result<Vec<i64>, MonoError> {
        let mut evaluated = [0i64; 3];
        for (value, bound) in evaluated.iter_mut().zip(bounds) {
            *value = eval_ct(bound, bindings)
                .ok()
                .as_ref()
                .and_then(mojito_types::param_expr::fold::integer_value)
                .and_then(|value| value.to_i64())
                .ok_or_else(|| {
                    self.error(
                        Some(template),
                        format!(
                            "comptime for over `{index}` has the bound `{bound}` the instance does not decide"
                        ),
                    )
                })?;
        }
        let [start, stop, step] = evaluated;
        let mut values = Vec::new();
        let mut current = start;
        while (step > 0 && current < stop) || (step < 0 && current > stop) {
            self.fuel = self.fuel.checked_sub(1).ok_or_else(|| {
                self.error(
                    Some(template),
                    "comptime for unrolling exceeded the compile-time fuel quota".to_string(),
                )
            })?;
            values.push(current);
            current = current.checked_add(step).ok_or_else(|| {
                self.error(
                    Some(template),
                    format!("comptime for over `{index}` overflows its index"),
                )
            })?;
        }
        Ok(values)
    }
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
            MirTerm::ComptimeFor { body, exit, .. } => {
                Some((id, loop_body(blocks, id, *body, *exit, function_level)))
            }
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

/// The blocks of a loop's body: those its entry dominates, in index order.
/// Reachability would not do: drop elaboration splits a `break` arm's edge
/// so it leaves for the block after the loop directly, past the exit. At
/// function level a `try` region's escapes are edges of this list too.
fn loop_body(
    blocks: &[MirBlock],
    header: MirBlockId,
    body: MirBlockId,
    exit: MirBlockId,
    function_level: bool,
) -> Vec<MirBlockId> {
    let successors = |block: MirBlockId| {
        if function_level {
            mojito_mir::mir::block_successors(&blocks[block])
        } else {
            terminator_targets(&blocks[block].term)
        }
    };
    let mut predecessors: Vec<Vec<MirBlockId>> = vec![Vec::new(); blocks.len()];
    for block in 0..blocks.len() {
        for successor in successors(block) {
            if successor < blocks.len() {
                predecessors[successor].push(block);
            }
        }
    }
    // Dominators by iteration: the entry dominates itself alone; every other
    // block is dominated by itself and by what dominates all of its
    // predecessors. An unreached block keeps the full set and is no member.
    let everything: HashSet<MirBlockId> = (0..blocks.len()).collect();
    let mut dominators: Vec<HashSet<MirBlockId>> = vec![everything; blocks.len()];
    dominators[0] = HashSet::from([0]);
    let mut changed = true;
    while changed {
        changed = false;
        for block in 1..blocks.len() {
            let mut next: Option<HashSet<MirBlockId>> = None;
            for &predecessor in &predecessors[block] {
                next = Some(match next {
                    None => dominators[predecessor].clone(),
                    Some(set) => set
                        .intersection(&dominators[predecessor])
                        .copied()
                        .collect(),
                });
            }
            let mut next = next.unwrap_or_default();
            next.insert(block);
            if next != dominators[block] {
                dominators[block] = next;
                changed = true;
            }
        }
    }
    let mut members: Vec<MirBlockId> = (0..blocks.len())
        .filter(|&block| {
            block != header
                && block != exit
                && !predecessors[block].is_empty()
                && dominators[block].contains(&body)
        })
        .collect();
    members.sort_unstable();
    members
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
