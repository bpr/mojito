//! Interior origins, the interior half of the lifetime scan
//! (`moves.rs`'s `analyze_lifetimes`, upstream's `TrackedAndInteriorLiveness`):
//! the generations interior references carry, their invalidation by
//! container mutation, and the check of a use through a stale one.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
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
    /// The origin's root was moved away or destroyed rather than mutated: a
    /// use through the reference is a use of the uninitialized root, as
    /// upstream reports it.
    root_moved: bool,
}

/// Forward may-state for interior origins. `active[reference]` is the set of
/// generations the reference variable may carry on paths reaching this point.
/// Invalidations are retained only while their generation remains active; this
/// preserves path correlation when one branch refreshes a reference after
/// invalidating it and another branch keeps the old, still-valid reference.
/// Mojo interior origins are invalidation generations, not exclusive loans:
/// ordinary reads and element writes may coexist with references into a
/// container, while a structural mutation makes every matching old
/// generation stale.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct InteriorState {
    active: BTreeMap<VarId, BTreeSet<u32>>,
    invalidated: BTreeMap<u32, InteriorInvalidationState>,
}

/// Every `EstablishLoans` generation under `blocks`, at any region depth,
/// that names at least one interior origin.
pub(super) fn collect_interior_generations(
    blocks: &[MirBlock],
) -> BTreeMap<u32, InteriorGeneration> {
    let mut generations = BTreeMap::new();
    for_each_instr_deep(blocks, &mut |instr| {
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
    });
    generations
}

pub(super) fn join_interior_states(
    mut left: InteriorState,
    right: &InteriorState,
) -> InteriorState {
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

pub(super) fn transfer_interior_instruction(
    state: &mut InteriorState,
    instr: &MirInstr,
    generations: &BTreeMap<u32, InteriorGeneration>,
    f: &MirFunction,
) {
    match instr {
        MirInstr::DropVar { var, .. } | MirInstr::ConsumeVar { var } => {
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
            dest, var, mode, ..
        } if mode.moves() => {
            invalidate_generations_rooted_at(state, *var, generations, || span_for_reg(f, *dest));
            remove_active_interior_reference(state, *var);
        }
        MirInstr::MovePlace { dest, place } if place.is_whole_root() => {
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
                            root_moved: false,
                        }
                    });
                }
            }
        }
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
                        root_moved: true,
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
            if invalidation.root_moved {
                return Err(OwnershipError::UseAfterMove {
                    var: var_name(f, invalidation.origin.root),
                    span: span_for_reg(f, marker),
                });
            }
            let origin = interior_origin_display(f, &invalidation.origin);
            let name = var_name(f, reference);
            // A call operand's hidden handle slot is named by its origin, as
            // the pin names it.
            return Err(OwnershipError::InvalidatedInteriorReference {
                reference: if name.starts_with("$call_ref_r") {
                    origin.clone()
                } else {
                    name
                },
                origin,
                span: span_for_reg(f, marker),
                invalidated_at: Box::new(invalidation.at.clone()),
            });
        }
    }
    Ok(())
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
                vec![value_place.as_deref().cloned()]
            } else {
                positional.push(value_place.as_deref().cloned());
                Vec::new()
            };
            add_subscript_call_places(
                &mut uses,
                call,
                receiver_place.as_deref(),
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
            base_place.as_deref(),
            &[index_place.as_deref().cloned()],
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
            object_place.as_deref(),
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
        MirInstr::DropPlace { place, .. } => add_place(&mut uses, place, Reg(0)),
        MirInstr::HasNext { dest, iter, .. } | MirInstr::Next { dest, iter, .. } => {
            uses.push((*iter, *dest));
        }
        // Call argument and receiver registers were evaluated before the
        // call-boundary invalidation. `arg_places`/`recv_place` are write-back
        // metadata, not a second read of a copied argument at call time.
        MirInstr::Call { .. } | MirInstr::CallIndirect { .. } | MirInstr::MethodCall { .. } => {}
        // Invalidating, dropping, or keeping an analytical handle alive is not
        // a read through that handle. Region ops' regions are replayed separately.
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::DefVar { .. }
        | MirInstr::DeclareVar { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::Try { .. }
        | MirInstr::If { .. }
        | MirInstr::Loop { .. }
        | MirInstr::ComptimeIf { .. }
        | MirInstr::ComptimeFor { .. }
        | MirInstr::ComptimeAssert { .. }
        | MirInstr::Match { .. }
        | MirInstr::Const { .. }
        | MirInstr::SizeOf { .. }
        | MirInstr::ParamListAddress { .. }
        | MirInstr::StoreToMem { .. }
        | MirInstr::TypeName { .. }
        | MirInstr::ConstructTypeParam { .. }
        | MirInstr::ConstructType { .. }
        | MirInstr::MaterializeLiteral { .. }
        | MirInstr::ReadRef { .. }
        | MirInstr::CopyValue { .. }
        | MirInstr::Rebind { .. }
        | MirInstr::WriteRef { .. }
        | MirInstr::UnOp { .. }
        | MirInstr::BinOp { .. }
        | MirInstr::GetField { .. }
        | MirInstr::Index { call: None, .. }
        | MirInstr::Slice { call: None, .. }
        | MirInstr::MultiIndex { call: None, .. }
        | MirInstr::MakeTuple { .. }
        | MirInstr::MakeStruct { .. }
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
        | MirInstr::MarkInitialized { .. }
        | MirInstr::MarkDestroyed { .. }
        | MirInstr::MakeSimd { .. }
        | MirInstr::SimdCast { .. }
        | MirInstr::SimdBitcast { .. }
        | MirInstr::SimdShuffle { .. }
        | MirInstr::Drop { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::GetIter { .. } => {}
    }
    uses.sort_unstable_by_key(|(reference, _)| *reference);
    uses.dedup_by_key(|(reference, _)| *reference);
    uses
}

pub(super) fn span_for_reg(f: &MirFunction, reg: Reg) -> mojito_common::token::SourceSpan {
    f.spans.0.get(&reg.0).map_or_else(
        || mojito_common::token::SourceSpan::new(None, mojito_common::token::DUMMY_SPAN),
        |(span, _)| span.clone(),
    )
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
