//! The post-elaboration pipeline: passes over each concrete function the
//! elaborator emits, as upstream nests a `'kgen.func'` pipeline (`SROA`,
//! `Mem2Reg`, `SCCP`, `Canonicalizer`, `CSEPass`) after
//! `ElaborateGenerators`. Folding here sees what only an instance knows: a
//! compile-time parameter read is a constant once the elaborator answers it.
//!
//! The passes are the pre-elaboration pipeline's own, but
//! `lower-arg-conventions`, which only this seam runs. Unlike that seam, no
//! body is verified after each pass: each pass reports whether it changed
//! a body, and the caller re-admits the program as concrete once, verifying
//! only the changed bodies (`mojito_mir::mir::ConcreteMir::rewrite_bodies`).

use super::canonicalize::CANONICALIZE;
use super::cse::CSE;
use super::dead_symbols::ELIMINATE_DEAD_CONCRETE_SYMBOLS;
use super::duplicate_functions::ELIMINATE_DUPLICATE_FUNCTIONS;
use super::mem2reg::{LOWER_ARG_CONVENTIONS, MEM_2_REG};
use super::pre_elaboration::{BodyPass, ProgramPass};
use super::sccp::SCCP;
use super::simplify_cf::SIMPLIFY_CF;
use super::sroa::SROA;
use mojito_common::timing;
use mojito_mir::mir::{MirDeclarations, MirFunction, MirProgram};

/// The post-elaboration pipeline, in order. It is per function, so it holds
/// no program pass.
///
/// It is upstream's `LowerArgConventions` (its callee half, which stores
/// each register-passable argument into a temporary) and `Mem2Reg`, which
/// its first optimization pipeline runs on every function, then the head
/// of the function pipeline its `AutomaticInline` runs: `SimplifyCF`,
/// `SROA`, `Mem2Reg`, `SCCP`, `Canonicalizer`, and `CSEPass`.
pub const POST_ELABORATION_PIPELINE: &[BodyPass] = &[
    LOWER_ARG_CONVENTIONS,
    MEM_2_REG,
    SIMPLIFY_CF,
    SROA,
    MEM_2_REG,
    SCCP,
    CANONICALIZE,
    CSE,
];

/// The symbol passes upstream runs over the elaborator's output before its
/// function pipeline, in order: `EliminateDuplicateFunctions`, then
/// `EliminateDeadSymbols`.
pub const ELABORATED_SYMBOL_PASSES: &[ProgramPass] = &[
    ELIMINATE_DUPLICATE_FUNCTIONS,
    ELIMINATE_DEAD_CONCRETE_SYMBOLS,
];

/// Run [`ELABORATED_SYMBOL_PASSES`] over the concrete program `program`,
/// which exports `exports` beside its entry roots, each pass under its own
/// timing span.
pub fn eliminate_elaborated_symbols(program: &mut MirProgram, exports: &[String]) {
    for pass in ELABORATED_SYMBOL_PASSES {
        let _pass = timing::span(pass.name);
        (pass.run)(program, exports);
    }
}

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
