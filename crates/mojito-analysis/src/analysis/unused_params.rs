//! `remove-unused-params`: upstream's `RemoveUnusedParams`, which its
//! pre-elaboration pipeline runs on each generator, so that instances
//! differing only in a parameter the generator never reads are one.
//!
//! The pass records on the body the function's own compile-time parameters
//! that nothing in its body or signature reads
//! ([`mojito_mir::mir::unread_parameters`]), and the elaborator leaves
//! their arguments out of an instance's identity. Upstream also rewrites the
//! generator's signature and its callers' arguments; Mojito keeps both, and
//! the instance binds the argument its body never reads. It runs last, so a
//! read the cleanup passes removed keeps no parameter.

use super::pre_elaboration::BodyPass;
use mojito_mir::mir::{MirDeclarations, MirFunction, unread_parameters};

pub const REMOVE_UNUSED_PARAMS: BodyPass = BodyPass {
    name: "remove-unused-params",
    run: remove_unused_params,
};

/// Record `function`'s unread parameters as its `removed_params`; returns
/// whether they changed.
pub fn remove_unused_params(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let removed = unread_parameters(name, function, declarations);
    if removed == function.removed_params {
        return false;
    }
    function.removed_params = removed;
    true
}
