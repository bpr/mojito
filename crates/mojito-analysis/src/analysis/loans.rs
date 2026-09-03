//! Place-loan analysis: reaching loans, overlap checks, and
//! load/store access validation against active loans.

use super::*;

#[derive(Clone)]
pub(super) struct Loan {
    place: MirPlace,
    mutable: bool,
    interior: Option<MirInteriorOrigin>,
}

/// The loan generation(s) a reference-bearing slot may currently contain.
/// Each `EstablishLoans` marker is a reaching definition: rebinding replaces
/// the previous generation on that path, while CFG joins retain every possible
/// incoming generation.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct LoanGenerationState {
    pub(super) active: BTreeMap<VarId, BTreeSet<u32>>,
}

pub(super) fn join_loan_generation_states(
    mut left: LoanGenerationState,
    right: &LoanGenerationState,
) -> LoanGenerationState {
    for (reference, generations) in &right.active {
        left.active
            .entry(*reference)
            .or_default()
            .extend(generations);
    }
    left
}

pub(super) fn transfer_loan_generation(
    state: &mut LoanGenerationState,
    instruction: &MirInstr,
    generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
) {
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            marker,
            dest_interior,
            ..
        } => {
            let markers = state.active.entry(*reference).or_default();
            match dest_interior {
                // A root-domain generation replaces every prior generation:
                // rebinding the whole value resets its loan picture.
                None => *markers = BTreeSet::from([marker.0]),
                // An interior-domain generation replaces overlapping
                // interior domains; the root domain and disjoint sibling
                // domains keep their own generations.
                Some(domain) => {
                    markers.retain(|existing| {
                        match generation_dests
                            .get(existing)
                            .and_then(|dest| dest.as_ref())
                        {
                            Some(other) => !origin_paths_overlap(&domain.path, &other.path),
                            None => true,
                        }
                    });
                    markers.insert(marker.0);
                }
            }
        }
        // Writing through a place releases the interior-domain generations
        // it covers: the overwrite destroyed the stored aliases.
        MirInstr::Store { place, .. } | MirInstr::StoreRef { place, .. } => {
            if !place.proj.is_empty()
                && let Some(markers) = state.active.get_mut(&place.root)
            {
                markers.retain(|existing| {
                    match generation_dests
                        .get(existing)
                        .and_then(|dest| dest.as_ref())
                    {
                        Some(domain) => !projection_covers_domain(&place.proj, &domain.path),
                        None => true,
                    }
                });
            }
        }
        // A definition with no following `EstablishLoans` replaces a
        // reference-bearing value with one carrying no owner dependency.
        MirInstr::DefVar { var, .. } | MirInstr::DropVar { var } | MirInstr::ConsumeVar { var } => {
            state.active.remove(var);
        }
        _ => {}
    }
}

/// Destination domain per loan generation marker, for domain-aware
/// generation replacement and release. Collected deep through `try` regions
/// (markers are function-wide unique, so region entries are inert for
/// top-level-only walks).
pub(super) fn loan_generation_dests(f: &MirFunction) -> BTreeMap<u32, Option<MirInteriorOrigin>> {
    let mut dests = BTreeMap::new();
    for_each_instr_deep(&f.blocks, &mut |instruction| {
        if let MirInstr::EstablishLoans {
            marker,
            dest_interior,
            ..
        } = instruction
        {
            dests.insert(marker.0, dest_interior.clone());
        }
    });
    dests
}

/// Two interior paths overlap when one is a prefix of the other.
pub(super) fn origin_paths_overlap(
    left: &[mojito_types::origin::OriginSeg],
    right: &[mojito_types::origin::OriginSeg],
) -> bool {
    left.iter().zip(right.iter()).all(|(a, b)| a == b)
}

/// A store's concrete field prefix covers a loan domain when it is a prefix
/// of the domain path (`t.a = ...` covers `[a]` and everything below it).
/// Non-field projections stay conservative (no release).
pub(super) fn projection_covers_domain(
    proj: &[Proj],
    domain: &[mojito_types::origin::OriginSeg],
) -> bool {
    proj.len() <= domain.len()
        && proj.iter().zip(domain.iter()).all(|(step, segment)| {
            matches!(
                (step, segment),
                (Proj::Field(field), mojito_types::origin::OriginSeg::Field(name)) if field == name
            )
        })
}

pub(super) fn loan_generation_block_entries(f: &MirFunction) -> Vec<LoanGenerationState> {
    loan_generation_entries_over(
        &f.blocks,
        &LoanGenerationState::default(),
        &loan_generation_dests(f),
    )
}

/// Per-block incoming loan-generation states over an arbitrary block vector (a
/// function body or a `try` region mini-CFG) with an explicit entry state.
pub(super) fn loan_generation_entries_over(
    blocks: &[MirBlock],
    entry: &LoanGenerationState,
    generation_dests: &BTreeMap<u32, Option<MirInteriorOrigin>>,
) -> Vec<LoanGenerationState> {
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); blocks.len()];
    for (block, body) in blocks.iter().enumerate() {
        for successor in successors(&body.term) {
            if successor < blocks.len() {
                predecessors[successor].push(block);
            }
        }
    }
    let mut incoming: Vec<Option<LoanGenerationState>> = vec![None; blocks.len()];
    let mut outgoing: Vec<Option<LoanGenerationState>> = vec![None; blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks.len() {
            let new_in = if block == 0 || predecessors[block].is_empty() {
                entry.clone()
            } else {
                let mut states = predecessors[block]
                    .iter()
                    .filter_map(|predecessor| outgoing[*predecessor].as_ref());
                let Some(first) = states.next() else {
                    continue;
                };
                states.fold(first.clone(), join_loan_generation_states)
            };
            let mut new_out = new_in.clone();
            for instruction in &blocks[block].instrs {
                transfer_loan_generation(&mut new_out, instruction, generation_dests);
            }
            if incoming[block].as_ref() != Some(&new_in)
                || outgoing[block].as_ref() != Some(&new_out)
            {
                incoming[block] = Some(new_in);
                outgoing[block] = Some(new_out);
                changed = true;
            }
        }
    }
    incoming
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

pub(super) fn reaching_loans<'a>(
    state: &LoanGenerationState,
    generations: &'a BTreeMap<u32, Vec<Loan>>,
    reference: VarId,
) -> Vec<&'a Loan> {
    state
        .active
        .get(&reference)
        .into_iter()
        .flatten()
        .filter_map(|generation| generations.get(generation))
        .flatten()
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoanAccess {
    Read,
    Write,
}

/// Persistent local-loan checking. Reference variables participate in ordinary
/// backward liveness through `MirPlace::through`, so a loan is active precisely
/// from `EstablishLoans` through the reference's last use, including CFG
/// joins/loops. Interior-storage loans have generation semantics instead of
/// exclusive-owner semantics; the forward interior-origin pass checks those.
pub(super) fn analyze_loans(
    f: &MirFunction,
    callees: &CalleeRefParams<'_>,
) -> Result<(), OwnershipError> {
    let mut generations: BTreeMap<u32, Vec<Loan>> = BTreeMap::new();
    for instr in f.blocks.iter().flat_map(|block| &block.instrs) {
        if let MirInstr::EstablishLoans {
            loans: established,
            marker,
            ..
        } = instr
        {
            generations.insert(
                marker.0,
                established
                    .iter()
                    .map(|loan| Loan {
                        place: loan.place.clone(),
                        mutable: loan.mutable,
                        interior: loan.interior.clone(),
                    })
                    .collect(),
            );
        }
    }
    if generations.is_empty() {
        return Ok(());
    }

    let nb = f.blocks.len();
    let generation_dests = loan_generation_dests(f);
    let generation_entries = loan_generation_block_entries(f);
    let mut live_in = vec![HashSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for block in (0..nb).rev() {
            let live_out = block_live_out(f, block, &live_in);
            let incoming = transfer_liveness(&f.blocks[block].instrs, live_out);
            if incoming != live_in[block] {
                live_in[block] = incoming;
                changed = true;
            }
        }
    }

    for (block, generation_entry) in generation_entries.iter().enumerate().take(nb) {
        let instrs = &f.blocks[block].instrs;
        let mut generation_state = generation_entry.clone();
        let mut live = block_live_out(f, block, &live_in);
        let mut live_before = vec![HashSet::new(); instrs.len()];
        for index in (0..instrs.len()).rev() {
            if let Some(def) = loan_liveness_def(&instrs[index]) {
                live.remove(&def);
            }
            for (used, _) in var_uses(&instrs[index]) {
                live.insert(used);
            }
            live_before[index] = live.clone();
        }

        for (index, instr) in instrs.iter().enumerate() {
            let active = &live_before[index];
            if let MirInstr::EstablishLoans {
                reference,
                loans: established,
                marker,
                ..
            } = instr
            {
                for loan in established.iter().filter(|loan| loan.interior.is_none()) {
                    for other in active.iter().filter(|id| **id != *reference) {
                        if loan.place.through == Some(*other) {
                            // A reborrow derives its permission from this live
                            // reference instead of competing with it. Mutable
                            // capability still cannot be recovered from a
                            // shared source reference.
                            if loan.mutable
                                && reaching_loans(&generation_state, &generations, *other)
                                    .iter()
                                    .any(|source| !source.mutable)
                            {
                                let span = f
                                    .spans
                                    .0
                                    .get(&marker.0)
                                    .map(|(span, _)| span.clone())
                                    .unwrap_or_else(|| {
                                        mojito_common::token::SourceSpan::new(
                                            None,
                                            mojito_common::token::DUMMY_SPAN,
                                        )
                                    });
                                return Err(loan_error(f, &loan.place, *other, span));
                            }
                            continue;
                        }
                        if reaching_loans(&generation_state, &generations, *other)
                            .iter()
                            .any(|existing| {
                                existing.interior.is_none()
                                    && (loan.mutable || existing.mutable)
                                    && mir_places_overlap(&loan.place, &existing.place)
                            })
                        {
                            let span = f
                                .spans
                                .0
                                .get(&marker.0)
                                .map(|(span, _)| span.clone())
                                .unwrap_or_else(|| {
                                    mojito_common::token::SourceSpan::new(
                                        None,
                                        mojito_common::token::DUMMY_SPAN,
                                    )
                                });
                            return Err(loan_error(f, &loan.place, *other, span));
                        }
                    }
                }
                transfer_loan_generation(&mut generation_state, instr, &generation_dests);
                continue;
            }
            for (place, access, span) in loan_accesses(f, instr, callees) {
                for reference in active {
                    let reference_loans =
                        reaching_loans(&generation_state, &generations, *reference);
                    if reference_loans.is_empty() {
                        continue;
                    }
                    if place.through == Some(*reference) {
                        if access == LoanAccess::Write
                            && reference_loans.iter().any(|loan| !loan.mutable)
                        {
                            return Err(loan_error(f, &place, *reference, span));
                        }
                        continue;
                    }
                    if reference_loans.iter().any(|loan| {
                        loan.interior.is_none()
                            && mir_places_overlap(&place, &loan.place)
                            && (access == LoanAccess::Write || loan.mutable)
                    }) {
                        return Err(loan_error(f, &place, *reference, span));
                    }
                }
            }
            transfer_loan_generation(&mut generation_state, instr, &generation_dests);
        }
    }
    Ok(())
}

pub(super) fn mir_places_overlap(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.proj.iter().zip(&right.proj).all(|(a, b)| {
            matches!((a, b), (Proj::Field(x), Proj::Field(y)) if x == y)
                || matches!((a, b), (Proj::Index(_), Proj::Index(_)))
                || matches!(
                    (a, b),
                    (Proj::Index(_), Proj::ConstIndex(_)) | (Proj::ConstIndex(_), Proj::Index(_))
                )
                || matches!((a, b), (Proj::ConstIndex(x), Proj::ConstIndex(y)) if x == y)
                || matches!((a, b), (Proj::Variant(x), Proj::Variant(y)) if x == y)
        })
}

pub(super) fn loan_accesses(
    f: &MirFunction,
    instr: &MirInstr,
    callees: &CalleeRefParams<'_>,
) -> Vec<(MirPlace, LoanAccess, mojito_common::token::SourceSpan)> {
    // The access a retained place at positional parameter `parameter` of
    // `callee` performs: a write at a declared `mut`/`ref` slot, a shared read
    // at a read-convention slot. An unknown callee (a builtin, an unresolved
    // dispatch) stays conservatively exclusive.
    let retained_access = |callee: &str, parameter: usize| -> LoanAccess {
        match callees.get(callee).and_then(|mask| mask.get(parameter)) {
            Some(false) => LoanAccess::Read,
            _ => LoanAccess::Write,
        }
    };
    let fallback = mojito_common::token::SourceSpan::new(None, mojito_common::token::DUMMY_SPAN);
    let span_for = |reg: Reg| {
        f.spans
            .0
            .get(&reg.0)
            .map(|(span, _)| span.clone())
            .unwrap_or_else(|| fallback.clone())
    };
    let captured = |access: &mojito_mir::mir::MirCaptureAccess, marker: Reg| {
        let mut place = MirPlace::root(access.root, None);
        // A concrete field prefix improves precision. Abstract indices and
        // interior-storage markers collapse to their owner prefix, which is the
        // conservative overlap relation required for call-side effects.
        for segment in &access.path {
            match segment {
                mojito_types::origin::OriginSeg::Field(field) => {
                    place.proj.push(Proj::Field(field.clone()));
                }
                mojito_types::origin::OriginSeg::AnyIndex
                | mojito_types::origin::OriginSeg::Interior(_)
                | mojito_types::origin::OriginSeg::Subtree => break,
            }
        }
        (
            place,
            if access.access == mojito_types::origin::CaptureAccess::Write {
                LoanAccess::Write
            } else {
                LoanAccess::Read
            },
            span_for(marker),
        )
    };
    let access_for_convention = |convention: Option<mojito_ast::ast::ArgConvention>| {
        if matches!(
            convention,
            Some(
                mojito_ast::ast::ArgConvention::Mut
                    | mojito_ast::ast::ArgConvention::Ref
                    | mojito_ast::ast::ArgConvention::Var
                    | mojito_ast::ast::ArgConvention::Out
                    | mojito_ast::ast::ArgConvention::Deinit
            )
        ) {
            LoanAccess::Write
        } else {
            LoanAccess::Read
        }
    };
    let subscript_accesses = |call: &mojito_mir::mir::MirSubscriptCall,
                              receiver_place: Option<&MirPlace>,
                              positional_places: &[Option<MirPlace>],
                              keyword_places: &[Option<MirPlace>],
                              marker: Reg| {
        let mut accesses = Vec::new();
        if let Some(place) = receiver_place {
            accesses.push((
                place.clone(),
                access_for_convention(call.receiver_convention),
                span_for(marker),
            ));
        }
        for argument in &call.arguments {
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
                accesses.push((
                    place.clone(),
                    access_for_convention(argument.convention),
                    span_for(marker),
                ));
            }
        }
        accesses.extend(
            call.capture_accesses
                .iter()
                .map(|access| captured(access, marker)),
        );
        accesses
    };
    match instr {
        MirInstr::UseVar { var, dest, mode } => vec![(
            MirPlace::root(*var, None),
            if matches!(mode, UseMode::Move) {
                LoanAccess::Write
            } else {
                LoanAccess::Read
            },
            span_for(*dest),
        )],
        MirInstr::DefVar { var, src, .. } => vec![(
            MirPlace::root(*var, None),
            LoanAccess::Write,
            span_for(*src),
        )],
        MirInstr::LoadPlace { dest, place } => {
            vec![(place.clone(), LoanAccess::Read, span_for(*dest))]
        }
        MirInstr::Store { place, src } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*src))]
        }
        MirInstr::StoreRef { place, reference } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*reference))]
        }
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            value,
            value_keyword,
            ..
        } => {
            let mut positional = arg_places.clone();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                Vec::new()
            };
            subscript_accesses(call, receiver_place.as_ref(), &positional, &keyword, *value)
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call,
            ..
        } => {
            if let Some(call) = call {
                subscript_accesses(
                    call,
                    base_place.as_ref(),
                    std::slice::from_ref(index_place),
                    &[],
                    *dest,
                )
            } else {
                base_place
                    .iter()
                    .chain(index_place.iter())
                    .cloned()
                    .map(|place| (place, LoanAccess::Read, span_for(*dest)))
                    .collect()
            }
        }
        MirInstr::Slice {
            dest,
            object_place,
            arg_places,
            call,
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place,
            arg_places,
            call,
            ..
        } => {
            if let Some(call) = call {
                subscript_accesses(call, object_place.as_ref(), arg_places, &[], *dest)
            } else {
                object_place
                    .iter()
                    .chain(arg_places.iter().flatten())
                    .cloned()
                    .map(|place| (place, LoanAccess::Read, span_for(*dest)))
                    .collect()
            }
        }
        MirInstr::VariantSet { place, value, .. } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*value))]
        }
        MirInstr::VariantSetInitWith { place, factory, .. } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*factory))]
        }
        MirInstr::VariantReplace { place, value, .. } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*value))]
        }
        MirInstr::MovePlace { dest, place } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*dest))]
        }
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .map(|capture| {
                (
                    capture.place.clone(),
                    if capture.mode == MirCaptureMode::Move {
                        LoanAccess::Write
                    } else {
                        LoanAccess::Read
                    },
                    span_for(*dest),
                )
            })
            .collect(),
        MirInstr::ConsumePlace { place, marker } => {
            vec![(place.clone(), LoanAccess::Write, span_for(*marker))]
        }
        MirInstr::Call {
            dest,
            func,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            // Formatting intrinsics borrow their retained caller places only
            // to keep pointer-backed values alive through `write_to`. Other
            // retained places are classified by the callee's convention at
            // that slot; a keyword place stays exclusive (its slot is not
            // known positionally here).
            let intrinsic_read = matches!(func.0.as_str(), "print" | "String" | "repr");
            let access = |parameter: Option<usize>| match parameter {
                _ if intrinsic_read => LoanAccess::Read,
                Some(parameter) => retained_access(&func.0, parameter),
                None => LoanAccess::Write,
            };
            let mut accesses = arg_places
                .iter()
                .enumerate()
                .filter_map(|(parameter, place)| {
                    place
                        .clone()
                        .map(|place| (place, access(Some(parameter)), span_for(*dest)))
                })
                .chain(
                    kwarg_places
                        .iter()
                        .flatten()
                        .cloned()
                        .map(|place| (place, access(None), span_for(*dest))),
                )
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::CallIndirect {
            dest,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut accesses = callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .cloned()
                .map(|place| (place, LoanAccess::Write, span_for(*dest)))
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::MethodCall {
            dest,
            resolved,
            recv_place,
            recv_writes,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            // A borrowed `self` receiver reads its retained place; only a
            // `mut`/mutable-`ref`/consuming receiver writes through it.
            // Retained argument places are classified by the resolved
            // callee's convention at that slot (`self` occupies slot zero);
            // keyword places stay exclusive.
            let receiver_access = if *recv_writes {
                LoanAccess::Write
            } else {
                LoanAccess::Read
            };
            let argument_access = |argument: usize| match resolved {
                Some(callee) => retained_access(callee, argument + 1),
                None => LoanAccess::Write,
            };
            let mut accesses = recv_place
                .iter()
                .cloned()
                .map(|place| (place, receiver_access, span_for(*dest)))
                .chain(
                    arg_places
                        .iter()
                        .enumerate()
                        .filter_map(|(argument, place)| {
                            place
                                .clone()
                                .map(|place| (place, argument_access(argument), span_for(*dest)))
                        }),
                )
                .chain(
                    kwarg_places
                        .iter()
                        .flatten()
                        .cloned()
                        .map(|place| (place, LoanAccess::Write, span_for(*dest))),
                )
                .collect::<Vec<_>>();
            accesses.extend(
                capture_accesses
                    .iter()
                    .map(|access| captured(access, *dest)),
            );
            accesses
        }
        MirInstr::DropVar { var } => {
            vec![(MirPlace::root(*var, None), LoanAccess::Write, fallback)]
        }
        _ => Vec::new(),
    }
}

pub(super) fn loan_error(
    f: &MirFunction,
    place: &MirPlace,
    reference: VarId,
    span: mojito_common::token::SourceSpan,
) -> OwnershipError {
    OwnershipError::LoanConflict {
        place: place_display(&f.var_names[place.root as usize], &place_path(place)),
        loan: f.var_names[reference as usize].clone(),
        span,
    }
}
