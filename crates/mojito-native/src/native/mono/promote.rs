//! Runtime promotion of a compile-time callable parameter.
//!
//! A callable parameter is ordinarily a compile-time input: the instance folds
//! the bound callable's name into its body and calls it directly. A closure
//! that captures carries an environment no name recovers, so its instance
//! takes the closure as its last runtime parameter and keeps the indirect
//! call. Promotion moves the parameter's variable slot into the leading
//! parameter block (`vars[0..n_params]`, the MIR call ABI) and renumbers every
//! slot the move displaces.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_hir::hir::VarId;

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
