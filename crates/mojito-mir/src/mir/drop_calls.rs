//! Drops lowered to calls of their destructor.
//!
//! Upstream's destructor insertion leaves, at each point a value dies, a
//! plain call of its type's `__del__` consuming the storage. A `drop.var`
//! names that destructor but stays a drop, which both backends run under a
//! runtime test of the slot's state: drop elaboration places a drop by
//! liveness, so on some path to it the slot may be empty or partly moved.
//! A definite-initialization state per slot, which follows a `try`'s
//! regions as upstream's `CheckLifetimes` walks them, decides where the
//! state is definite. Where the slot holds a whole value on every path to
//! the drop, the drop is the call: a forwarding use of the slot, which ends
//! it and hands its storage to the callee as Mojo's owned argument
//! convention does, and a call of the named destructor on it as the
//! `deinit self` receiver, which is what an explicit `x^.finish()` lowers
//! to. Where it holds no value on every path, nothing dies there and the
//! drop goes. Every other drop stays a drop.

use super::{
    MirBlock, MirFunction, MirInstr, MirPlace, MirTerm, Proj, UseMode, VarId, cleanup_drops,
    fresh_register, instruction_regions, instruction_regions_mut, instruction_slots,
    region_escape_targets, terminator_targets,
};
use mojito_common::token::SourceSpan;
use mojito_types::types::TyKind;
use std::collections::HashMap;

/// Lower every named `drop.var` of `function` whose slot holds a whole
/// value on every path to it to a call of the destructor, and erase every
/// one whose slot holds no value on every path to it.
///
/// A lowered drop becomes a forwarding use of the slot and a call of the
/// named destructor on it, in the function's regions too. An owned
/// parameter holds a whole value as the function is entered; every other
/// slot holds one once a whole store fills it, or a store fills the last
/// part a partial move or destruction left missing.
pub fn lower_whole_drops(function: &mut MirFunction) {
    let mut blocks = std::mem::take(&mut function.blocks);
    let owned: Vec<bool> = (0..function.n_vars)
        .map(|var| {
            var < function.n_params && function.owned_params.get(var).copied().unwrap_or(false)
        })
        .collect();
    lower_list(
        &mut blocks,
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
fn lower_list(list: &mut [MirBlock], entry: &dyn Fn(VarId) -> State, function: &mut MirFunction) {
    let blocks: &[MirBlock] = list;
    let mut slots: Vec<VarId> = blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .flat_map(|instruction| match instruction {
            MirInstr::DropVar {
                var,
                destructor: Some(_),
            } => vec![*var],
            _ => named_drop_slots(instruction),
        })
        .collect();
    slots.sort_unstable();
    slots.dedup();
    // A lowered drop leaves every slot's state as the drop did, so the
    // sites and the regions' entries are decided over the list as it is.
    let flows: HashMap<VarId, Flow> = slots
        .iter()
        .map(|&var| (var, flow(blocks, entry(var), var)))
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
                    } => Some((block, index, *var, destructor.clone())),
                    _ => None,
                })
        })
        .filter_map(|(block, index, var, destructor)| {
            held_before(block, index, var)
                .filter(|held| matches!(held, Held::Whole | Held::Empty))
                .map(|held| DropSite {
                    block,
                    index,
                    var,
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
                let states = region_flow(instruction, held_before(block, index, var), var).entries;
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
            lower_list(region, &|var| entry.get(&var).cloned().flatten(), function);
        }
    }
    for site in sites.iter().rev() {
        let DropSite {
            block,
            index,
            var,
            ref destructor,
            ref held,
        } = *site;
        if *held == Held::Empty {
            list[block].instrs.remove(index);
            continue;
        }
        let span = death_span(function, var);
        let Some(ty) = function.var_tys.get(&var).cloned() else {
            continue;
        };
        let recv = fresh_register(function, ty, span.as_ref());
        let dest = fresh_register(function, TyKind::None.into(), span.as_ref());
        list[block].instrs.splice(
            index..=index,
            [
                MirInstr::UseVar {
                    dest: recv,
                    var,
                    mode: UseMode::Forward,
                    constructor: None,
                },
                MirInstr::MethodCall {
                    dest,
                    recv,
                    method: "__deinit__".to_string(),
                    resolved: Some(destructor.clone()),
                    raises: None,
                    reference_result: None,
                    result_adapter: None,
                    args: Vec::new(),
                    kwargs: Vec::new(),
                    recv_place: None,
                    recv_writes: true,
                    arg_places: Vec::new(),
                    kwarg_places: Vec::new(),
                    capture_accesses: Vec::new(),
                    param_arg_regs: Vec::new(),
                    param_decls: Vec::new(),
                    instantiated_args: Vec::new(),
                    spread: None,
                },
            ],
        );
    }
}

/// What a slot holds at a point, as every path to it agrees; `None` where
/// no path reaches it.
type State = Option<Held>;

/// The definite initialization state of one slot, upstream's per-slot
/// lifetime state: whole, whole but for some owned parts, or anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Field(String),
    Element(usize),
}

/// A named `drop.var` of a block list whose slot's state is definite: the
/// instruction `index` of `block`, ending `var` through `destructor`.
struct DropSite {
    block: usize,
    index: usize,
    var: VarId,
    destructor: String,
    held: Held,
}

/// The states one block list leaves a slot in: before each instruction of
/// each block and after its last, at the points the list completes (a
/// region's `yield`), and joined over every point, its regions included.
struct Flow {
    before: Vec<Vec<State>>,
    yielded: State,
    anywhere: State,
}

/// The states a region op leaves a slot in: on entry to each of its
/// regions, in the order [`instruction_regions`] lists them, after the op,
/// and joined over every point inside it.
struct RegionFlow {
    entries: Vec<State>,
    after: State,
    anywhere: State,
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

/// The slots the named drops inside `instruction`'s regions end.
fn named_drop_slots(instruction: &MirInstr) -> Vec<VarId> {
    let mut slots = Vec::new();
    for instruction in instruction_regions(instruction)
        .into_iter()
        .flatten()
        .flat_map(|block| &block.instrs)
    {
        if let MirInstr::DropVar {
            var,
            destructor: Some(_),
        } = instruction
        {
            slots.push(*var);
        }
        slots.extend(named_drop_slots(instruction));
    }
    slots.sort_unstable();
    slots.dedup();
    slots
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

/// The state of the slot `var` through the block list `list`, entered in
/// `entry`. An escape into `list` from one of its regions is taken to hold
/// no whole value.
fn flow(list: &[MirBlock], entry: State, var: VarId) -> Flow {
    let mut inputs: Vec<State> = vec![None; list.len()];
    let Some(first) = inputs.first_mut() else {
        return Flow {
            before: Vec::new(),
            yielded: entry.clone(),
            anywhere: entry,
        };
    };
    *first = entry;
    let mut pending = vec![0];
    while let Some(current) = pending.pop() {
        if inputs[current].is_none() {
            continue;
        }
        let (states, _) = through(&list[current], inputs[current].clone(), var);
        let out = states.last().cloned().flatten();
        let escapes = list[current]
            .instrs
            .iter()
            .flat_map(region_escape_targets)
            .map(|target| (target, Some(Held::Partial)));
        let successors = terminator_targets(&list[current].term)
            .into_iter()
            .map(|target| (target, out.clone()));
        for (target, state) in successors.chain(escapes) {
            if let Some(input) = inputs.get_mut(target) {
                let joined = join(input.clone(), state);
                if *input != joined {
                    *input = joined;
                    pending.push(target);
                }
            }
        }
    }
    let mut before = Vec::with_capacity(list.len());
    let mut yielded = None;
    let mut anywhere = None;
    for (block, input) in list.iter().zip(inputs) {
        let (states, inside) = through(block, input, var);
        if matches!(
            block.term,
            MirTerm::TryYield | MirTerm::Yield(_) | MirTerm::ComptimeYield(_)
        ) {
            yielded = join(yielded, states.last().cloned().flatten());
        }
        anywhere = join(anywhere, inside);
        before.push(states);
    }
    Flow {
        before,
        yielded,
        anywhere,
    }
}

/// The state of the slot `var` before each instruction of `block` and
/// after its last, entered in `input`, and joined over every point of the
/// block, its regions included.
fn through(block: &MirBlock, input: State, var: VarId) -> (Vec<State>, State) {
    let mut anywhere = input.clone();
    let mut states = vec![input];
    for instruction in &block.instrs {
        let state = states.last().cloned().flatten();
        let (after, inside) = match state {
            None => (None, None),
            Some(_) if !instruction_regions(instruction).is_empty() => {
                let region = region_flow(instruction, state, var);
                (region.after, region.anywhere)
            }
            Some(held) => (Some(held_after(held, instruction, var)), None),
        };
        anywhere = join(join(anywhere, inside), after.clone());
        states.push(after);
    }
    (states, anywhere)
}

/// The state of the slot `var` through the region op `instruction`,
/// entered in `state`.
///
/// A `try`'s body is entered as the op is; its handler as the body may
/// raise, through the raise edge's cleanup; its `else` as the body
/// completes; its `finally` on every way out of the others. The op
/// completes as its `else`, or its body without one, and its handler
/// complete, through the `finally`. Every other region op is lowered to
/// blocks before drop elaboration, so one met here holds no whole value
/// inside, nor after it if it names the slot.
fn region_flow(instruction: &MirInstr, state: State, var: VarId) -> RegionFlow {
    let regions = instruction_regions(instruction).len();
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        cleanup,
    } = instruction
    else {
        let after = state.map(|held| {
            if names_slot(instruction, var) {
                Held::Partial
            } else {
                held
            }
        });
        return RegionFlow {
            entries: vec![Some(Held::Partial); regions],
            anywhere: join(after.clone(), Some(Held::Partial)),
            after,
        };
    };
    let body = flow(body, state.clone(), var);
    let raised = if cleanup_drops(cleanup, var) {
        body.anywhere.as_ref().map(|_| Held::Partial)
    } else {
        body.anywhere.clone()
    };
    let mut entries = vec![state];
    let mut anywhere = join(body.anywhere.clone(), raised.clone());
    let handled = handler.as_ref().and_then(|(error_var, blocks)| {
        let caught = if *error_var == Some(var) {
            raised.as_ref().map(|_| Held::Whole)
        } else {
            raised
        };
        entries.push(caught.clone());
        let handler = flow(blocks, caught, var);
        anywhere = join(anywhere.clone(), handler.anywhere);
        handler.yielded
    });
    let completed = match orelse {
        Some(blocks) => {
            entries.push(body.yielded.clone());
            let orelse = flow(blocks, body.yielded, var);
            anywhere = join(anywhere.clone(), orelse.anywhere);
            orelse.yielded
        }
        None => body.yielded,
    };
    let normal = join(completed, handled);
    let after = match finalbody {
        Some(blocks) => {
            let entered = join(normal.clone(), anywhere.clone());
            entries.push(entered.clone());
            anywhere = join(anywhere, flow(blocks, entered, var).anywhere);
            flow(blocks, normal, var).yielded
        }
        None => normal,
    };
    RegionFlow {
        entries,
        after,
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
/// marks that part missing, and a store into it, or a mark of it
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
        MirInstr::MovePlace { place, .. }
        | MirInstr::DropPlace { place, .. }
        | MirInstr::ConsumePlace { place, .. }
        | MirInstr::MarkDestroyed { place }
            if place.root == var =>
        {
            owned_part(place).map_or(Held::Partial, |path| without(held, path))
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
