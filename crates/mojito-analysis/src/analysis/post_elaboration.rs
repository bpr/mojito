//! The post-elaboration pipeline: passes over each concrete function the
//! elaborator emits, as upstream nests a `'kgen.func'` pipeline (`SROA`,
//! `Mem2Reg`, `SCCP`, `Canonicalizer`, `CSEPass`) after
//! `ElaborateGenerators`. Folding here sees what only an instance knows: a
//! compile-time parameter read is a constant once the elaborator answers it.
//!
//! The passes are the pre-elaboration pipeline's own. Unlike that seam, no
//! body is verified after each pass: the caller re-admits the whole program
//! as concrete once, in every build
//! (`native::mono::SpecializedProgram::rewritten`).

use super::canonicalize::CANONICALIZE;
use super::pre_elaboration::BodyPass;
use super::sccp::SCCP;
use mojito_common::timing;
use mojito_mir::mir::MirProgram;

/// The post-elaboration pipeline, in order: upstream's `'kgen.func'`
/// pipeline without the passes Mojito does not have (`SROA`, `Mem2Reg`,
/// `CSEPass`). It is per function, so it holds no program pass.
pub const POST_ELABORATION_PIPELINE: &[BodyPass] = &[SCCP, CANONICALIZE];

/// Run [`POST_ELABORATION_PIPELINE`] over every concrete function of
/// `program` but `__toplevel__`, each pass under its own timing span.
pub fn post_elaborate_program(program: &mut MirProgram) {
    let MirProgram {
        functions,
        declarations,
        ..
    } = program;
    for pass in POST_ELABORATION_PIPELINE {
        let _pass = timing::span(pass.name);
        for (name, function) in functions.iter_mut() {
            // Module-scope (`__toplevel__`) variables keep their final
            // values for the CLI/`bindings()` global dump, so no pass
            // rewrites them.
            if name != "__toplevel__" {
                (pass.run)(name, function, declarations);
            }
        }
    }
}
