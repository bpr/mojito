//! Place-loan analysis: reaching loans, overlap checks, and
//! load/store access validation against active loans.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

#[derive(Clone)]
pub(super) struct Loan {
    place: MirPlace,
    mutable: bool,
    interior: Option<MirInteriorOrigin>,
    shared: bool,
    setup: bool,
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
            if !place.is_whole_root()
                && let Some(markers) = state.active.get_mut(&place.root)
            {
                markers.retain(|existing| {
                    match generation_dests
                        .get(existing)
                        .and_then(|dest| dest.as_ref())
                    {
                        Some(domain) => {
                            !projection_covers_domain(&place.storage().proj, &domain.path)
                        }
                        None => true,
                    }
                });
            }
        }
        // A definition with no following `EstablishLoans` replaces a
        // reference-bearing value with one carrying no owner dependency.
        MirInstr::DefVar { var, .. }
        | MirInstr::DropVar { var, .. }
        | MirInstr::ConsumeVar { var } => {
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
/// from `EstablishLoans` through the reference's last use, including branch
/// joins and loops. Interior-storage loans have generation semantics instead of
/// exclusive-owner semantics among themselves (the lifetime scan checks
/// those), but against a whole-place loan of the same storage they are
/// ordinary borrows: establishing one beside the other conflicts when either
/// is mutable (a `for ref` loop over a list while a view of the list lives,
/// or a view taken inside such a loop).
///
/// The check walks the structured body twice: backward for reference
/// liveness (`region_walk.rs`'s `DestructorInsertion`-shaped walk) and
/// forward for the loan generations reaching each instruction. A `try` is
/// one instruction to both: it uses every variable read anywhere in its
/// parts, and neither the loans established nor the accesses made inside it
/// are checked. Upstream has no persistent-loan check at all (argument
/// exclusivity is checked at the call), so the exemption keeps the check on
/// the side of the pin's acceptance.
pub(super) fn analyze_loans(
    f: &MirFunction,
    callees: &CalleeRefParams<'_>,
) -> Result<(), OwnershipError> {
    let mut generations: BTreeMap<u32, Vec<Loan>> = BTreeMap::new();
    let mut references = HashSet::new();
    for_each_instr_outside_try(&f.blocks, &mut |instr| {
        if let MirInstr::EstablishLoans {
            reference,
            loans: established,
            marker,
            ..
        } = instr
        {
            references.insert(*reference);
            generations.insert(
                marker.0,
                established
                    .iter()
                    .map(|loan| Loan {
                        place: loan.place.clone(),
                        mutable: loan.mutable,
                        interior: loan.interior.clone(),
                        shared: loan.shared,
                        setup: loan.setup,
                    })
                    .collect(),
            );
        }
    });
    if generations.is_empty() {
        return Ok(());
    }
    // The references live just before each instruction, in declaration
    // order, so a conflict names the same one on every run; keyed by the
    // instruction's address, which is stable while `f` is borrowed.
    let mut live_references: HashMap<*const MirInstr, Vec<VarId>> = HashMap::new();
    walk_backward(&ReferenceLiveness, &f.blocks, &mut |instr: &MirInstr,
                                                       live: &HashSet<
        VarId,
    >| {
        let mut active: Vec<VarId> = live
            .iter()
            .copied()
            .filter(|var| references.contains(var))
            .collect();
        if !active.is_empty() {
            active.sort_unstable();
            live_references.insert(std::ptr::from_ref(instr), active);
        }
    });
    let scan = LoanScan {
        dests: loan_generation_dests(f),
    };
    let mut check = |state: &LoanGenerationState, instr: &MirInstr| {
        let active = live_references
            .get(&std::ptr::from_ref(instr))
            .map_or(&[][..], Vec::as_slice);
        check_loans_at(f, callees, &generations, state, active, instr)
    };
    walk_forward(&scan, LoanGenerationState::default(), &f.blocks, &mut check).map(|_| ())
}

/// Whether establishing `new` beside the live `existing` loan is a conflict:
/// overlapping places, at most one interior-generation domain (two interior
/// loans are generation-tracked, never exclusive), at most one shared
/// pointer alias (two `Pointer(to=place)` loans coexist), neither side a
/// setup loan (a call operand awaiting its read excludes nothing), and either
/// side excluding the other's reads.
fn loans_exclusive(new: &mojito_mir::mir::MirLoan, existing: &Loan) -> bool {
    let generation_tracked = new.interior.is_some() && existing.interior.is_some();
    let shared_aliases = new.shared && existing.shared;
    !(generation_tracked || shared_aliases || new.setup || existing.setup)
        && (excludes_reads(new) || excludes_reads(existing))
        && mir_places_overlap(&new.place, &existing.place)
}

/// Whether a live loan excludes another borrow's *reads* of the same storage.
/// Only an exclusive mutable borrow does. A `Pointer(to=place)` loan is
/// shared: upstream's `Pointer` is provenance, not an exclusive borrow, so a
/// second reader — another `Pointer`, or a `ref` argument naming the place —
/// coexists with it. Writes to the owner still conflict with either kind
/// (`assets/ownership_error/pointer_to_owner_twice_then_mutation.mojo`).
fn excludes_reads(loan: &impl LoanCapability) -> bool {
    loan.mutable() && !loan.shared()
}

/// The mutability/sharing pair `excludes_reads` reads, so the established
/// (`MirLoan`) and reaching (`Loan`) forms answer it the same way.
trait LoanCapability {
    fn mutable(&self) -> bool;
    fn shared(&self) -> bool;
}

impl LoanCapability for mojito_mir::mir::MirLoan {
    fn mutable(&self) -> bool {
        self.mutable
    }

    fn shared(&self) -> bool {
        self.shared
    }
}

impl LoanCapability for Loan {
    fn mutable(&self) -> bool {
        self.mutable
    }

    fn shared(&self) -> bool {
        self.shared
    }
}

pub(super) fn mir_places_overlap(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.steps().zip(right.steps()).all(|(a, b)| {
            matches!((a, b), (Proj::Field(x), Proj::Field(y)) if x == y)
                || matches!((a, b), (Proj::Index(_), Proj::Index(_)))
                || matches!(
                    (a, b),
                    (Proj::Index(_), Proj::ConstIndex(_)) | (Proj::ConstIndex(_), Proj::Index(_))
                )
                || matches!((a, b), (Proj::ConstIndex(x), Proj::ConstIndex(y)) if x == y)
                || matches!((a, b), (Proj::Variant(x), Proj::Variant(y)) if x.may_equal(y))
        })
}

#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
pub(super) fn loan_accesses(
    f: &MirFunction,
    instr: &MirInstr,
    callees: &CalleeRefParams<'_>,
) -> Vec<(MirPlace, LoanAccess, mojito_common::token::SourceSpan)> {
    // The access a retained place at positional parameter `parameter` of
    // `callee` performs: a write where the callee's declaration writes
    // through that slot, a shared read elsewhere (`CalleeRefParams`). An
    // unknown callee (a builtin, an unresolved dispatch) stays conservatively
    // exclusive.
    let retained_access = |callee: &str, parameter: usize| -> LoanAccess {
        callees.retained_access(callee, parameter, false)
    };
    let fallback = mojito_common::token::SourceSpan::new(None, mojito_common::token::DUMMY_SPAN);
    let span_for = |reg: Reg| {
        f.spans
            .0
            .get(&reg.0)
            .map_or_else(|| fallback.clone(), |(span, _)| span.clone())
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
        MirInstr::UseVar {
            var, dest, mode, ..
        } => vec![(
            MirPlace::root(*var, None),
            if mode.moves() {
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
                vec![value_place.as_deref().cloned()]
            } else {
                positional.push(value_place.as_deref().cloned());
                Vec::new()
            };
            subscript_accesses(
                call,
                receiver_place.as_deref(),
                &positional,
                &keyword,
                *value,
            )
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
                    base_place.as_deref(),
                    &[index_place.as_deref().cloned()],
                    &[],
                    *dest,
                )
            } else {
                base_place
                    .as_deref()
                    .into_iter()
                    .chain(index_place.as_deref())
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
                subscript_accesses(call, object_place.as_deref(), arg_places, &[], *dest)
            } else {
                object_place
                    .as_deref()
                    .into_iter()
                    .chain(arg_places.iter().flatten())
                    .cloned()
                    .map(|place| (place, LoanAccess::Read, span_for(*dest)))
                    .collect()
            }
        }
        // A type parameter's initializer borrows its keyword sources
        // (`copy: Self`).
        MirInstr::ConstructTypeParam {
            dest, kwarg_places, ..
        } => kwarg_places
            .iter()
            .flatten()
            .cloned()
            .map(|place| (place, LoanAccess::Read, span_for(*dest)))
            .collect(),
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
        MirInstr::DropPlace { place, .. }
        | MirInstr::MarkInitialized { place }
        | MirInstr::MarkDestroyed { place } => {
            vec![(place.clone(), LoanAccess::Write, span_for(Reg(0)))]
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
            let intrinsic_read = matches!(
                func.0.as_str(),
                "print" | "String" | "repr" | "external_call"
            );
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
                .as_deref()
                .into_iter()
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
                Some(callee) => callees.retained_access(callee, argument, true),
                None => LoanAccess::Write,
            };
            let mut accesses = recv_place
                .as_deref()
                .into_iter()
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
        MirInstr::DropVar { var, .. } => {
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

/// Check one instruction against the loans of the references `active` live
/// before it, given the loan generations `state` reaching it: establishing a
/// loan beside an exclusive live one, or accessing a place a live loan
/// excludes, conflicts.
fn check_loans_at(
    f: &MirFunction,
    callees: &CalleeRefParams<'_>,
    generations: &BTreeMap<u32, Vec<Loan>>,
    state: &LoanGenerationState,
    active: &[VarId],
    instr: &MirInstr,
) -> Result<(), OwnershipError> {
    if let MirInstr::EstablishLoans {
        reference,
        loans: established,
        marker,
        ..
    } = instr
    {
        let span = || span_for_reg(f, *marker);
        for loan in established {
            for other in active.iter().filter(|id| **id != *reference) {
                let reaching = reaching_loans(state, generations, *other);
                if loan.place.through == Some(*other) {
                    // A reborrow derives its permission from this live
                    // reference instead of competing with it. Mutable
                    // capability still cannot be recovered from a shared
                    // source reference.
                    if loan.mutable && reaching.iter().any(|source| !source.mutable) {
                        return Err(loan_error(f, &loan.place, *other, span()));
                    }
                    continue;
                }
                if reaching
                    .iter()
                    .any(|existing| loans_exclusive(loan, existing))
                {
                    return Err(loan_error(f, &loan.place, *other, span()));
                }
            }
        }
        return Ok(());
    }
    for (place, access, span) in loan_accesses(f, instr, callees) {
        // An access through a shared pointer alias (a stable-pointer
        // dereference substitutes the owner place) never competes with
        // another shared alias of the same storage.
        let through_shared = place.through.is_some_and(|through| {
            let loans = reaching_loans(state, generations, through);
            !loans.is_empty() && loans.iter().all(|loan| loan.shared)
        });
        for reference in active {
            let reference_loans = reaching_loans(state, generations, *reference);
            if reference_loans.is_empty() {
                continue;
            }
            if place.through == Some(*reference) {
                if access == LoanAccess::Write && reference_loans.iter().any(|loan| !loan.mutable) {
                    return Err(loan_error(f, &place, *reference, span));
                }
                continue;
            }
            if reference_loans.iter().any(|loan| {
                loan.interior.is_none()
                    && !loan.setup
                    && !(loan.shared && through_shared)
                    && mir_places_overlap(&place, &loan.place)
                    && (access == LoanAccess::Write || excludes_reads(*loan))
            }) {
                return Err(loan_error(f, &place, *reference, span));
            }
        }
    }
    Ok(())
}

/// The backward liveness of reference variables the loan check reads.
struct ReferenceLiveness;

impl BackwardScan for ReferenceLiveness {
    type State = HashSet<VarId>;

    fn join(&self, state: &mut HashSet<VarId>, other: &HashSet<VarId>) {
        state.extend(other);
    }

    fn transfer(&self, live: &mut HashSet<VarId>, instr: &MirInstr) {
        if let Some(def) = loan_liveness_def(instr) {
            live.remove(&def);
        }
        live.extend(var_uses(instr).into_iter().map(|(used, _)| used));
    }

    fn enters(&self, instr: &MirInstr) -> bool {
        !matches!(instr, MirInstr::Try { .. })
    }

    fn define(&self, live: &mut HashSet<VarId>, var: VarId) {
        live.remove(&var);
    }
}

/// The loan generations reaching each instruction, which a `try` does not
/// change.
struct LoanScan {
    dests: BTreeMap<u32, Option<MirInteriorOrigin>>,
}

impl ForwardScan for LoanScan {
    type State = LoanGenerationState;

    fn join(
        &self,
        state: &LoanGenerationState,
        other: &LoanGenerationState,
    ) -> LoanGenerationState {
        join_loan_generation_states(state.clone(), other)
    }

    fn transfer(&self, state: &mut LoanGenerationState, instr: &MirInstr) {
        transfer_loan_generation(state, instr, &self.dests);
    }

    fn raise_seed(
        &self,
        state: &LoanGenerationState,
        _next: Option<&MirInstr>,
    ) -> LoanGenerationState {
        state.clone()
    }

    fn enters(&self, instr: &MirInstr) -> bool {
        !matches!(instr, MirInstr::Try { .. })
    }
}
