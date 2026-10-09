//! Renumbering a function's variable slots: every place root and
//! `through`, every capture access, loan, interior, and cleanup that names a
//! slot, and the span table's origins. Runtime promotion and the unrolling
//! of a `comptime for` both move slots.

use std::collections::BTreeSet;

use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_places_mut;
use mojito_mir::mir::{MirBlock, MirFunction, MirInstr, MirTerm};
use mojito_types::types::{ParamDecl, Ty};

/// Remove the `retired` slots from `function`, none of them a parameter nor
/// still addressed: every slot above one moves down past it, and its name
/// and type leave the tables.
pub(super) fn retire_slots(function: &mut MirFunction, retired: &BTreeSet<VarId>) {
    debug_assert!(
        retired
            .iter()
            .all(|&slot| slot as usize >= function.n_params),
        "a parameter slot is never retired"
    );
    let addressed = std::cell::Cell::new(false);
    let renumber = |var: VarId| {
        addressed.set(addressed.get() || retired.contains(&var));
        var - retired.range(..var).count() as VarId
    };
    renumber_slots(function, &renumber);
    debug_assert!(!addressed.get(), "a retired slot is still addressed");
    for &slot in retired.iter().rev() {
        function.var_names.remove(slot as usize);
    }
    function.var_tys = function
        .var_tys
        .iter()
        .filter(|(var, _)| !retired.contains(var))
        .map(|(var, ty)| (renumber(*var), ty.clone()))
        .collect();
    function.n_vars -= retired.len();
}

/// The slots `blocks` address, the regions they hold included.
pub(super) fn addressed_slots<'a>(
    blocks: impl IntoIterator<Item = &'a MirBlock>,
) -> BTreeSet<VarId> {
    let addressed = std::cell::RefCell::new(BTreeSet::new());
    let record = |var: VarId| {
        addressed.borrow_mut().insert(var);
        var
    };
    let mut blocks: Vec<MirBlock> = blocks.into_iter().cloned().collect();
    renumber_blocks(&mut blocks, &record);
    addressed.into_inner()
}

/// The slots a template seeds for its constructible type parameters (a
/// `Hasher` or `Defaultable` bound), each typed by its own binder, that no
/// block of `function` still addresses. Mojo gives a type parameter no
/// storage, so a concrete instance, whose every construction names its
/// type, keeps none.
pub(super) fn reification_slots(function: &MirFunction, scope: &[ParamDecl]) -> BTreeSet<VarId> {
    let addressed = addressed_slots(&function.blocks);
    scope
        .iter()
        .filter(|decl| mojito_types::types::constructible_type_parameter(decl))
        .filter_map(|decl| {
            let binder = decl.binder();
            let slot = function
                .var_names
                .iter()
                .enumerate()
                .skip(function.n_params)
                .position(|(slot, name)| {
                    *name == *binder.name
                        && matches!(function.var_tys.get(&(slot as VarId)),
                            Some(Ty::Param { binder: typed, .. }) if *typed == binder)
                })?;
            Some((slot + function.n_params) as VarId)
        })
        .filter(|slot| !addressed.contains(slot))
        .collect()
}

pub(super) fn renumber_slots(function: &mut MirFunction, renumber: &dyn Fn(VarId) -> VarId) {
    renumber_blocks(&mut function.blocks, renumber);
    for (_, origin) in function.spans.0.values_mut() {
        if let Some(var) = origin {
            *var = renumber(*var);
        }
    }
}

pub(super) fn renumber_blocks(blocks: &mut [MirBlock], renumber: &dyn Fn(VarId) -> VarId) {
    for block in blocks {
        for instruction in &mut block.instrs {
            renumber_instruction(instruction, renumber);
        }
        match &mut block.term {
            MirTerm::ReturnWithCleanup { cleanup, .. } | MirTerm::EscapeJump { cleanup, .. } => {
                for var in cleanup {
                    *var = renumber(*var);
                }
            }
            MirTerm::ComptimeFor { slot, .. } => *slot = renumber(*slot),
            MirTerm::Jump(_)
            | MirTerm::Branch { .. }
            | MirTerm::ComptimeBranch { .. }
            | MirTerm::Return(_)
            | MirTerm::FallOff => {}
        }
    }
}

fn renumber_instruction(instruction: &mut MirInstr, renumber: &dyn Fn(VarId) -> VarId) {
    for place in instruction_places_mut(instruction) {
        place.root = renumber(place.root);
        if let Some(through) = &mut place.through {
            *through = renumber(*through);
        }
    }
    for access in capture_accesses_mut(instruction) {
        access.root = renumber(access.root);
    }
    match instruction {
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            *reference = renumber(*reference);
            for interior in loans
                .iter_mut()
                .filter_map(|loan| loan.interior.as_mut())
                .chain(dest_interior.as_mut())
            {
                interior.root = renumber(interior.root);
            }
        }
        MirInstr::InvalidateInteriors { base, except, .. } => {
            base.root = renumber(base.root);
            if let Some(except) = except {
                *except = renumber(*except);
            }
        }
        MirInstr::KeepAlive { var }
        | MirInstr::UseVar { var, .. }
        | MirInstr::DefVar { var, .. }
        | MirInstr::DropVar { var }
        | MirInstr::ConsumeVar { var } => *var = renumber(*var),
        MirInstr::GetIter { source, dest, .. } => {
            *source = renumber(*source);
            *dest = renumber(*dest);
        }
        MirInstr::HasNext { iter, .. }
        | MirInstr::Next { iter, .. }
        | MirInstr::TryNext { iter, .. } => *iter = renumber(*iter),
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            cleanup,
        } => {
            renumber_blocks(body, renumber);
            if let Some((slot, blocks)) = handler {
                if let Some(slot) = slot {
                    *slot = renumber(*slot);
                }
                renumber_blocks(blocks, renumber);
            }
            if let Some(blocks) = orelse {
                renumber_blocks(blocks, renumber);
            }
            if let Some(blocks) = finalbody {
                renumber_blocks(blocks, renumber);
            }
            for var in cleanup {
                *var = renumber(*var);
            }
        }
        _ => {}
    }
}

/// The capture accesses an instruction records — static ownership facts whose
/// roots name the same slots the body addresses.
fn capture_accesses_mut(instruction: &mut MirInstr) -> Vec<&mut mojito_mir::mir::MirCaptureAccess> {
    match instruction {
        MirInstr::Call {
            capture_accesses, ..
        }
        | MirInstr::CallIndirect {
            capture_accesses, ..
        }
        | MirInstr::MethodCall {
            capture_accesses, ..
        } => capture_accesses.iter_mut().collect(),
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => call.capture_accesses.iter_mut().collect(),
        _ => Vec::new(),
    }
}
