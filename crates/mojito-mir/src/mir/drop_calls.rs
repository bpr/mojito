//! Drops lowered to calls of their destructor.
//!
//! Upstream's destructor insertion leaves, at each point a value dies, a
//! plain call of its type's `__del__` consuming the storage, a field's
//! included, which it calls on the field's address. A `drop.var` or
//! `drop.place` names that destructor but stays a drop, which both
//! backends run under a runtime test of the slot's state: drop elaboration
//! places a drop by liveness, so on some path to it the slot may be empty
//! or partly moved. A definite-initialization state per slot, which
//! follows each region op as upstream's `CheckLifetimes` walks it (an `if`
//! joining its arms, a loop its `break`s once its `continue`s are stable,
//! a `match` case entered from the previous case's `match.next`s, a `try`'s
//! handler from the body's raises), decides where the state is
//! definite. Where the slot holds a whole value on every path to a
//! `drop.var`, the drop is the call: a forwarding use of the slot, which
//! ends it and hands its storage to the callee as Mojo's owned argument
//! convention does, and a call of the named destructor on it as the
//! `deinit self` receiver, which is what an explicit `x^.finish()` lowers
//! to. Where it holds the owned part a `drop.place` names whole, the drop
//! is a load of the part, the call on it with the part as its receiver's
//! place, and a consume of the part, which is what an explicit
//! `x.f^.finish()` lowers to. Where the slot holds no value on every path
//! to a `drop.var`, nothing dies there and the drop goes. Every other drop
//! stays a drop.

use super::{
    MirBlock, MirFunction, MirInstr, MirPlace, MirTerm, Proj, RegionFrame, UseMode, VarId,
    fresh_register, instruction_regions, instruction_regions_mut, instruction_slots, region_exit,
};
use mojito_common::token::SourceSpan;
use mojito_types::types::TyKind;
use std::collections::HashMap;

/// Lower every named drop of `function` whose slot holds the dying value
/// whole on every path to it to a call of the destructor.
///
/// That is a `drop.var` whose slot holds a whole value, or a `drop.place`
/// of an owned part its slot holds whole; a `drop.var` whose slot holds no
/// value on every path to it is erased. A destructor `unconditional` does
/// not hold of, one available only where a `where` clause holds, stays a
/// drop for the elaborator to name per instance.
///
/// A lowered drop of a slot becomes a forwarding use of the slot and a
/// call of the named destructor on it; one of a part, a load of the part,
/// the call on it with the part as its receiver's place, and a consume of
/// the part, which is what an explicit `x.f^.finish()` lowers to; in the
/// function's regions too. An owned, `deinit`, `mut`, or `ref` parameter
/// holds a whole value as the function is entered, a reference's referent
/// being whole whenever the callee runs; every other slot holds one once a
/// whole store fills it, or a store fills the last part a partial move or
/// destruction left missing.
pub fn lower_whole_drops(function: &mut MirFunction, unconditional: &dyn Fn(&str) -> bool) {
    let mut blocks = std::mem::take(&mut function.blocks);
    let owned: Vec<bool> = (0..function.n_vars)
        .map(|var| {
            var < function.n_params
                && [
                    &function.owned_params,
                    &function.deinit_params,
                    &function.ref_params,
                ]
                .iter()
                .any(|params| params.get(var).copied().unwrap_or(false))
        })
        .collect();
    lower_list(
        &mut blocks,
        unconditional,
        &|var| {
            Some(if owned.get(var as usize).copied().unwrap_or(false) {
                Held::Whole
            } else {
                Held::Partial
            })
        },
        function,
    );
    function.blocks = blocks;
}

/// Lower the whole drops of the block list `list`, whose slots hold what
/// `entry` says as `list` is entered, and of its regions.
fn lower_list(
    list: &mut [MirBlock],
    unconditional: &dyn Fn(&str) -> bool,
    entry: &dyn Fn(VarId) -> State,
    function: &mut MirFunction,
) {
    let blocks: &[MirBlock] = list;
    let mut slots: Vec<VarId> = blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .flat_map(|instruction| match named_drop_root(instruction) {
            Some(var) => vec![var],
            None => named_drop_slots(instruction),
        })
        .collect();
    slots.sort_unstable();
    slots.dedup();
    // A lowered drop leaves every slot's state as the drop did, so the
    // sites and the regions' entries are decided over the list as it is.
    let mut memo = Memo::new();
    let flows: HashMap<VarId, Flow> = slots
        .iter()
        .map(|&var| {
            let flow = flow(blocks, entry(var), var, &mut Vec::new(), &mut memo);
            (var, flow)
        })
        .collect();
    let held_before = |block: usize, index: usize, var: VarId| -> State {
        flows
            .get(&var)
            .and_then(|flow| flow.before[block].get(index).cloned())
            .flatten()
    };
    let sites: Vec<DropSite> = blocks
        .iter()
        .enumerate()
        .flat_map(|(block, holder)| {
            holder
                .instrs
                .iter()
                .enumerate()
                .filter_map(move |(index, instruction)| match instruction {
                    MirInstr::DropVar {
                        var,
                        destructor: Some(destructor),
                    } => Some((block, index, *var, None, destructor.clone())),
                    MirInstr::DropPlace {
                        place,
                        destructor: Some(destructor),
                    } => owned_part(place).map(|path| {
                        (
                            block,
                            index,
                            place.root,
                            Some((place.clone(), path)),
                            destructor.clone(),
                        )
                    }),
                    _ => None,
                })
        })
        .filter(|(.., destructor)| unconditional(destructor))
        .filter_map(|(block, index, var, part, destructor)| {
            let held = held_before(block, index, var)?;
            let definite = match &part {
                None => matches!(held, Held::Whole | Held::Empty),
                Some((_, path)) => holds_part(&held, path),
            };
            definite.then(|| DropSite {
                block,
                index,
                var,
                place: part.map(|(place, _)| place),
                destructor,
                held,
            })
        })
        .collect();
    let mut region_entries = Vec::new();
    for (block, holder) in blocks.iter().enumerate() {
        for (index, instruction) in holder.instrs.iter().enumerate() {
            let regions = instruction_regions(instruction).len();
            if regions == 0 {
                continue;
            }
            let mut entries = vec![HashMap::new(); regions];
            for var in named_drop_slots(instruction) {
                let states = memoized_region_flow(
                    instruction,
                    held_before(block, index, var),
                    var,
                    &mut Vec::new(),
                    &mut memo,
                )
                .entries;
                for (entry, state) in entries.iter_mut().zip(states) {
                    entry.insert(var, state);
                }
            }
            region_entries.push((block, index, entries));
        }
    }
    for (block, index, entries) in region_entries {
        for (region, entry) in instruction_regions_mut(&mut list[block].instrs[index])
            .into_iter()
            .zip(entries)
        {
            lower_list(
                region,
                unconditional,
                &|var| entry.get(&var).cloned().flatten(),
                function,
            );
        }
    }
    for site in sites.iter().rev() {
        let DropSite {
            block,
            index,
            var,
            ref place,
            ref destructor,
            ref held,
        } = *site;
        if place.is_none() && *held == Held::Empty {
            list[block].instrs.remove(index);
            continue;
        }
        let span = death_span(function, var);
        let ty = match place {
            Some(place) => place.ty.clone(),
            None => function.var_tys.get(&var).cloned(),
        };
        let Some(ty) = ty else {
            continue;
        };
        let recv = fresh_register(function, ty, span.as_ref());
        let dest = fresh_register(function, TyKind::None.into(), span.as_ref());
        let call = MirInstr::MethodCall {
            dest,
            recv,
            method: "__deinit__".to_string(),
            resolved: Some(destructor.clone()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            recv_place: place.clone().map(Box::new),
            recv_writes: true,
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
        };
        let lowered = match place {
            Some(place) => vec![
                MirInstr::LoadPlace {
                    dest: recv,
                    place: place.clone(),
                },
                call,
                MirInstr::ConsumePlace {
                    place: place.clone(),
                    marker: recv,
                },
            ],
            None => vec![
                MirInstr::UseVar {
                    dest: recv,
                    var,
                    mode: UseMode::Forward,
                    constructor: None,
                },
                call,
            ],
        };
        list[block].instrs.splice(index..=index, lowered);
    }
}

/// What a slot holds at a point, as every path to it agrees; `None` where
/// no path reaches it.
type State = Option<Held>;

/// The definite initialization state of one slot, upstream's per-slot
/// lifetime state: whole, whole but for some owned parts, or anything else.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Held {
    Whole,
    /// A whole value but for the owned parts at these paths, each moved out
    /// or destroyed, none inside another.
    Missing(Vec<Vec<Part>>),
    /// No value: declared and not yet defined, moved out, or ended.
    Empty,
    /// Possibly empty, or partly so in a way the paths do not track.
    Partial,
}

/// One step of the path to an owned part of a slot: a field, or an
/// element of compiler-private tuple storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Part {
    Field(String),
    Element(usize),
}

/// A named drop of a block list whose slot's state is definite: the
/// instruction `index` of `block`, ending `var`, or the owned part `place`
/// of it, through `destructor`.
struct DropSite {
    block: usize,
    index: usize,
    var: VarId,
    place: Option<MirPlace>,
    destructor: String,
    held: Held,
}

/// The states one block list leaves a slot in: before each instruction of
/// each block and after its last, at the points the list completes (a
/// region's `yield`, `comptime.yield`, `try.yield`, or elif condition), at
/// the points it raises from, and joined over every point, its regions
/// included.
struct Flow {
    before: Vec<Vec<State>>,
    yielded: State,
    raised: State,
    anywhere: State,
}

/// The states a region op leaves a slot in: on entry to each of its
/// regions, in the order [`instruction_regions`] lists them, after the op,
/// at the points it raises from, and joined over every point inside it.
#[derive(Clone)]
struct RegionFlow {
    entries: Vec<State>,
    after: State,
    raised: State,
    anywhere: State,
}

/// A region op the region terminators inside it leave to, upstream's break
/// and continue sets of a loop and the next and complete sets of a `match`
/// case: the states joined at each of its two exits.
struct Frame {
    kind: RegionFrame,
    /// A loop's break set; a `match` case's complete set.
    leave: State,
    /// A loop's continue set; a `match` case's next set.
    resume: State,
}

impl Frame {
    const fn new(kind: RegionFrame) -> Self {
        Self {
            kind,
            leave: None,
            resume: None,
        }
    }
}

/// Whether `instruction` names the slot `var`, in its regions too.
fn names_slot(instruction: &MirInstr, var: VarId) -> bool {
    instruction_slots(instruction).contains(&var)
        || instruction_regions(instruction)
            .into_iter()
            .flatten()
            .flat_map(|block| &block.instrs)
            .any(|instruction| names_slot(instruction, var))
}

/// The slots the named drops inside `instruction`'s regions end, whole or
/// in an owned part.
fn named_drop_slots(instruction: &MirInstr) -> Vec<VarId> {
    let mut slots = Vec::new();
    for instruction in instruction_regions(instruction)
        .into_iter()
        .flatten()
        .flat_map(|block| &block.instrs)
    {
        slots.extend(named_drop_root(instruction));
        slots.extend(named_drop_slots(instruction));
    }
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// The slot a named drop ends whole, or in the owned part it names.
fn named_drop_root(instruction: &MirInstr) -> Option<VarId> {
    match instruction {
        MirInstr::DropVar {
            var,
            destructor: Some(_),
        } => Some(*var),
        MirInstr::DropPlace {
            place,
            destructor: Some(_),
        } if owned_part(place).is_some() => Some(place.root),
        _ => None,
    }
}

/// Whether a slot in `held` holds the owned part at `path` whole: the slot
/// is whole, or no missing part overlaps it.
fn holds_part(held: &Held, path: &[Part]) -> bool {
    match held {
        Held::Whole => true,
        Held::Missing(paths) => !paths
            .iter()
            .any(|missing| missing.starts_with(path) || path.starts_with(missing)),
        Held::Empty | Held::Partial => false,
    }
}

/// Where the slot `var` dies: the location of its last read, which a drop
/// of it follows.
fn death_span(function: &MirFunction, var: VarId) -> Option<SourceSpan> {
    function
        .spans
        .0
        .iter()
        .filter(|(_, (_, origin))| *origin == Some(var))
        .max_by_key(|(reg, _)| **reg)
        .map(|(_, (span, _))| span.clone())
}

/// The state of the slot `var` through the block list `list`, its one
/// block entered in `entry`, under the enclosing `frames`: a block ending in
/// a region terminator, or a raise naming a `try`, joins its end state into
/// the frame it names.
fn flow(
    list: &[MirBlock],
    entry: State,
    var: VarId,
    frames: &mut [Frame],
    memo: &mut Memo,
) -> Flow {
    let mut before = Vec::with_capacity(list.len());
    let mut yielded = None;
    let mut raised = None;
    let mut anywhere = None;
    let inputs = std::iter::once(entry).chain(std::iter::repeat(None));
    for (block, input) in list.iter().zip(inputs) {
        let (states, raising, inside) = through(block, input, var, frames, memo);
        raised = join(raised, raising);
        let end = states.last().cloned().flatten();
        let exit = match block.term {
            MirTerm::Raise {
                label: Some(label), ..
            } => Some((RegionFrame::Try(label), false)),
            ref term => region_exit(term),
        };
        if let Some((target, resume)) = exit {
            if let Some(frame) = frames
                .iter_mut()
                .rev()
                .find(|frame| target.names(frame.kind))
            {
                let set = if resume {
                    &mut frame.resume
                } else {
                    &mut frame.leave
                };
                *set = join(set.take(), end);
            }
        } else if matches!(
            block.term,
            MirTerm::TryYield(_)
                | MirTerm::Yield(_)
                | MirTerm::ComptimeYield(_)
                | MirTerm::ElifCond { .. }
        ) {
            yielded = join(yielded, end);
        } else if matches!(block.term, MirTerm::Raise { label: None, .. }) {
            raised = join(raised, end);
        }
        anywhere = join(anywhere, inside);
        before.push(states);
    }
    Flow {
        before,
        yielded,
        raised,
        anywhere,
    }
}

/// The state of the slot `var` before each instruction of `block` and
/// after its last, entered in `input`; joined over the points the block
/// raises out of the function from; and joined over every point of the
/// block, its regions included. A region op raises as its regions do, a
/// raising call through its error region's `raise`.
fn through(
    block: &MirBlock,
    input: State,
    var: VarId,
    frames: &mut [Frame],
    memo: &mut Memo,
) -> (Vec<State>, State, State) {
    let mut anywhere = input.clone();
    let mut raised = None;
    let mut states = vec![input];
    for instruction in &block.instrs {
        let state = states.last().cloned().flatten();
        let (after, inside) = match state {
            None => (None, None),
            Some(_) if !instruction_regions(instruction).is_empty() => {
                let region = memoized_region_flow(instruction, state, var, frames, memo);
                raised = join(raised, region.raised);
                (region.after, region.anywhere)
            }
            Some(held) => (Some(held_after(held, instruction, var)), None),
        };
        anywhere = join(join(anywhere, inside), after.clone());
        states.push(after);
    }
    (states, raised, anywhere)
}

/// The region flows one block list's walk computed, by op (its address in
/// the list, which the walk does not change), slot, entry state, and the
/// depth of the frames around it, each beside what it left in those frames.
type Memo = HashMap<(*const MirInstr, VarId, State, usize), (RegionFlow, Vec<(State, State)>)>;

/// [`region_flow`], computed once per op, slot, and entry state of one
/// list's walk: a loop's fixpoint walks its body again, and each walk
/// reaches the regions nested in it, so without it the walks multiply with
/// each loop of the nest. What the op leaves in the enclosing frames is
/// joined into them on each visit, which is what walking it again would
/// join.
fn memoized_region_flow(
    instruction: &MirInstr,
    state: State,
    var: VarId,
    frames: &mut [Frame],
    memo: &mut Memo,
) -> RegionFlow {
    let key = (std::ptr::from_ref(instruction), var, state, frames.len());
    let (region, left) = if let Some(known) = memo.get(&key) {
        known.clone()
    } else {
        let mut own: Vec<Frame> = frames.iter().map(|frame| Frame::new(frame.kind)).collect();
        let region = region_flow(instruction, key.2.clone(), var, &mut own, memo);
        let left: Vec<(State, State)> = own
            .into_iter()
            .map(|frame| (frame.leave, frame.resume))
            .collect();
        memo.insert(key, (region.clone(), left.clone()));
        (region, left)
    };
    for (frame, (leave, resume)) in frames.iter_mut().zip(left) {
        frame.leave = join(frame.leave.take(), leave);
        frame.resume = join(frame.resume.take(), resume);
    }
    region
}

/// The state of the slot `var` through the region op `instruction`,
/// entered in `state`, under the enclosing `frames`, as upstream's
/// `CheckLifetimes` walks each op.
///
/// - An `if` enters its then arm and its first condition as the op is,
///   each arm after a condition and the next condition (or the else arm)
///   as that condition yields, and completes as its arms' yields join.
/// - A `comptime if` enters every arm and its else arm as the op is, and
///   completes as their yields join.
/// - A loop or `comptime for` enters its body as the op is, joined with
///   the states its `continue`s carry, until that join is stable, and
///   completes as its `break`s join; a `comptime for`'s else region is
///   never entered.
/// - A `match` enters its first case as the op is, each later case and the
///   else region as the previous case's `match.next`s join, and completes
///   as its `match.complete`s and the else region's yield join.
/// - A `try`'s body is entered as the op is, under the frame its raises
///   name; its handler as those raises join; its `else` as the body
///   completes. The op completes as its `else`, or its body without one,
///   and its handler complete.
fn region_flow(
    instruction: &MirInstr,
    state: State,
    var: VarId,
    frames: &mut Vec<Frame>,
    memo: &mut Memo,
) -> RegionFlow {
    let mut entries = Vec::new();
    let mut after = None;
    let mut raised = None;
    let mut anywhere = None;
    let mut enter = |list: &[MirBlock], entry: State, frames: &mut Vec<Frame>, memo: &mut Memo| {
        entries.push(entry.clone());
        let flow = flow(list, entry, var, frames, memo);
        raised = join(raised.take(), flow.raised.clone());
        anywhere = join(anywhere.take(), flow.anywhere.clone());
        flow
    };
    match instruction {
        MirInstr::If {
            then,
            elifs,
            orelse,
            ..
        } => {
            after = enter(then, state.clone(), frames, memo).yielded;
            let mut rest = state;
            for elif in elifs {
                let cond = enter(&elif.cond, rest, frames, memo).yielded;
                let arm = enter(&elif.then, cond.clone(), frames, memo);
                after = join(after, arm.yielded);
                rest = cond;
            }
            after = join(after, enter(orelse, rest, frames, memo).yielded);
        }
        MirInstr::ComptimeIf { arms, orelse, .. } => {
            for arm in arms.iter().chain(std::iter::once(orelse)) {
                after = join(after, enter(arm, state.clone(), frames, memo).yielded);
            }
        }
        MirInstr::Loop { body, .. } | MirInstr::ComptimeFor { body, .. } => {
            let frame = match instruction {
                MirInstr::Loop { label, .. } => RegionFrame::Loop(*label),
                _ => RegionFrame::ComptimeFor,
            };
            let mut resume = state;
            let leave = loop {
                frames.push(Frame::new(frame));
                flow(body, resume.clone(), var, frames, memo);
                let looped = frames.pop().unwrap_or_else(|| Frame::new(frame));
                let next = join(resume.clone(), looped.resume);
                if next == resume {
                    break looped.leave;
                }
                resume = next;
            };
            frames.push(Frame::new(frame));
            enter(body, resume, frames, memo);
            frames.pop();
            after = leave;
            if matches!(instruction, MirInstr::ComptimeFor { .. }) {
                entries.push(None);
            }
        }
        MirInstr::Match { cases, orelse, .. } => {
            let mut next = state;
            for case in cases {
                frames.push(Frame::new(RegionFrame::MatchCase));
                enter(&case.body, next, frames, memo);
                let case = frames
                    .pop()
                    .unwrap_or_else(|| Frame::new(RegionFrame::MatchCase));
                after = join(after, case.leave);
                next = case.resume;
            }
            after = join(after, enter(orelse, next, frames, memo).yielded);
        }
        MirInstr::Try { .. } => return try_flow(instruction, state, var, frames, memo),
        _ => after = state,
    }
    RegionFlow {
        entries,
        after,
        raised,
        anywhere,
    }
}

/// [`region_flow`] of a `try`.
fn try_flow(
    instruction: &MirInstr,
    state: State,
    var: VarId,
    frames: &mut Vec<Frame>,
    memo: &mut Memo,
) -> RegionFlow {
    let MirInstr::Try {
        label,
        body,
        handler,
        orelse,
        ..
    } = instruction
    else {
        return RegionFlow {
            entries: Vec::new(),
            after: state,
            raised: None,
            anywhere: None,
        };
    };
    let frame = RegionFrame::Try(*label);
    frames.push(Frame::new(frame));
    let body = flow(body, state.clone(), var, frames, memo);
    let caught = frames.pop().unwrap_or_else(|| Frame::new(frame)).leave;
    let mut entries = vec![state];
    let mut raised = body.raised;
    let mut anywhere = body.anywhere;
    let handled = handler.as_ref().and_then(|(error_var, blocks)| {
        let caught = if *error_var == Some(var) {
            caught.as_ref().map(|_| Held::Whole)
        } else {
            caught
        };
        entries.push(caught.clone());
        let handler = flow(blocks, caught, var, frames, memo);
        raised = join(raised.take(), handler.raised);
        anywhere = join(anywhere.take(), handler.anywhere);
        handler.yielded
    });
    let completed = match orelse {
        Some(blocks) => {
            entries.push(body.yielded.clone());
            let orelse = flow(blocks, body.yielded, var, frames, memo);
            raised = join(raised.take(), orelse.raised);
            anywhere = join(anywhere.take(), orelse.anywhere);
            orelse.yielded
        }
        None => body.yielded,
    };
    RegionFlow {
        entries,
        after: join(completed, handled),
        raised,
        anywhere,
    }
}

/// The state of the slot `var` after the instruction `instruction`, which
/// holds no region, given `held` before it.
///
/// A whole store fills the slot, and so do a mark of it initialized and
/// the iterator a `for` loop makes in a slot of its own; a declaration, a
/// drop, an end, a move of the whole, a consuming `__iter__`, or a mark of
/// it destroyed empties it;
/// a move out of an owned part, a destruction of one, or an end of one
/// marks that part missing, an end of one subsuming the parts inside it
/// already missing, and a store into it, or a mark of it
/// initialized, fills it again; a read, a borrow, a store into a part
/// present, or an advance of the iterator keeps the state; anything else
/// naming the slot, or naming it empty, leaves it partial; a lifetime
/// marker names it without touching it. Compiler-private iterator storage
/// a `for` loop normalizes in place holds a cursor from then on, which the
/// slot's destructor does not destroy.
fn held_after(held: Held, instruction: &MirInstr, var: VarId) -> Held {
    match instruction {
        MirInstr::DefVar { var: slot, .. } if *slot == var => Held::Whole,
        MirInstr::GetIter { source, dest, .. } if *source == var && *dest == var => Held::Partial,
        MirInstr::GetIter { dest, .. } if *dest == var => Held::Whole,
        MirInstr::GetIter { .. } if instruction.consumed_iteration_source() == Some(var) => {
            Held::Empty
        }
        MirInstr::DeclareVar { var: slot }
        | MirInstr::DropVar { var: slot, .. }
        | MirInstr::ConsumeVar { var: slot }
            if *slot == var =>
        {
            Held::Empty
        }
        MirInstr::UseVar {
            var: slot, mode, ..
        } if *slot == var => {
            if mode.moves() {
                Held::Empty
            } else {
                held
            }
        }
        MirInstr::MarkInitialized { place } | MirInstr::MarkDestroyed { place }
            if place.root == var && place.proj.is_empty() && place.through.is_none() =>
        {
            if matches!(instruction, MirInstr::MarkInitialized { .. }) {
                Held::Whole
            } else {
                Held::Empty
            }
        }
        _ if held == Held::Empty && names_slot(instruction, var) => Held::Partial,
        MirInstr::MakeClosure { captures, .. } => {
            if captures.iter().all(|capture| {
                capture.mode == super::MirCaptureMode::Reference || capture.place.root != var
            }) {
                held
            } else {
                Held::Partial
            }
        }
        MirInstr::MovePlace { place, .. } | MirInstr::DropPlace { place, .. }
            if place.root == var =>
        {
            owned_part(place).map_or(Held::Partial, |path| without(held, path))
        }
        MirInstr::ConsumePlace { place, .. } | MirInstr::MarkDestroyed { place }
            if place.root == var =>
        {
            owned_part(place).map_or(Held::Partial, |path| ended(held, path))
        }
        MirInstr::MarkInitialized { place } | MirInstr::Store { place, .. }
            if place.root == var =>
        {
            match owned_part(place) {
                Some(path) => restored(held, &path),
                None => held,
            }
        }
        MirInstr::LoadPlace { .. }
        | MirInstr::Store { .. }
        | MirInstr::StoreRef { .. }
        | MirInstr::MakeRef { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::InvalidateInteriors { .. }
        | MirInstr::EstablishLoans { .. }
        | MirInstr::VariantSet { .. }
        | MirInstr::VariantSetInitWith { .. }
        | MirInstr::VariantReplace { .. }
        | MirInstr::Call { .. }
        | MirInstr::CallIndirect { .. }
        | MirInstr::MethodCall { .. }
        | MirInstr::Index { .. }
        | MirInstr::Slice { .. }
        | MirInstr::MultiIndex { .. }
        | MirInstr::MultiSet { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::HasNext { .. }
        | MirInstr::Next { .. }
        | MirInstr::TryNext { .. } => held,
        _ if names_slot(instruction, var) => Held::Partial,
        _ => held,
    }
}

/// The meet of two states: what both agree the slot holds.
fn join(left: State, right: State) -> State {
    match (left, right) {
        (None, state) | (state, None) => state,
        (Some(left), Some(right)) if left == right => Some(left),
        _ => Some(Held::Partial),
    }
}

/// The path of the owned part `place` names directly in its root slot:
/// fields and tuple elements, at least one, and no reference on the way.
fn owned_part(place: &MirPlace) -> Option<Vec<Part>> {
    if place.through.is_some() || place.proj.is_empty() {
        return None;
    }
    place
        .proj
        .iter()
        .map(|step| match step {
            Proj::Field(name) => Some(Part::Field(name.clone())),
            Proj::ConstIndex(index) => Some(Part::Element(*index)),
            _ => None,
        })
        .collect()
}

/// `held` with the part at `path` moved out or destroyed.
fn without(held: Held, path: Vec<Part>) -> Held {
    match held {
        Held::Whole => Held::Missing(vec![path]),
        Held::Missing(mut paths)
            if !paths
                .iter()
                .any(|missing| missing.starts_with(&path) || path.starts_with(missing)) =>
        {
            paths.push(path);
            Held::Missing(paths)
        }
        _ => Held::Partial,
    }
}

/// `held` with the part at `path` ended without being destroyed: a part
/// inside it already missing is subsumed, since ending the part destroys
/// nothing.
fn ended(held: Held, path: Vec<Part>) -> Held {
    match held {
        Held::Missing(mut paths) => {
            paths.retain(|missing| !missing.starts_with(&path));
            without(Held::Missing(paths), path)
        }
        held => without(held, path),
    }
}

/// `held` with the part at `path` stored whole: every missing part inside
/// it is filled.
fn restored(held: Held, path: &[Part]) -> Held {
    match held {
        Held::Missing(mut paths) => {
            paths.retain(|missing| !missing.starts_with(path));
            if paths.is_empty() {
                Held::Whole
            } else {
                Held::Missing(paths)
            }
        }
        held => held,
    }
}
