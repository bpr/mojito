//! Runtime parameters an instance gains for a callable parameter.
//!
//! A callable parameter is ordinarily a compile-time input: the instance folds
//! the bound callable's name into its body and calls it directly. A closure
//! that captures carries an environment no name recovers. A generic closure's
//! instance still folds the lifted body, and takes the captured places as
//! trailing reference parameters it passes to the body's specialization; any
//! other closure's instance takes the closure itself as its last runtime
//! parameter and keeps the indirect call. Either way the new slots join the
//! leading parameter block (`vars[0..n_params]`, the MIR call ABI) and every
//! slot the move displaces is renumbered.

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

/// Append one reference parameter per captured place, referring to the
/// place's type in `captures`, to this function's parameter block, named for
/// the callable parameter `binder`. Answers each new slot.
pub(super) fn append_environment_parameters(
    function: &mut MirFunction,
    binder: &str,
    captures: &[Ty],
) -> Vec<VarId> {
    let first_local = function.n_params as VarId;
    let count = captures.len() as VarId;
    renumber_slots(function, &|var| {
        if var >= first_local { var + count } else { var }
    });
    function.var_tys = function
        .var_tys
        .iter()
        .map(|(var, ty)| {
            (
                if *var >= first_local {
                    var + count
                } else {
                    *var
                },
                ty.clone(),
            )
        })
        .collect();
    captures
        .iter()
        .enumerate()
        .map(|(position, ty)| {
            let slot = first_local + position as VarId;
            function
                .var_names
                .insert(slot as usize, format!("{binder}$env{position}"));
            function.var_tys.insert(slot, ty.clone());
            function.n_vars += 1;
            function.n_params += 1;
            function.param_types.push(ty.clone());
            function.owned_params.push(false);
            function.deinit_params.push(false);
            function.ref_params.push(true);
            slot
        })
        .collect()
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

/// Declare an environment parameter on the instance's signature, passed as
/// the lifted body's own capture parameter at `position` is.
pub(super) fn declare_environment_parameter(
    declaration: &mut MirFunctionDeclaration,
    name: String,
    ty: Ty,
    body: &MirFunctionDeclaration,
    position: usize,
) {
    declaration.param_names.push(name);
    declaration.param_types.push(ty);
    declaration.defaults.push(None);
    declaration.required.push(true);
    declaration
        .param_conventions
        .push(body.param_conventions.get(position).copied().flatten());
    declaration.ref_params.push(true);
    declaration
        .param_writes
        .push(body.param_writes.get(position).copied().unwrap_or(true));
}
