//! The post-elaboration pipeline: passes over each concrete function the
//! elaborator emits, as upstream nests a `'kgen.func'` pipeline (`SROA`,
//! `Mem2Reg`, `SCCP`, `Canonicalizer`, `CSEPass`) after
//! `ElaborateGenerators`. Folding here sees what only an instance knows: a
//! compile-time parameter read is a constant once the elaborator answers it.
//!
//! The passes are the pre-elaboration pipeline's own. Unlike that seam, no
//! body is verified after each pass: each pass reports whether it changed
//! a body, and the caller re-admits the program as concrete once, verifying
//! only the changed bodies (`mojito_mir::mir::ConcreteMir::rewrite_bodies`).

use super::canonicalize::CANONICALIZE;
use super::pre_elaboration::BodyPass;
use super::sccp::SCCP;
use mojito_common::timing;
use mojito_mir::mir::{MirDeclarations, MirFunction};

/// The post-elaboration pipeline, in order: upstream's `'kgen.func'`
/// pipeline without the passes Mojito does not have (`SROA`, `Mem2Reg`,
/// `CSEPass`). It is per function, so it holds no program pass.
pub const POST_ELABORATION_PIPELINE: &[BodyPass] = &[SCCP, CANONICALIZE];

/// Run [`POST_ELABORATION_PIPELINE`] over every concrete function in
/// `functions` but `__toplevel__`.
///
/// Each pass runs under its own timing span. Returns the index of each
/// function some pass changed, ascending.
pub fn post_elaborate_program(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) -> Vec<usize> {
    let mut changed = vec![false; functions.len()];
    for pass in POST_ELABORATION_PIPELINE {
        let _pass = timing::span(pass.name);
        for ((name, function), changed) in functions.iter_mut().zip(&mut changed) {
            // Module-scope (`__toplevel__`) variables keep their final
            // values for the CLI/`bindings()` global dump, so no pass
            // rewrites them.
            if name != "__toplevel__" {
                *changed |= (pass.run)(name, function, declarations);
            }
        }
    }
    timing::count(
        "unchanged_functions",
        changed.iter().filter(|changed| !**changed).count() as u64,
    );
    changed
        .into_iter()
        .enumerate()
        .filter_map(|(at, changed)| changed.then_some(at))
        .collect()
}
