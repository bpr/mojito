//! Runtime parameters an instance gains for a callable parameter bound to a
//! closure that captures.
//!
//! A callable parameter is a compile-time input: the instance folds the bound
//! callable's name into its body and calls it directly. A closure that
//! captures carries an environment no name recovers, so its instance still
//! folds the lifted body, as upstream keys a `@parameter` closure on its body,
//! and takes the captured places as trailing reference parameters it passes to
//! the body. The new slots join the leading parameter block
//! (`vars[0..n_params]`, the MIR call ABI) and every slot they displace is
//! renumbered.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

use mojito_hir::hir::VarId;

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
