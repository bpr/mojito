//! Copy elision at a dying source: after drop elaboration, a named-struct
//! copy construction `Name(copy=v)` whose whole owned source `v` dies right
//! at it becomes `v`'s move, as Mojo's lifetime checker turns a copy
//! initialization followed by the source's destruction into a move.
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

use super::scan::{for_each_instr_deep, may_raise, var_def, var_uses, vars_moved};
use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_operand_regs;
use mojito_mir::mir::{
    FORWARDED_ARGUMENT_SLOT, MirBlock, MirDeclarations, MirFunction, MirInstr, MirPlace,
    READ_TEMPORARY_SLOT, Reg, UseMode, instruction_regions, instruction_regions_mut,
    terminator_regs,
};
use std::collections::HashMap;

/// Rewrite every dying named-struct copy construction in `function`, `Try`
/// regions included, into a move of its source, into its source's
/// forwarding when an owned parameter takes it, or into its source's loan
/// when a read parameter takes it.
pub(super) fn elide_dying_copies(function: &mut MirFunction, declarations: &MirDeclarations) {
    let mut uses = HashMap::new();
    count_operand_uses(&function.blocks, &mut uses);
    let elision = Elision {
        var_names: &function.var_names,
        declarations,
        uses,
        mentions: count_slot_mentions(&function.blocks),
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
    declarations: &'a MirDeclarations,
    /// Operand uses of each register.
    uses: HashMap<u32, usize>,
    /// Mentions of each slot outside its `DropVar`s.
    mentions: HashMap<VarId, usize>,
}

/// A read-temporary slot whose binding a lent source replaced.
struct Loan {
    slot: VarId,
    source: VarId,
    load: Reg,
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
        let mut at = 1;
        while at < block.instrs.len() {
            let instrs = &mut block.instrs;
            let Some(source) = dying_copy(instrs, at, elision.declarations, &elision.uses) else {
                at += 1;
                continue;
            };
            if let Some(forward_at) = forwarding_use(instrs, source, elision.var_names) {
                forward(instrs, at, source, forward_at);
            } else if let Some(lending) = lending_use(instrs, at, source, elision) {
                lent.push(lend(instrs, at, source, lending));
            } else {
                elide(instrs, at, source);
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
}

/// The source slot and its death of the copy construction at `at`, when the
/// construction copies a whole slot loaded just before it and that slot's
/// `DropVar` follows it (or its in-place binding) in the run of drops.
fn dying_copy(
    instrs: &[MirInstr],
    at: usize,
    declarations: &MirDeclarations,
    uses: &HashMap<u32, usize>,
) -> Option<DyingCopy> {
    let MirInstr::Call {
        dest,
        func,
        raises: None,
        args,
        kwargs,
        ..
    } = &instrs[at]
    else {
        return None;
    };
    let [(keyword, operand)] = kwargs.as_slice() else {
        return None;
    };
    let copy_constructor = declarations.structure(&func.0).is_some()
        || matches!(
            mojito_symbol::symbol::lifecycle_constructor(&func.0),
            Some((_, "__copyinit__"))
        );
    if keyword != "copy" || !args.is_empty() || !copy_constructor {
        return None;
    }
    let MirInstr::LoadPlace {
        dest: loaded,
        place: MirPlace { root, proj, .. },
    } = &instrs[at - 1]
    else {
        return None;
    };
    if loaded != operand || !proj.is_empty() || uses.get(&operand.0) != Some(&1) {
        return None;
    }
    let var = *root;
    let binding_at = match instrs.get(at + 1) {
        Some(MirInstr::DefVar {
            src,
            in_place: true,
            ..
        }) if src == dest => Some(at + 1),
        _ => None,
    };
    let drop_at = instrs[binding_at.unwrap_or(at) + 1..]
        .iter()
        .take_while(|instruction| is_drop(instruction))
        .position(
            |instruction| matches!(instruction, MirInstr::DropVar { var: dying, .. } if *dying == var),
        )?
        + binding_at.unwrap_or(at)
        + 1;
    Some(DyingCopy {
        var,
        drop_at,
        binding_at,
    })
}

/// Where the call takes the forwarding slot the dying copy is bound to: the
/// slot's one forwarding use later in the block, before any region op, when
/// nothing past the
/// source's death up to the call that consumes the slot can raise, names
/// the source, or names the slot.
fn forwarding_use(instrs: &[MirInstr], source: DyingCopy, var_names: &[String]) -> Option<usize> {
    let MirInstr::DefVar { var: slot, .. } = instrs[source.binding_at?] else {
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
    let mut operands = Vec::new();
    let consumer_at = (forward_at + 1..end).find(|&at| {
        operands.clear();
        instruction_operand_regs(&instrs[at], &mut operands);
        operands.contains(&forwarded)
    })?;
    (source.drop_at + 1..consumer_at)
        .all(|at| {
            let instruction = &instrs[at];
            (is_drop(instruction) || cannot_raise(instruction))
                && !names(instruction, source.var)
                && (at == forward_at || !names(instruction, slot))
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
    if let Some(binding) = source.binding_at {
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
    let MirInstr::Call { dest, .. } = &instrs[at] else {
        return None;
    };
    if source.binding_at.is_some() {
        return None;
    }
    let binding_at = (source.drop_at + 1..instrs.len()).find(|&at| !is_drop(&instrs[at]))?;
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
    let mut operands = Vec::new();
    let consumer_at = (load_at + 1..end).find(|&at| {
        operands.clear();
        instruction_operand_regs(&instrs[at], &mut operands);
        operands.contains(&load)
    })?;
    let last_death = instrs[..end]
        .iter()
        .rposition(
            |instruction| matches!(instruction, MirInstr::DropVar { var, .. } if *var == slot),
        )
        .filter(|&death| death > consumer_at)?;
    let silent = instrs[binding_at + 1..consumer_at].iter().all(cannot_raise);
    let untouched = !instrs[binding_at + 1..=last_death]
        .iter()
        .any(|instruction| names(instruction, source.var));
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
    instrs.remove(lending.binding_at);
    instrs.remove(source.drop_at);
    instrs.remove(at);
    instrs.remove(at - 1);
    Loan {
        slot: lending.slot,
        source: source.var,
        load: lending.load,
    }
}

/// Replace the construction at `at` with its source's move: the source's
/// death and the dead load go, and the binding is no longer in place.
fn elide(instrs: &mut Vec<MirInstr>, at: usize, source: DyingCopy) {
    instrs.remove(source.drop_at);
    if let Some(MirInstr::DefVar { in_place, .. }) = source
        .binding_at
        .and_then(|binding| instrs.get_mut(binding))
    {
        *in_place = false;
    }
    let MirInstr::Call { dest, .. } = instrs[at] else {
        return;
    };
    instrs[at] = MirInstr::UseVar {
        dest,
        var: source.var,
        mode: UseMode::Move,
        constructor: None,
    };
    instrs.remove(at - 1);
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

const fn is_drop(instruction: &MirInstr) -> bool {
    matches!(
        instruction,
        MirInstr::DropVar { .. } | MirInstr::Drop { .. } | MirInstr::DropPlace { .. }
    )
}

/// Whether `instruction` cannot raise: a silent instruction, a read, a
/// constant, or a call whose contract does not raise.
const fn cannot_raise(instruction: &MirInstr) -> bool {
    !may_raise(instruction)
        || matches!(
            instruction,
            MirInstr::LoadPlace { .. }
                | MirInstr::Const { .. }
                | MirInstr::MaterializeLiteral { .. }
                | MirInstr::Call { raises: None, .. }
                | MirInstr::MethodCall { raises: None, .. }
                | MirInstr::CallIndirect { raises: None, .. }
        )
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
