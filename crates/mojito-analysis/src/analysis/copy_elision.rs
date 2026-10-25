//! Copy elision at a dying source: after drop elaboration, a copy
//! construction `Name(copy=v)`, a call of the struct's `__copyinit__` or
//! the `value.copy` a register-passable struct's runs as, whose
//! whole owned source `v` dies right at it becomes `v`'s move, as Mojo's
//! lifetime checker turns a copy initialization followed by the source's
//! destruction into a move.
//!
//! The rewrite keys on the `DropVar` drop elaboration placed directly after
//! the construction (or after its in-place binding): that drop is the proof
//! that `v` is dead there on every path, loans included. Read, `ref`, and
//! `deinit` slots get no such drop, and a field source is not a whole slot,
//! so neither is rewritten. A type parameter's `T(copy=x)` is still a
//! `ConstructTypeParam` here and stays a copy, as at Mojo. A source that dies
//! only at a region's entry keeps its copy, and no scan reaches past a
//! region op other than `try`.
//!
//! A copy an owned parameter or owned receiver takes is bound in place to a
//! forwarding slot ([`FORWARDED_ARGUMENT_SLOT`]) whose forwarding use sits
//! at the call's setup, after the later operands. When its source dies there
//! and nothing from the source's death to the call can raise or names the
//! source, the construction, the slot, and the source's death all go, and the call takes the source's storage itself: no
//! move initializer runs, as Mojo's lifetime checker hands a dying source to
//! an owned argument. A raising operand after the construction keeps the
//! move, as at the pin.
//!
//! A copy a read parameter, read receiver, or read operator operand takes
//! is bound to a read-temporary slot ([`READ_TEMPORARY_SLOT`]) the call
//! reads once. When its source dies there and nothing evaluated between the binding and the
//! call can raise, the source is lent instead: the construction, the
//! binding, and the source's death go, the call reads the source where it
//! lies, and every death of the slot, its `DropVar`, becomes the source's, as Mojo binds the borrowed
//! temporary to the dead source's own storage. A raising argument after the
//! construction keeps the move, as at Mojo. A copy any other binding takes
//! still becomes a move.
//!
//! The other half of upstream's `optimizeCopyDestroys` handles a copy at a
//! death of its own: a copy (a `var.use` copy, a `value.copy`, or a
//! `__copyinit__` call) or a constant materializing a parameter value whose
//! one use is the `drop.reg` drop elaboration placed right after it is
//! deleted with that drop, and a `var.use` copy whose source slot's
//! `DropVar` follows it (or its binding) in the run of drops becomes the
//! slot's move.
//!
//! A dying copy, explicit or implicit, that no binding takes and that an
//! owned parameter, owned receiver, or owned operator operand takes at its
//! setup ([`RegisterEnds::forwarded`]), with nothing between that can
//! raise or names the source, is forwarded instead: the callee takes the
//! source's storage with no move initializer, as upstream's lifetime check
//! merges an owned argument's copy with its source's destruction. One a
//! binding, an owning collector, or an aggregate takes moves into storage
//! of its own.

use super::scan::{for_each_instr_deep, var_def, var_uses, vars_moved};
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_operand_regs;
use mojito_mir::mir::{
    FORWARDED_ARGUMENT_SLOT, MirBlock, MirDeclarations, MirFunction, MirInstr, MirPlace,
    READ_TEMPORARY_SLOT, Reg, RegisterEnds, UseMode, instruction_regions, instruction_regions_mut,
    is_environment_slot, terminator_regs,
};
use std::collections::{HashMap, HashSet};

/// Rewrite every dying named-struct copy construction in `function`, `Try`
/// regions included, into a move of its source, into its source's
/// forwarding when an owned parameter takes it, or into its source's loan
/// when a read parameter takes it.
pub(super) fn elide_dying_copies(function: &mut MirFunction, declarations: &MirDeclarations) {
    let mut uses = HashMap::new();
    count_operand_uses(&function.blocks, &mut uses);
    let forwarded = forwarded_registers(function, declarations);
    let elision = Elision {
        var_names: &function.var_names,
        uses,
        mentions: count_slot_mentions(&function.blocks),
        forwarded,
    };
    let mut lent = Vec::new();
    rewrite_blocks(&mut function.blocks, &elision, &mut lent);
    for loan in &lent {
        if let Some((_, origin)) = function.spans.0.get_mut(&loan.load.0) {
            *origin = Some(loan.source);
        }
    }
    rename_lent_slots(&mut function.blocks, &lent);
}

/// What the rewrite reads of the whole function.
struct Elision<'a> {
    var_names: &'a [String],
    /// Operand uses of each register.
    uses: HashMap<u32, usize>,
    /// Mentions of each slot outside its `DropVar`s.
    mentions: HashMap<VarId, usize>,
    /// The registers an instruction hands its callee by storage
    /// ([`RegisterEnds::forwarded`]).
    forwarded: HashSet<u32>,
}

/// A read-temporary slot whose binding a lent source replaced.
struct Loan {
    slot: VarId,
    source: VarId,
    load: Reg,
}

/// Every register some instruction of `function`, at any depth, hands its
/// callee by storage.
fn forwarded_registers(function: &MirFunction, declarations: &MirDeclarations) -> HashSet<u32> {
    let ends = RegisterEnds::new(function, declarations);
    let mut forwarded = Vec::new();
    for_each_instr_deep(&function.blocks, &mut |instruction| {
        ends.forwarded(instruction, &mut forwarded);
    });
    forwarded.into_iter().map(|reg| reg.0).collect()
}

fn count_operand_uses(blocks: &[MirBlock], uses: &mut HashMap<u32, usize>) {
    let mut operands = Vec::new();
    for block in blocks {
        for instruction in &block.instrs {
            operands.clear();
            instruction_operand_regs(instruction, &mut operands);
            for register in &operands {
                *uses.entry(register.0).or_default() += 1;
            }
            for region in instruction_regions(instruction) {
                count_operand_uses(region, uses);
            }
        }
        for register in terminator_regs(&block.term) {
            *uses.entry(register.0).or_default() += 1;
        }
    }
}

/// How often each slot is named outside its `DropVar`s:
/// a read-temporary slot that only its binding and its one load name is
/// one a lent source can stand in for.
fn count_slot_mentions(blocks: &[MirBlock]) -> HashMap<VarId, usize> {
    let mut mentions = HashMap::new();
    for_each_instr_deep(blocks, &mut |instruction| {
        // A `try` reports its regions' uses as its own; the walk visits them.
        let uses = if matches!(instruction, MirInstr::Try { .. }) {
            Vec::new()
        } else {
            var_uses(instruction)
        };
        for var in uses
            .into_iter()
            .map(|(var, _)| var)
            .chain(var_def(instruction))
            .chain(vars_moved(instruction))
        {
            *mentions.entry(var).or_default() += 1;
        }
    });
    mentions
}

fn rewrite_blocks(blocks: &mut [MirBlock], elision: &Elision, lent: &mut Vec<Loan>) {
    for block in blocks {
        for instruction in &mut block.instrs {
            for region in instruction_regions_mut(instruction) {
                rewrite_blocks(region, elision, lent);
            }
        }
        delete_dead_copies(&mut block.instrs, &elision.uses);
        move_dying_slot_copies(&mut block.instrs, elision);
        let mut at = 1;
        while at < block.instrs.len() {
            let instrs = &mut block.instrs;
            let Some(source) = dying_copy(instrs, at, &elision.uses) else {
                at += 1;
                continue;
            };
            if let Some(forward_at) = forwarding_use(instrs, source, elision.var_names) {
                forward(instrs, at, source, forward_at);
            } else if let Some(lending) = lending_use(instrs, at, source, elision) {
                lent.push(lend(instrs, at, source, lending));
            } else {
                elide(instrs, at, source, elision);
            }
        }
    }
}

/// A dying copy construction rewritten into a move.
#[derive(Clone, Copy)]
struct DyingCopy {
    var: VarId,
    drop_at: usize,
    binding_at: Option<usize>,
    /// The binding of the copy into a slot that is not in place, when it
    /// sits between the construction and the source's death.
    held_at: Option<usize>,
}

/// The source slot and its death of the copy construction at `at`, when the
/// construction copies a whole slot loaded just before it and that slot's
/// `DropVar` follows it (or its in-place binding) in the run of drops.
fn dying_copy(instrs: &[MirInstr], at: usize, uses: &HashMap<u32, usize>) -> Option<DyingCopy> {
    let (dest, operand) = copy_construction(&instrs[at])?;
    // A load through a handle reads a referent the slot does not own.
    let MirInstr::LoadPlace {
        dest: loaded,
        place:
            MirPlace {
                root,
                proj,
                through: None,
                ..
            },
    } = &instrs[at - 1]
    else {
        return None;
    };
    if loaded != operand || !proj.is_empty() || uses.get(&operand.0) != Some(&1) {
        return None;
    }
    let var = *root;
    let (binding_at, held_at) = match instrs.get(at + 1) {
        Some(MirInstr::DefVar { src, in_place, .. }) if src == dest => {
            if *in_place {
                (Some(at + 1), None)
            } else {
                (None, Some(at + 1))
            }
        }
        _ => (None, None),
    };
    let after = binding_at.or(held_at).unwrap_or(at);
    let drop_at = instrs[after + 1..]
        .iter()
        .take_while(|instruction| is_drop(instruction))
        .position(
            |instruction| matches!(instruction, MirInstr::DropVar { var: dying, .. } if *dying == var),
        )?
        + after
        + 1;
    Some(DyingCopy {
        var,
        drop_at,
        binding_at,
        held_at,
    })
}

/// The result and the copied operand of `instruction` when it is a copy
/// construction: a call of a struct's `__copyinit__` (`Name(copy=v)`), or
/// the `value.copy` a register-passable struct's runs as.
fn copy_construction(instruction: &MirInstr) -> Option<(&Reg, &Reg)> {
    match instruction {
        MirInstr::Call {
            dest,
            func,
            raises: None,
            args,
            kwargs,
            ..
        } => {
            let [(keyword, operand)] = kwargs.as_slice() else {
                return None;
            };
            let copy_constructor = matches!(
                mojito_symbol::symbol::lifecycle_constructor(&func.0),
                Some((_, "__copyinit__"))
            );
            (keyword == "copy" && args.is_empty() && copy_constructor).then_some((dest, operand))
        }
        MirInstr::CopyValue { dest, value, .. } => Some((dest, value)),
        _ => None,
    }
}

/// Where the call takes the forwarding slot the dying copy is bound to: the
/// slot's one forwarding use later in the block, before any region op, when
/// nothing past the
/// source's death up to the call that consumes the slot can raise, names
/// the source, or names the slot.
fn forwarding_use(instrs: &[MirInstr], source: DyingCopy, var_names: &[String]) -> Option<usize> {
    let MirInstr::DefVar { var: slot, .. } = instrs[source.binding_at.or(source.held_at)?] else {
        return None;
    };
    if !var_names
        .get(slot as usize)
        .is_some_and(|name| name.starts_with(FORWARDED_ARGUMENT_SLOT))
    {
        return None;
    }
    let end = segment_end(instrs, source.drop_at + 1);
    let (forward_at, forwarded) = instrs[..end]
        .iter()
        .enumerate()
        .skip(source.drop_at + 1)
        .find_map(|(at, instruction)| match instruction {
            MirInstr::UseVar {
                dest,
                var,
                mode: UseMode::Forward,
                ..
            } if *var == slot => Some((at, *dest)),
            _ => None,
        })?;
    let consumer_at = first_reader(instrs, forward_at + 1..end, forwarded)?;
    (source.drop_at + 1..consumer_at)
        .all(|at| {
            let instruction = &instrs[at];
            quiet(instruction, source.var) && (at == forward_at || !names(instruction, slot))
        })
        .then_some(forward_at)
}

/// Hand the dying source to the call that takes the forwarding slot: the
/// load, the construction, the slot's binding, and the source's death go,
/// and the slot's forwarding use forwards the source instead.
fn forward(instrs: &mut Vec<MirInstr>, at: usize, source: DyingCopy, forward_at: usize) {
    if let MirInstr::UseVar { var, .. } = &mut instrs[forward_at] {
        *var = source.var;
    }
    instrs.remove(source.drop_at);
    if let Some(binding) = source.binding_at.or(source.held_at) {
        instrs.remove(binding);
    }
    instrs.remove(at);
    instrs.remove(at - 1);
}

/// Where a read call takes the read-temporary slot the dying copy is bound
/// to.
#[derive(Clone, Copy)]
struct Lending {
    binding_at: usize,
    load_at: usize,
    load: Reg,
    slot: VarId,
}

/// The read-temporary slot the dying copy at `at` binds right past the run
/// of drops holding its source's death, when the source can stand in for
/// it: only the binding and one whole load name the slot, that load's one
/// use is a later instruction of the block before any region op, nothing
/// from the binding to that
/// use can raise, and nothing from the binding to the slot's last death in
/// the block names the source.
fn lending_use(
    instrs: &[MirInstr],
    at: usize,
    source: DyingCopy,
    elision: &Elision,
) -> Option<Lending> {
    let (dest, _) = copy_construction(&instrs[at])?;
    if source.binding_at.is_some() {
        return None;
    }
    let binding_at = match source.held_at {
        Some(held_at) => held_at,
        None => (source.drop_at + 1..instrs.len()).find(|&at| !is_drop(&instrs[at]))?,
    };
    let MirInstr::DefVar {
        var: slot,
        src,
        in_place: false,
        ..
    } = &instrs[binding_at]
    else {
        return None;
    };
    let slot = *slot;
    let read_temporary = elision
        .var_names
        .get(slot as usize)
        .is_some_and(|name| name.starts_with(READ_TEMPORARY_SLOT));
    if src != dest || !read_temporary || elision.mentions.get(&slot) != Some(&2) {
        return None;
    }
    let end = segment_end(instrs, binding_at + 1);
    let (load_at, load) = instrs[..end]
        .iter()
        .enumerate()
        .skip(binding_at + 1)
        .find_map(|(load_at, instruction)| match instruction {
            MirInstr::LoadPlace { dest, place } if place.root == slot => {
                (place.proj.is_empty() && place.through.is_none()).then_some((load_at, *dest))
            }
            _ => None,
        })?;
    if elision.uses.get(&load.0) != Some(&1) {
        return None;
    }
    let consumer_at = first_reader(instrs, load_at + 1..end, load)?;
    let last_death = instrs[..end]
        .iter()
        .rposition(
            |instruction| matches!(instruction, MirInstr::DropVar { var, .. } if *var == slot),
        )
        .filter(|&death| death > consumer_at)?;
    let silent = instrs[binding_at + 1..consumer_at].iter().all(cannot_raise);
    let untouched =
        !instrs[binding_at + 1..=last_death]
            .iter()
            .enumerate()
            .any(|(offset, instruction)| {
                binding_at + 1 + offset != source.drop_at && names(instruction, source.var)
            });
    (silent && untouched).then_some(Lending {
        binding_at,
        load_at,
        load,
        slot,
    })
}

/// Lend the dying source to the read call that takes the read-temporary
/// slot: the load, the construction, the source's death, and the slot's
/// binding go, and the slot's load reads the source instead. The slot's
/// deaths become the source's once the whole function is rewritten.
fn lend(instrs: &mut Vec<MirInstr>, at: usize, source: DyingCopy, lending: Lending) -> Loan {
    if let MirInstr::LoadPlace { place, .. } = &mut instrs[lending.load_at] {
        place.root = source.var;
    }
    let mut removed = [lending.binding_at, source.drop_at, at, at - 1];
    removed.sort_unstable_by(|left, right| right.cmp(left));
    for index in removed {
        instrs.remove(index);
    }
    Loan {
        slot: lending.slot,
        source: source.var,
        load: lending.load,
    }
}

/// Replace the construction at `at` with its source's move: the source's
/// death and the dead load go, and the binding is no longer in place. An
/// unbound copy a callee takes at its setup ([`taken_at_setup`]) becomes
/// the source's forwarding instead.
fn elide(instrs: &mut Vec<MirInstr>, at: usize, source: DyingCopy, elision: &Elision) {
    instrs.remove(source.drop_at);
    if let Some(MirInstr::DefVar { in_place, .. }) = source
        .binding_at
        .and_then(|binding| instrs.get_mut(binding))
    {
        *in_place = false;
    }
    let Some((&dest, _)) = copy_construction(&instrs[at]) else {
        return;
    };
    let unbound = source.binding_at.is_none() && source.held_at.is_none();
    let mode = if unbound && taken_at_setup(instrs, at, dest, source.var, elision) {
        UseMode::Forward
    } else {
        UseMode::Move
    };
    instrs[at] = MirInstr::UseVar {
        dest,
        var: source.var,
        mode,
        constructor: None,
    };
    instrs.remove(at - 1);
}

/// Delete each copy or constant of `instrs` whose one use is the
/// `drop.reg` in the run of drops right after it, with that drop: nothing
/// reads the value, so neither it nor its destruction is observable.
fn delete_dead_copies(instrs: &mut Vec<MirInstr>, uses: &HashMap<u32, usize>) {
    let mut at = 0;
    while at < instrs.len() {
        let dead = dead_value(&instrs[at])
            .filter(|dest| uses.get(&dest.0) == Some(&1))
            .and_then(|dest| {
                instrs[at + 1..]
                    .iter()
                    .take_while(|instruction| is_drop(instruction))
                    .position(|instruction| matches!(instruction, MirInstr::Drop { reg, .. } if *reg == dest))
            });
        match dead {
            Some(offset) => {
                instrs.remove(at + 1 + offset);
                instrs.remove(at);
            }
            None => at += 1,
        }
    }
}

/// The result of `instruction` when it is a value whose deletion is
/// unobservable once nothing reads it: a copy construction, a `var.use`
/// copy, or a constant.
fn dead_value(instruction: &MirInstr) -> Option<Reg> {
    match instruction {
        MirInstr::UseVar {
            dest,
            mode: UseMode::Copy,
            ..
        }
        | MirInstr::Const { dest, .. } => Some(*dest),
        _ => copy_construction(instruction).map(|(dest, _)| *dest),
    }
}

/// Turn each `var.use` copy of `instrs` whose source slot's `DropVar`
/// follows it, or its binding, in the run of drops into the slot's move,
/// dropping that `DropVar`: the slot dies right at the copy, so the copy
/// takes its value, as `optimizeCopyDestroys` turns a copy of a dying
/// value into a move. An unbound copy a callee takes at its setup
/// ([`taken_at_setup`]) becomes the slot's forwarding instead, and a copy
/// bound into a nested `def`'s environment ([`is_environment_slot`]) stays
/// one.
fn move_dying_slot_copies(instrs: &mut Vec<MirInstr>, elision: &Elision) {
    let var_names = elision.var_names;
    let mut at = 0;
    while at < instrs.len() {
        let MirInstr::UseVar {
            dest,
            var,
            mode: UseMode::Copy,
            ..
        } = instrs[at]
        else {
            at += 1;
            continue;
        };
        let environment = instrs[at + 1..]
            .iter()
            .find(|instruction| !is_drop(instruction))
            .is_some_and(|instruction| {
                matches!(instruction, MirInstr::DefVar { src, var: slot, .. }
                    if *src == dest
                        && var_names
                            .get(*slot as usize)
                            .is_some_and(|name| is_environment_slot(name)))
            });
        if environment {
            at += 1;
            continue;
        }
        let after = match instrs.get(at + 1) {
            Some(MirInstr::DefVar { src, .. }) if *src == dest => at + 1,
            _ => at,
        };
        let death = instrs[after + 1..]
            .iter()
            .take_while(|instruction| is_drop(instruction))
            .position(|instruction| matches!(instruction, MirInstr::DropVar { var: dying, .. } if *dying == var));
        if let Some(offset) = death {
            instrs.remove(after + 1 + offset);
            let mode = if after == at && taken_at_setup(instrs, at, dest, var, elision) {
                UseMode::Forward
            } else {
                UseMode::Move
            };
            instrs[at] = MirInstr::UseVar {
                dest,
                var,
                mode,
                constructor: None,
            };
        }
        at += 1;
    }
}

/// Whether the unbound copy `dest` at `at` of the dying `source` is one the
/// first instruction reading it hands its callee by storage, an owned
/// parameter's, owned receiver's, or owned operator operand's, with
/// nothing between that can raise or names `source`: upstream's lifetime
/// check then merges the copy with the source's destruction into the
/// callee taking the source's own storage, so no initializer runs.
fn taken_at_setup(
    instrs: &[MirInstr],
    at: usize,
    dest: Reg,
    source: VarId,
    elision: &Elision,
) -> bool {
    if !elision.forwarded.contains(&dest.0) || elision.uses.get(&dest.0) != Some(&1) {
        return false;
    }
    let end = segment_end(instrs, at + 1);
    first_reader(instrs, at + 1..end, dest)
        .is_some_and(|consumer_at| instrs[at + 1..consumer_at].iter().all(|i| quiet(i, source)))
}

/// The index of the first region op other than `try` at or after `from`,
/// or the block's end: lowering would end the block there.
fn segment_end(instrs: &[MirInstr], from: usize) -> usize {
    instrs
        .iter()
        .skip(from)
        .position(|instruction| {
            matches!(
                instruction,
                MirInstr::If { .. }
                    | MirInstr::Loop { .. }
                    | MirInstr::ComptimeIf { .. }
                    | MirInstr::ComptimeFor { .. }
                    | MirInstr::Match { .. }
            )
        })
        .map_or(instrs.len(), |offset| from + offset)
}

/// The index of the first instruction in `within` that reads `reg`.
fn first_reader(instrs: &[MirInstr], within: std::ops::Range<usize>, reg: Reg) -> Option<usize> {
    let mut operands = Vec::new();
    within.into_iter().find(|&at| {
        operands.clear();
        instruction_operand_regs(&instrs[at], &mut operands);
        operands.contains(&reg)
    })
}

/// Whether `instruction` can sit between a dying source's copy and the
/// call taking it: a drop, or an instruction that cannot raise, that does
/// not name `source`.
fn quiet(instruction: &MirInstr, source: VarId) -> bool {
    (is_drop(instruction) || cannot_raise(instruction)) && !names(instruction, source)
}

const fn is_drop(instruction: &MirInstr) -> bool {
    matches!(
        instruction,
        MirInstr::DropVar { .. } | MirInstr::Drop { .. } | MirInstr::DropPlace { .. }
    )
}

/// Whether `instruction` cannot raise: it holds no region and has no raise
/// edge ([`MirInstr::raise_edge`]), as drop elaboration seeds a handler
/// only at a raising instruction's error region. A `try` raises; no scan
/// reaches another region op ([`segment_end`]).
fn cannot_raise(instruction: &MirInstr) -> bool {
    instruction.raise_edge().is_none() && instruction_regions(instruction).is_empty()
}

/// Whether `instruction` reads, writes, moves, declares, or destroys `var`.
fn names(instruction: &MirInstr, var: VarId) -> bool {
    matches!(
        instruction,
        MirInstr::DropVar { var: named, .. } | MirInstr::DeclareVar { var: named } if *named == var
    ) || var_def(instruction) == Some(var)
        || vars_moved(instruction).contains(&var)
        || var_uses(instruction).iter().any(|(named, _)| *named == var)
}

/// Make every death of a lent slot its source's: its `DropVar`s, and its
/// place in a `try` body's or an exit's cleanup, where a source already
/// listed keeps its one entry.
fn rename_lent_slots(blocks: &mut [MirBlock], lent: &[Loan]) {
    if lent.is_empty() {
        return;
    }
    let sources = lent
        .iter()
        .map(|loan| (loan.slot, loan.source))
        .collect::<HashMap<_, _>>();
    rename_deaths(blocks, &sources);
}

fn rename_deaths(blocks: &mut [MirBlock], sources: &HashMap<VarId, VarId>) {
    for block in blocks {
        for instruction in &mut block.instrs {
            if let MirInstr::DropVar { var, .. } = instruction
                && let Some(&source) = sources.get(var)
            {
                *var = source;
            }
            for region in instruction_regions_mut(instruction) {
                rename_deaths(region, sources);
            }
        }
    }
}
