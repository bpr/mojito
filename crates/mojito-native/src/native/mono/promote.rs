//! Runtime promotion of a compile-time callable parameter.
//!
//! A callable parameter is ordinarily a compile-time input: the instance folds
//! the bound callable's name into its body and calls it directly. A closure
//! that captures carries an environment no name recovers, so its instance
//! takes the closure as its last runtime parameter and keeps the indirect
//! call. Promotion moves the parameter's variable slot into the leading
//! parameter block (`vars[0..n_params]`, the MIR call ABI) and renumbers every
//! slot the move displaces. Folding a lifted body's leading capture
//! parameters into constant locals is the same renumbering in reverse.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_hir::hir::VarId;
use mojito_mir::mir::verify::instruction_places_mut;

/// Make the variable slot named `name` this function's last runtime parameter,
/// answering the type it takes. `None` when the function has no such slot, or
/// when it is already a parameter.
pub(super) fn promote_to_runtime_parameter(function: &mut MirFunction, name: &str) -> Option<Ty> {
    let slot = function
        .var_names
        .iter()
        .position(|candidate| candidate == name)? as VarId;
    let first_local = function.n_params as VarId;
    if slot < first_local {
        return None;
    }
    let ty = function.var_tys.get(&slot)?.clone();
    // Rotate `[first_local, slot]` right by one: the callable slot lands at the
    // end of the parameter block and the locals it passes keep their order.
    let renumber = |var: VarId| {
        if var == slot {
            first_local
        } else if var >= first_local && var < slot {
            var + 1
        } else {
            var
        }
    };
    renumber_slots(function, &renumber);
    let moved = function.var_names.remove(slot as usize);
    function.var_names.insert(first_local as usize, moved);
    function.var_tys = function
        .var_tys
        .iter()
        .map(|(var, ty)| (renumber(*var), ty.clone()))
        .collect();
    function.n_params += 1;
    function.param_types.push(ty.clone());
    function.owned_params.push(false);
    function.deinit_params.push(false);
    function.ref_params.push(false);
    Some(ty)
}

/// Declare the promoted parameter on the instance's signature, so the call
/// ABI the backend compiles matches the body.
pub(super) fn declare_runtime_parameter(
    declaration: &mut MirFunctionDeclaration,
    name: &str,
    ty: Ty,
) {
    declaration.param_names.push(name.to_string());
    declaration.param_types.push(ty);
    declaration.defaults.push(None);
    declaration.required.push(true);
    declaration.param_conventions.push(None);
    declaration.ref_params.push(false);
    declaration.param_writes.push(false);
}

/// Fold a lifted body's leading capture parameters into locals holding the
/// constants the instance binds them to, in capture order: the instance of a
/// nested `def` whose captures are all its enclosing function's value
/// parameters takes no environment. The folded slots move behind the body's
/// own parameters, and a place that read a capture through its reference
/// parameter reads the local directly.
pub(super) fn fold_leading_captures(function: &mut MirFunction, constants: &[Const]) {
    let folded = constants.len();
    let first_local = function.n_params;
    let (folded_slots, first_local_slot) = (folded as VarId, first_local as VarId);
    read_captures_directly(&mut function.blocks, folded_slots);
    let renumber = |var: VarId| {
        if var < folded_slots {
            first_local_slot - folded_slots + var
        } else if var < first_local_slot {
            var - folded_slots
        } else {
            var
        }
    };
    renumber_slots(function, &renumber);
    function.var_names[..first_local].rotate_left(folded);
    function.var_tys = function
        .var_tys
        .iter()
        .map(|(var, ty)| (renumber(*var), ty.clone()))
        .collect();
    function.n_params -= folded;
    let types: Vec<Ty> = function.param_types.drain(..folded).collect();
    function.owned_params.drain(..folded);
    function.deinit_params.drain(..folded);
    function.ref_params.drain(..folded);
    let mut initializers = Vec::with_capacity(2 * folded);
    for (slot, (k, ty)) in (function.n_params..).zip(constants.iter().zip(types)) {
        let dest = Reg(function.n_regs);
        function.n_regs += 1;
        function.reg_types.insert(dest.0, ty.clone());
        initializers.push(MirInstr::Const { dest, k: k.clone() });
        initializers.push(MirInstr::DefVar {
            var: slot as VarId,
            src: dest,
            binding_ty: Some(ty),
        });
    }
    if let Some(entry) = function.blocks.first_mut() {
        entry.instrs.splice(0..0, initializers);
    }
}

/// Drop a folded instance's leading capture parameters from its signature,
/// as [`fold_leading_captures`] drops them from its body.
pub(super) fn fold_leading_capture_parameters(
    declaration: &mut MirFunctionDeclaration,
    folded: usize,
) {
    declaration.param_names.drain(..folded);
    declaration.param_types.drain(..folded);
    declaration.defaults.drain(..folded);
    declaration.required.drain(..folded);
    declaration.param_conventions.drain(..folded);
    declaration.ref_params.drain(..folded);
    declaration.param_writes.drain(..folded);
    for index in [
        &mut declaration.variadic_index,
        &mut declaration.kw_variadic_index,
        &mut declaration.positional_only,
        &mut declaration.keyword_only,
    ]
    .into_iter()
    .flatten()
    {
        *index = index.saturating_sub(folded);
    }
}

fn read_captures_directly(blocks: &mut [MirBlock], folded: VarId) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        for place in instruction_places_mut(instruction) {
            if place.through.is_some_and(|through| through < folded) {
                place.through = None;
            }
        }
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = instruction
        {
            read_captures_directly(body, folded);
            for blocks in handler
                .iter_mut()
                .map(|(_, blocks)| blocks)
                .chain(orelse)
                .chain(finalbody)
            {
                read_captures_directly(blocks, folded);
            }
        }
    }
}
