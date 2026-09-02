//! Shared MIR scans: per-instruction variable uses/defs/moves, deep
//! instruction walks, and droppable-root classification.

use super::*;

/// The variables a MIR instruction reads, each paired with a nearby result
/// register (for a diagnostic span). Covers direct reads (`UseVar`), place roots
/// (`Store`/`LoadPlace`/a `mut self` receiver — so a write *through* a moved value
/// is caught too), and the `for` iterator variable.
pub(super) fn var_uses(i: &MirInstr) -> Vec<(VarId, Reg)> {
    match i {
        MirInstr::EstablishLoans { loans, marker, .. } => loans
            .iter()
            .flat_map(|loan| place_loan_uses(&loan.place, *marker))
            .collect(),
        MirInstr::MakeRef { dest, place } => place_loan_uses(place, *dest),
        MirInstr::MakeClosure { dest, captures, .. } => captures
            .iter()
            .flat_map(|capture| place_loan_uses(&capture.place, *dest))
            .collect(),
        MirInstr::UseVar { dest, var, .. } => vec![(*var, *dest)],
        MirInstr::KeepAlive { var } => vec![(*var, Reg(0))],
        MirInstr::MovePlace { dest, place } => place_loan_uses(place, *dest),
        MirInstr::Store { place, src } => place_loan_uses(place, *src),
        MirInstr::StoreRef { place, reference } => place_loan_uses(place, *reference),
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            call,
            value,
            ..
        } => {
            let mut uses = receiver_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(value_place.iter())
                .flat_map(|place| place_loan_uses(place, *value))
                .collect::<Vec<_>>();
            uses.extend(
                call.capture_accesses
                    .iter()
                    .map(|access| (access.root, *value)),
            );
            uses
        }
        MirInstr::VariantSet { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::VariantSetInitWith { place, factory, .. } => place_loan_uses(place, *factory),
        MirInstr::VariantReplace { place, value, .. } => place_loan_uses(place, *value),
        MirInstr::LoadPlace { dest, place } => place_loan_uses(place, *dest),
        // Call arguments are evaluated into registers first, but mutable/ref
        // conventions still consume their retained places at the call
        // boundary for handle passing and write-back. Keep those slots alive
        // through the instruction. (Interior stale-use checking deliberately
        // does not count this as a second value read.)
        MirInstr::Call {
            dest,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = arg_places
                .iter()
                .flatten()
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::CallIndirect {
            dest,
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::MethodCall {
            dest,
            recv_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            ..
        } => {
            let mut uses = recv_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            uses.extend(capture_accesses.iter().map(|access| (access.root, *dest)));
            uses
        }
        MirInstr::Index {
            dest,
            base_place,
            index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .iter()
                .chain(index_place.iter())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::Slice {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            object_place: base_place,
            arg_places: index_place,
            call,
            ..
        } => {
            let mut uses = base_place
                .iter()
                .chain(index_place.iter().flatten())
                .flat_map(|place| place_loan_uses(place, *dest))
                .collect::<Vec<_>>();
            if let Some(call) = call {
                uses.extend(
                    call.capture_accesses
                        .iter()
                        .map(|access| (access.root, *dest)),
                );
            }
            uses
        }
        MirInstr::HasNext { dest, iter, .. }
        | MirInstr::Next { dest, iter, .. }
        | MirInstr::TryNext { dest, iter, .. } => {
            vec![(*iter, *dest)]
        }
        // `GetIter` normalizes its source into the iterator; count the source as a
        // use so a borrowed named source (a reference bound before the loop) stays
        // live through the read and is not dropped before the iterator is derived.
        MirInstr::GetIter { source, .. } => vec![(*source, Reg(0))],
        // A `try` reads every variable its sub-regions read: the outer liveness must
        // treat it as one big use, so a value used only inside the `try` is not
        // dropped *before* it.
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            let mut uses = Vec::new();
            let mut add = |bs: &[MirBlock]| {
                for b in bs {
                    for instr in &b.instrs {
                        uses.extend(var_uses(instr));
                    }
                }
            };
            add(body);
            if let Some((_, h)) = handler {
                add(h);
            }
            if let Some(e) = orelse {
                add(e);
            }
            if let Some(fb) = finalbody {
                add(fb);
            }
            uses
        }
        _ => Vec::new(),
    }
}

pub(super) fn place_loan_uses(place: &MirPlace, reg: Reg) -> Vec<(VarId, Reg)> {
    let mut uses = vec![(place.root, reg)];
    if let Some(reference) = place.through {
        uses.push((reference, reg));
    }
    uses
}

/// The variables **local** to a `try` region's blocks — those whose every
/// `DefVar` in the whole function lies within the region (at any nesting),
/// excluding those moved out with `^` — the body-local values to destroy when
/// the body is left (the exceptional-edge / scope-exit cleanup). A reassignment
/// is also a `DefVar`, so a variable declared outside the region and merely
/// reassigned inside it has def sites beyond the region and must survive the
/// exit; `function_defs` supplies the function-wide def counts to compare
/// against.
pub(super) fn region_cleanup_vars(
    blocks: &[MirBlock],
    function_defs: &HashMap<VarId, usize>,
) -> Vec<VarId> {
    let mut counts: HashMap<VarId, usize> = HashMap::new();
    let mut order: Vec<VarId> = Vec::new();
    let mut moved: HashSet<VarId> = HashSet::new();
    collect_region_defs(blocks, &mut counts, &mut order, &mut moved);
    order.retain(|v| !moved.contains(v) && counts.get(v) == function_defs.get(v));
    order
}

/// Count every `DefVar` write per variable across `blocks`, recursing into
/// nested `try` sub-regions, recording first-def order and `^`-moved variables.
/// Over a function's top-level blocks this yields the function-wide def counts
/// that region-locality is judged against.
pub(super) fn collect_region_defs(
    blocks: &[MirBlock],
    counts: &mut HashMap<VarId, usize>,
    order: &mut Vec<VarId>,
    moved: &mut HashSet<VarId>,
) {
    for b in blocks {
        for instr in &b.instrs {
            if let Some(v) = var_def(instr) {
                if !counts.contains_key(&v) {
                    order.push(v);
                }
                *counts.entry(v).or_insert(0) += 1;
            }
            for v in vars_moved(instr) {
                moved.insert(v);
            }
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                collect_region_defs(body, counts, order, moved);
                if let Some((_, h)) = handler {
                    collect_region_defs(h, counts, order, moved);
                }
                if let Some(e) = orelse {
                    collect_region_defs(e, counts, order, moved);
                }
                if let Some(fb) = finalbody {
                    collect_region_defs(fb, counts, order, moved);
                }
            }
        }
    }
}

/// The variable a MIR instruction writes (a `DefVar`), if any.
pub(super) fn var_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::DefVar { var, .. } => Some(*var),
        _ => None,
    }
}

/// `EstablishLoans` starts the reference's analytical live range, but it does not
/// overwrite the runtime handle already stored by `DefVar`.
pub(super) fn loan_liveness_def(i: &MirInstr) -> Option<VarId> {
    match i {
        MirInstr::EstablishLoans { reference, .. } => Some(*reference),
        _ => var_def(i),
    }
}

/// The variable transferred out by this instruction (a `^` move), if any — such a
/// variable is *not* dropped here (its value has moved to a new owner).
pub(super) fn vars_moved(i: &MirInstr) -> Vec<VarId> {
    match i {
        MirInstr::UseVar {
            var,
            mode: UseMode::Move,
            ..
        } => vec![*var],
        // A lowered explicit-destructor call already consumes the receiver
        // slot (residual fields destroyed, whole-value `__deinit__` skipped);
        // splicing an ordinary drop as well would run the destructor the
        // named destructor replaced.
        MirInstr::ConsumeVar { var } => vec![*var],
        // A declaration-time move capture transfers a whole root into the
        // closure owner. Projected captures leave a residual aggregate that
        // still needs its ordinary drop, so only whole-root moves suppress it.
        MirInstr::MakeClosure { captures, .. } => captures
            .iter()
            .filter(|capture| capture.mode == MirCaptureMode::Move && capture.place.proj.is_empty())
            .map(|capture| capture.place.root)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether executing this instruction can surface a caught
/// `RuntimeError::Raised`, transferring control to an enclosing handler.
/// Minimal allowlist of provably silent instructions; everything else — calls,
/// subscripts, `Raise`, nested `Try`, ... — counts as a potential raise.
/// Misclassifying silent→raising merely delays a drop to the `Try.cleanup`
/// backstop; raising→silent could let a handler observe a vacated slot, so the
/// allowlist stays minimal. (Non-`Raised` runtime errors abort the program, so
/// drops are unobservable on those paths.)
pub(super) fn may_raise(instr: &MirInstr) -> bool {
    !matches!(
        instr,
        MirInstr::DefVar { .. }
            | MirInstr::UseVar { .. }
            | MirInstr::DropVar { .. }
            | MirInstr::ConsumeVar { .. }
            | MirInstr::KeepAlive { .. }
    )
}

/// Visit every instruction under `blocks`, recursing into `try` sub-regions.
pub(super) fn for_each_instr_deep(blocks: &[MirBlock], visit: &mut impl FnMut(&MirInstr)) {
    for block in blocks {
        for instr in &block.instrs {
            visit(instr);
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                for_each_instr_deep(body, visit);
                if let Some((_, h)) = handler {
                    for_each_instr_deep(h, visit);
                }
                if let Some(e) = orelse {
                    for_each_instr_deep(e, visit);
                }
                if let Some(fb) = finalbody {
                    for_each_instr_deep(fb, visit);
                }
            }
        }
    }
}

/// Mutable counterpart of [`for_each_instr_deep`].
pub(super) fn for_each_instr_deep_mut(
    blocks: &mut [MirBlock],
    visit: &mut impl FnMut(&mut MirInstr),
) {
    for block in blocks {
        for instr in &mut block.instrs {
            visit(instr);
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instr
            {
                for_each_instr_deep_mut(body, visit);
                if let Some((_, h)) = handler {
                    for_each_instr_deep_mut(h, visit);
                }
                if let Some(e) = orelse {
                    for_each_instr_deep_mut(e, visit);
                }
                if let Some(fb) = finalbody {
                    for_each_instr_deep_mut(fb, visit);
                }
            }
        }
    }
}

/// Whether the value in variable `v` is dropped by *this* function: locals always;
/// a consuming `var` parameter (the caller transferred it) yes; a borrowed parameter or
/// `self` never (the caller owns a borrow; `self` is written back / would recurse).
pub(super) fn is_droppable_root(f: &MirFunction, v: VarId) -> bool {
    let vi = v as usize;
    // `self` is always caller-owned here, including generated/specialized
    // method CFGs whose stable binding slot is not in the leading parameter
    // range.  Test this before `n_params`: classifying such a receiver as a
    // local makes drop elaboration recursively destroy borrowed fields (and,
    // for pointer-backed collections, free the caller's allocation).
    if f.var_names.get(vi).is_some_and(|name| name == "self") {
        return false;
    }
    if vi < f.n_params {
        // A consuming `var` parameter is destroyed here; a `deinit` parameter is
        // *consumed* here (its teardown is rewritten to `ConsumeVar` below).
        f.owned_params.get(vi).copied().unwrap_or(false)
            || f.deinit_params.get(vi).copied().unwrap_or(false)
    } else {
        true
    }
}
