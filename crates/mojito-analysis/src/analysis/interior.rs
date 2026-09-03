//! Interior-origin analysis: generation states for interior references
//! and their invalidation on mutation, including try regions.

use super::*;

/// One `EstablishLoans` generation that contains at least one reference into
/// container-owned storage. The marker register is its stable identity; a
/// reference variable can name several possible markers after a CFG join.
#[derive(Clone)]
pub(super) struct InteriorGeneration {
    origins: Vec<MirInteriorOrigin>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct InteriorInvalidationState {
    at: mojito_common::token::SourceSpan,
    origin: MirInteriorOrigin,
}

/// Forward may-state for interior origins. `active[reference]` is the set of
/// generations the reference variable may carry on paths reaching this point.
/// Invalidations are retained only while their generation remains active; this
/// preserves path correlation when one branch refreshes a reference after
/// invalidating it and another branch keeps the old, still-valid reference.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct InteriorState {
    /// `false` is the dataflow bottom used when a structured region has no
    /// normal (`FallOff`) exit. It prevents `return`/outer-loop escape paths
    /// from contaminating instructions that execute after a `try`.
    reachable: bool,
    active: BTreeMap<VarId, BTreeSet<u32>>,
    invalidated: BTreeMap<u32, InteriorInvalidationState>,
}

impl Default for InteriorState {
    fn default() -> Self {
        Self {
            reachable: true,
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
        }
    }
}

impl InteriorState {
    pub(super) fn unreachable() -> Self {
        Self {
            reachable: false,
            active: BTreeMap::new(),
            invalidated: BTreeMap::new(),
        }
    }
}

/// The three ways a structured region can complete. `normal` reaches the next
/// instruction, `raises` transfers to an enclosing handler, and `exits` carries
/// `return`/outer-loop `break`/`continue` paths. Keeping these channels separate
/// is what prevents non-fallthrough invalidations from leaking after a `try`.
#[derive(Clone)]
pub(super) struct InteriorFlow {
    normal: InteriorState,
    raises: InteriorState,
    exits: InteriorState,
}

impl InteriorFlow {
    pub(super) fn unreachable() -> Self {
        Self {
            normal: InteriorState::unreachable(),
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        }
    }
}

/// Mojo interior origins are invalidation generations, not exclusive loans.
/// Ordinary reads and element writes may coexist with references into a
/// container, while a structural mutation makes every matching old generation
/// stale. This forward pass rejects a use when *any* path to it carries an
/// invalidated generation, including branch joins and loop backedges.
pub(super) fn analyze_interior_origins(f: &MirFunction) -> Result<(), OwnershipError> {
    let generations = collect_interior_generations(&f.blocks);
    if generations.is_empty() {
        return Ok(());
    }
    check_interior_region_uses(InteriorState::default(), &f.blocks, &generations, f).map(|_| ())
}

pub(super) fn collect_interior_generations(
    blocks: &[MirBlock],
) -> BTreeMap<u32, InteriorGeneration> {
    pub(super) fn collect(
        blocks: &[MirBlock],
        generations: &mut BTreeMap<u32, InteriorGeneration>,
    ) {
        for instr in blocks.iter().flat_map(|block| &block.instrs) {
            if let MirInstr::EstablishLoans { loans, marker, .. } = instr {
                let mut origins: Vec<_> = loans
                    .iter()
                    .filter_map(|loan| loan.interior.clone())
                    .collect();
                origins.sort();
                origins.dedup();
                if !origins.is_empty() {
                    generations.insert(marker.0, InteriorGeneration { origins });
                }
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                collect(body, generations);
                if let Some((_, handler)) = handler {
                    collect(handler, generations);
                }
                if let Some(orelse) = orelse {
                    collect(orelse, generations);
                }
                if let Some(finalbody) = finalbody {
                    collect(finalbody, generations);
                }
            }
        }
    }

    let mut generations = BTreeMap::new();
    collect(blocks, &mut generations);
    generations
}

pub(super) fn join_interior_states(
    mut left: InteriorState,
    right: &InteriorState,
) -> InteriorState {
    if !left.reachable {
        return right.clone();
    }
    if !right.reachable {
        return left;
    }
    for (reference, generations) in &right.active {
        left.active
            .entry(*reference)
            .or_default()
            .extend(generations);
    }
    for (generation, invalidation) in &right.invalidated {
        match left.invalidated.entry(*generation) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(invalidation.clone());
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if invalidation_precedes(invalidation, entry.get()) {
                    entry.insert(invalidation.clone());
                }
            }
        }
    }
    left
}

pub(super) fn invalidation_precedes(
    candidate: &InteriorInvalidationState,
    current: &InteriorInvalidationState,
) -> bool {
    candidate
        .at
        .source
        .cmp(&current.at.source)
        .then_with(|| candidate.at.span.cmp(&current.at.span))
        .then_with(|| candidate.origin.cmp(&current.origin))
        .is_lt()
}

pub(super) fn transfer_interior_state(
    state: &mut InteriorState,
    instrs: &[MirInstr],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) {
    for instr in instrs {
        if !state.reachable {
            break;
        }
        transfer_interior_instruction(state, instr, generations, f);
    }
}

pub(super) fn transfer_interior_instruction(
    state: &mut InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) {
    match instr {
        MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            remove_active_interior_reference(state, *var);
        }
        // Moving a variable out relocates the storage its interior
        // generations designate (a `var` argument like `dealloc(a^)`, a
        // whole-variable rebind): every generation rooted at it becomes
        // stale, so a use-after-free through a tracked pointer rejects
        // statically instead of trapping in the VM. (Whole-place loans
        // already make such moves conflict eagerly; interior-domain loans
        // are non-exclusive and need this lazy channel.)
        MirInstr::UseVar {
            dest,
            var,
            mode: mojito_mir::mir::UseMode::Move,
        } => {
            invalidate_generations_rooted_at(state, *var, generations, || span_for_reg(f, *dest));
            remove_active_interior_reference(state, *var);
        }
        MirInstr::MovePlace { dest, place } if place.proj.is_empty() => {
            invalidate_generations_rooted_at(state, place.root, generations, || {
                span_for_reg(f, *dest)
            });
        }
        MirInstr::EstablishLoans {
            reference, marker, ..
        } => {
            remove_active_interior_reference(state, *reference);
            if generations.contains_key(&marker.0) {
                state.active.insert(*reference, BTreeSet::from([marker.0]));
            }
        }
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            marker,
        } => {
            let at = span_for_reg(f, *marker);
            for (reference, active) in &state.active {
                let excepted = Some(*reference) == *except;
                for generation_id in active {
                    let Some(generation) = generations.get(generation_id) else {
                        continue;
                    };
                    let Some(origin) = generation.origins.iter().find(|origin| {
                        // A mutation through the excepted handle preserves that
                        // handle's own generation — except when it is a subtree
                        // generation, which self-invalidates on any write below
                        // its base, its own included.
                        if excepted
                            && !matches!(
                                origin.path.last(),
                                Some(mojito_types::origin::OriginSeg::Subtree)
                            )
                        {
                            return false;
                        }
                        interior_origin_invalidated_by(origin, base, *include_base_generation)
                    }) else {
                        continue;
                    };
                    state.invalidated.entry(*generation_id).or_insert_with(|| {
                        InteriorInvalidationState {
                            at: at.clone(),
                            origin: origin.clone(),
                        }
                    });
                }
            }
        }
        MirInstr::Try { .. } => {
            *state = summarize_interior_try(state.clone(), instr, generations, f).normal;
        }
        MirInstr::Raise { .. } => state.reachable = false,
        _ => {}
    }
}

/// Mark every active generation whose origin roots at `root` as invalidated:
/// the root's storage was moved away or destroyed.
pub(super) fn invalidate_generations_rooted_at(
    state: &mut InteriorState,
    root: VarId,
    generations: &BTreeMap<u32, InteriorGeneration>,
    at: impl Fn() -> mojito_common::token::SourceSpan,
) {
    for active in state.active.values() {
        for generation_id in active {
            let Some(generation) = generations.get(generation_id) else {
                continue;
            };
            let Some(origin) = generation.origins.iter().find(|origin| origin.root == root) else {
                continue;
            };
            if !state.invalidated.contains_key(generation_id) {
                state.invalidated.insert(
                    *generation_id,
                    InteriorInvalidationState {
                        at: at(),
                        origin: origin.clone(),
                    },
                );
            }
        }
    }
}

pub(super) fn remove_active_interior_reference(state: &mut InteriorState, reference: VarId) {
    let Some(removed) = state.active.remove(&reference) else {
        return;
    };
    for generation in removed {
        if !state
            .active
            .values()
            .any(|active| active.contains(&generation))
        {
            state.invalidated.remove(&generation);
        }
    }
}

pub(super) fn interior_region_states(
    entry: &InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> (Vec<Option<InteriorState>>, Vec<Option<InteriorState>>) {
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (block, body) in blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < blocks.len() {
                preds[successor].push(block);
            }
        }
    }
    let mut incoming: Vec<Option<InteriorState>> = vec![None; blocks.len()];
    let mut outgoing: Vec<Option<InteriorState>> = vec![None; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks.len() {
            let new_in = if block == 0 {
                entry.clone()
            } else {
                let mut states = preds[block]
                    .iter()
                    .filter_map(|predecessor| outgoing[*predecessor].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join_interior_states)
            };
            let mut new_out = new_in.clone();
            transfer_interior_state(&mut new_out, &blocks[block].instrs, generations, f);
            if incoming[block].as_ref() != Some(&new_in)
                || outgoing[block].as_ref() != Some(&new_out)
            {
                incoming[block] = Some(new_in);
                outgoing[block] = Some(new_out);
                changed = true;
            }
        }
    }
    (incoming, outgoing)
}

pub(super) fn add_interior_state(target: &mut InteriorState, source: &InteriorState) {
    *target = join_interior_states(target.clone(), source);
}

pub(super) fn joined_interior_flow_inputs(flow: &InteriorFlow) -> InteriorState {
    let joined = join_interior_states(flow.normal.clone(), &flow.raises);
    join_interior_states(joined, &flow.exits)
}

pub(super) fn summarize_interior_region(
    entry: InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    if blocks.is_empty() {
        return InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        };
    }
    let (incoming, _) = interior_region_states(&entry, blocks, generations, f);
    let mut flow = InteriorFlow::unreachable();
    for (block, body) in blocks.iter().enumerate() {
        let Some(mut state) = incoming[block].clone() else {
            continue;
        };
        if !state.reachable {
            continue;
        }
        for instr in &body.instrs {
            if matches!(instr, MirInstr::Try { .. }) {
                let nested = summarize_interior_try(state, instr, generations, f);
                add_interior_state(&mut flow.raises, &nested.raises);
                add_interior_state(&mut flow.exits, &nested.exits);
                state = nested.normal;
            } else {
                if interior_instruction_directly_raises(instr) {
                    add_interior_state(&mut flow.raises, &state);
                }
                transfer_interior_instruction(&mut state, instr, generations, f);
            }
            if !state.reachable {
                break;
            }
        }
        if !state.reachable {
            continue;
        }
        match body.term {
            MirTerm::FallOff => add_interior_state(&mut flow.normal, &state),
            MirTerm::Return(_) | MirTerm::ReturnWithCleanup { .. } | MirTerm::EscapeJump { .. } => {
                add_interior_state(&mut flow.exits, &state);
            }
            MirTerm::Jump(_) | MirTerm::Branch { .. } => {}
        }
    }
    flow
}

/// Replay one CFG from its fixed-point states, recursively checking every
/// nested `try` region. Effects occur after uses: borrowing through a stale
/// reference is rejected before an establishment can replace its generation.
pub(super) fn check_interior_region_uses(
    entry: InteriorState,
    blocks: &[MirBlock],
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> Result<InteriorFlow, OwnershipError> {
    let summary = summarize_interior_region(entry.clone(), blocks, generations, f);
    if blocks.is_empty() {
        return Ok(summary);
    }
    let (incoming, _) = interior_region_states(&entry, blocks, generations, f);
    for (block, body) in blocks.iter().enumerate() {
        let Some(mut state) = incoming[block].clone() else {
            continue;
        };
        if !state.reachable {
            continue;
        }
        for instr in &body.instrs {
            if matches!(instr, MirInstr::Try { .. }) {
                state = check_interior_try_uses(state, instr, generations, f)?.normal;
            } else {
                check_interior_instruction_uses(&state, instr, f)?;
                transfer_interior_instruction(&mut state, instr, generations, f);
            }
            if !state.reachable {
                break;
            }
        }
    }
    Ok(summary)
}

pub(super) fn check_interior_instruction_uses(
    state: &InteriorState,
    instr: &MirInstr,
    f: &MirFunction,
) -> Result<(), OwnershipError> {
    for (reference, marker) in interior_reference_uses(instr) {
        let Some(active) = state.active.get(&reference) else {
            continue;
        };
        if let Some(invalidation) = active
            .iter()
            .find_map(|generation| state.invalidated.get(generation))
        {
            return Err(OwnershipError::InvalidatedInteriorReference {
                reference: var_name(f, reference),
                origin: interior_origin_display(f, &invalidation.origin),
                span: span_for_reg(f, marker),
                invalidated_at: Box::new(invalidation.at.clone()),
            });
        }
    }
    Ok(())
}

pub(super) fn interior_instruction_directly_raises(instr: &MirInstr) -> bool {
    matches!(
        instr,
        MirInstr::Raise { .. }
            | MirInstr::Call {
                raises: Some(_),
                ..
            }
            | MirInstr::CallIndirect {
                raises: Some(_),
                ..
            }
            | MirInstr::MethodCall {
                raises: Some(_),
                ..
            }
            | MirInstr::Index {
                call: Some(mojito_mir::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::Slice {
                call: Some(mojito_mir::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::MultiIndex {
                call: Some(mojito_mir::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                }),
                ..
            }
            | MirInstr::MultiSet {
                call: mojito_mir::mir::MirSubscriptCall {
                    raises: Some(_),
                    ..
                },
                ..
            }
    )
}

pub(super) fn summarize_interior_try(
    entry: InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        };
    };

    let body_flow = summarize_interior_region(entry, body, generations, f);
    let mut normal = InteriorState::unreachable();
    let mut raises = InteriorState::unreachable();
    let mut exits = body_flow.exits.clone();

    if let Some(orelse) = orelse {
        let else_flow = summarize_interior_region(body_flow.normal, orelse, generations, f);
        add_interior_state(&mut normal, &else_flow.normal);
        add_interior_state(&mut raises, &else_flow.raises);
        add_interior_state(&mut exits, &else_flow.exits);
    } else {
        add_interior_state(&mut normal, &body_flow.normal);
    }

    if let Some((_, handler)) = handler {
        let handler_flow = summarize_interior_region(body_flow.raises, handler, generations, f);
        add_interior_state(&mut normal, &handler_flow.normal);
        add_interior_state(&mut raises, &handler_flow.raises);
        add_interior_state(&mut exits, &handler_flow.exits);
    } else {
        add_interior_state(&mut raises, &body_flow.raises);
    }

    apply_interior_finally(
        InteriorFlow {
            normal,
            raises,
            exits,
        },
        finalbody.as_deref(),
        generations,
        f,
    )
}

pub(super) fn check_interior_try_uses(
    entry: InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> Result<InteriorFlow, OwnershipError> {
    let MirInstr::Try {
        body,
        handler,
        orelse,
        finalbody,
        ..
    } = instr
    else {
        return Ok(InteriorFlow {
            normal: entry,
            raises: InteriorState::unreachable(),
            exits: InteriorState::unreachable(),
        });
    };

    let body_flow = check_interior_region_uses(entry, body, generations, f)?;
    let mut normal = InteriorState::unreachable();
    let mut raises = InteriorState::unreachable();
    let mut exits = body_flow.exits.clone();

    if let Some(orelse) = orelse {
        let else_flow = check_interior_region_uses(body_flow.normal, orelse, generations, f)?;
        add_interior_state(&mut normal, &else_flow.normal);
        add_interior_state(&mut raises, &else_flow.raises);
        add_interior_state(&mut exits, &else_flow.exits);
    } else {
        add_interior_state(&mut normal, &body_flow.normal);
    }

    if let Some((_, handler)) = handler {
        let handler_flow = check_interior_region_uses(body_flow.raises, handler, generations, f)?;
        add_interior_state(&mut normal, &handler_flow.normal);
        add_interior_state(&mut raises, &handler_flow.raises);
        add_interior_state(&mut exits, &handler_flow.exits);
    } else {
        add_interior_state(&mut raises, &body_flow.raises);
    }

    let flow = InteriorFlow {
        normal,
        raises,
        exits,
    };
    if let Some(finalbody) = finalbody {
        // `finally` executes for normal, exceptional, and return/escape paths;
        // checking it from their joined input catches a stale use on any one of
        // those paths. Only its normal-channel input can reach after the `try`.
        let all_inputs = joined_interior_flow_inputs(&flow);
        let _ = check_interior_region_uses(all_inputs, finalbody, generations, f)?;
    }
    Ok(apply_interior_finally(
        flow,
        finalbody.as_deref(),
        generations,
        f,
    ))
}

pub(super) fn apply_interior_finally(
    flow: InteriorFlow,
    finalbody: Option<&[MirBlock]>,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) -> InteriorFlow {
    let Some(finalbody) = finalbody else {
        return flow;
    };

    let normal_final = summarize_interior_region(flow.normal, finalbody, generations, f);
    let raising_final = summarize_interior_region(flow.raises, finalbody, generations, f);
    let exiting_final = summarize_interior_region(flow.exits, finalbody, generations, f);

    let mut raises = raising_final.normal;
    add_interior_state(&mut raises, &normal_final.raises);
    add_interior_state(&mut raises, &raising_final.raises);
    add_interior_state(&mut raises, &exiting_final.raises);

    let mut exits = exiting_final.normal;
    add_interior_state(&mut exits, &normal_final.exits);
    add_interior_state(&mut exits, &raising_final.exits);
    add_interior_state(&mut exits, &exiting_final.exits);

    InteriorFlow {
        normal: normal_final.normal,
        raises,
        exits,
    }
}

pub(super) fn interior_origin_invalidated_by(
    origin: &MirInteriorOrigin,
    base: &MirInteriorOrigin,
    include_base_generation: bool,
) -> bool {
    if origin.root != base.root {
        return false;
    }
    // A subtree generation designates its base or any descendant, so a
    // mutation anywhere at, above, or below that base stales it — including a
    // write through the subtree reference itself (the base then carries the
    // subtree tail), which is the first-write self-invalidation rule.
    let subtree = matches!(
        origin.path.last(),
        Some(mojito_types::origin::OriginSeg::Subtree)
    );
    if !subtree && base.path.len() > origin.path.len() {
        return false;
    }
    let prefix_matches = base.path.iter().zip(&origin.path).all(|(left, right)| {
        left == right
            || matches!(
                left,
                mojito_types::origin::OriginSeg::AnyIndex
                    | mojito_types::origin::OriginSeg::Subtree
            )
            || matches!(
                right,
                mojito_types::origin::OriginSeg::AnyIndex
                    | mojito_types::origin::OriginSeg::Subtree
            )
    });
    if !prefix_matches {
        return false;
    }
    subtree
        || include_base_generation
        || origin.path[base.path.len()..]
            .iter()
            .any(|segment| matches!(segment, mojito_types::origin::OriginSeg::Interior(_)))
}

/// Every reference variable semantically consumed by an instruction, paired
/// with a register carrying the source span for the diagnostic. Place roots are
/// included as well as `through`: roots can themselves be reference-bearing
/// aggregate slots, while an ordinary reference place normally names its owner
/// root and records the actual reference in `through`.
pub(super) fn interior_reference_uses(instr: &MirInstr) -> Vec<(VarId, Reg)> {
    pub(super) fn add_place(uses: &mut Vec<(VarId, Reg)>, place: &MirPlace, marker: Reg) {
        uses.push((place.root, marker));
        if let Some(reference) = place.through {
            uses.push((reference, marker));
        }
    }

    pub(super) fn add_subscript_call_places(
        uses: &mut Vec<(VarId, Reg)>,
        call: &mojito_mir::mir::MirSubscriptCall,
        receiver_place: Option<&MirPlace>,
        positional_places: &[Option<MirPlace>],
        keyword_places: &[Option<MirPlace>],
        marker: Reg,
    ) {
        if call.receiver_requires_place
            && let Some(place) = receiver_place
        {
            add_place(uses, place, marker);
        }
        for argument in &call.arguments {
            if !argument.requires_place {
                continue;
            }
            let place = match argument.source {
                mojito_checked::checked::CheckedCallArgumentSource::Positional(index) => {
                    positional_places.get(index).and_then(Option::as_ref)
                }
                mojito_checked::checked::CheckedCallArgumentSource::Keyword(index) => {
                    keyword_places.get(index).and_then(Option::as_ref)
                }
                mojito_checked::checked::CheckedCallArgumentSource::Default => None,
            };
            if let Some(place) = place {
                add_place(uses, place, marker);
            }
        }
    }

    let mut uses = Vec::new();
    match instr {
        MirInstr::EstablishLoans { loans, marker, .. } => {
            for loan in loans {
                add_place(&mut uses, &loan.place, *marker);
            }
        }
        MirInstr::MakeRef { dest, place }
        | MirInstr::MovePlace { dest, place }
        | MirInstr::LoadPlace { dest, place } => add_place(&mut uses, place, *dest),
        MirInstr::MakeClosure { dest, captures, .. } => {
            for capture in captures {
                add_place(&mut uses, &capture.place, *dest);
            }
        }
        MirInstr::UseVar { dest, var, .. } => uses.push((*var, *dest)),
        MirInstr::Store { place, src } => add_place(&mut uses, place, *src),
        MirInstr::StoreRef { place, reference } => {
            add_place(&mut uses, place, *reference);
        }
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            value,
            value_keyword,
            call,
            ..
        } => {
            let mut positional = arg_places.clone();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                Vec::new()
            };
            add_subscript_call_places(
                &mut uses,
                call,
                receiver_place.as_ref(),
                &positional,
                &keyword,
                *value,
            );
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call: Some(call),
            ..
        } => add_subscript_call_places(
            &mut uses,
            call,
            base_place.as_ref(),
            std::slice::from_ref(index_place),
            &[],
            *dest,
        ),
        MirInstr::Slice {
            dest,
            object_place,
            arg_places,
            call: Some(call),
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place,
            arg_places,
            call: Some(call),
            ..
        } => add_subscript_call_places(
            &mut uses,
            call,
            object_place.as_ref(),
            arg_places,
            &[],
            *dest,
        ),
        MirInstr::VariantSet { place, value, .. }
        | MirInstr::VariantReplace { place, value, .. } => {
            add_place(&mut uses, place, *value);
        }
        MirInstr::VariantSetInitWith { place, factory, .. } => {
            add_place(&mut uses, place, *factory);
        }
        MirInstr::ConsumePlace { place, marker } => add_place(&mut uses, place, *marker),
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            uses.push((*iter, *dest));
        }
        // Call argument and receiver registers were evaluated before the
        // call-boundary invalidation. `arg_places`/`recv_place` are write-back
        // metadata, not a second read of a copied argument at call time.
        MirInstr::Call { .. } | MirInstr::CallIndirect { .. } | MirInstr::MethodCall { .. } => {}
        // Invalidating, dropping, or keeping an analytical handle alive is not
        // a read through that handle. `Try` regions are replayed separately.
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::Try { .. }
        | MirInstr::Const { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::MaterializeLiteral { .. }
        | MirInstr::ReadRef { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::WriteRef { .. }
        | MirInstr::UnOp { .. }
        | MirInstr::BinOp { .. }
        | MirInstr::GetField { .. }
        | MirInstr::Index { call: None, .. }
        | MirInstr::Slice { call: None, .. }
        | MirInstr::MultiIndex { call: None, .. }
        | MirInstr::MakeTuple { .. }
        | MirInstr::MakeVariant { .. }
        | MirInstr::VariantIs { .. }
        | MirInstr::VariantGet { .. }
        | MirInstr::VariantTake { .. }
        | MirInstr::VariantDeinitWith { .. }
        | MirInstr::PointerStorageTake { .. }
        | MirInstr::PointerStorageDestroy { .. }
        | MirInstr::UninitStorage { .. }
        | MirInstr::UninitStorageTake { .. }
        | MirInstr::UninitStorageDestroy { .. }
        | MirInstr::MakeSimd { .. }
        | MirInstr::SimdCast { .. }
        | MirInstr::SimdShuffle { .. }
        | MirInstr::Raise { .. }
        | MirInstr::Drop { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::GetIter { .. } => {}
    }
    uses.sort_unstable_by_key(|(reference, _)| *reference);
    uses.dedup_by_key(|(reference, _)| *reference);
    uses
}

pub(super) fn span_for_reg(f: &MirFunction, reg: Reg) -> mojito_common::token::SourceSpan {
    f.spans
        .0
        .get(&reg.0)
        .map(|(span, _)| span.clone())
        .unwrap_or_else(|| {
            mojito_common::token::SourceSpan::new(None, mojito_common::token::DUMMY_SPAN)
        })
}

pub(super) fn var_name(f: &MirFunction, var: VarId) -> String {
    f.var_names
        .get(var as usize)
        .cloned()
        .unwrap_or_else(|| format!("${var}"))
}

pub(super) fn interior_origin_display(f: &MirFunction, origin: &MirInteriorOrigin) -> String {
    let mut display = var_name(f, origin.root);
    for segment in &origin.path {
        match segment {
            mojito_types::origin::OriginSeg::Field(field) => {
                display.push('.');
                display.push_str(field);
            }
            mojito_types::origin::OriginSeg::AnyIndex => display.push_str("[…]"),
            mojito_types::origin::OriginSeg::Interior(tag) => {
                display.push_str("[\"");
                display.push_str(tag);
                display.push_str("\"]");
            }
            mojito_types::origin::OriginSeg::Subtree => display.push('~'),
        }
    }
    display
}
