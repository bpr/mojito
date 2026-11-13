//! The post-elaboration pipeline: passes over each concrete function the
//! elaborator emits, as upstream's first optimization pipeline runs after
//! `ElaborateGenerators`. Folding here sees what only an instance knows: a
//! compile-time parameter read is a constant once the elaborator answers it.
//!
//! The body passes are the pre-elaboration pipeline's own, but
//! `lower-arg-conventions`, which only this seam runs, and its call-graph
//! pass is `automatic-inline`, which nests a function pipeline of its own.
//! Unlike that seam, no body is verified after each pass: each pass reports
//! whether it changed a body, and the caller re-admits the program as
//! concrete once, verifying only the changed bodies
//! (`mojito_mir::mir::ConcreteMir::rewrite_bodies`).

use super::automatic_inline::{ALWAYS_INLINE, AUTOMATIC_INLINE};
use super::canonicalize::CANONICALIZE;
use super::cse::CSE;
use super::dead_symbols::ELIMINATE_DEAD_CONCRETE_SYMBOLS;
use super::duplicate_functions::ELIMINATE_DUPLICATE_FUNCTIONS;
use super::mem2reg::{LOWER_ARG_CONVENTIONS, MEM_2_REG};
use super::pre_elaboration::{Pass, ProgramPass, run_call_graph_pass};
use super::sccp::SCCP;
use super::sroa::SROA;
use mojito_common::timing;
use mojito_mir::mir::{MirDeclarations, MirFunction, MirProgram};

/// The post-elaboration pipeline, in order. It holds no program pass: the
/// symbol passes around it run separately ([`ELABORATED_SYMBOL_PASSES`]).
///
/// It is upstream's first optimization pipeline after elaboration as Mojito
/// adopts it: `LowerArgConventions` (its callee half, which stores each
/// register-passable argument into a temporary) and `Mem2Reg` on every
/// function, then `AutomaticInline` with its nested function pipeline
/// (`automatic-inline`), then the guaranteed `SROA`, `Mem2Reg`, and
/// `Canonicalizer`, and last `SROA`, `Mem2Reg`, `SCCP`, `Canonicalizer`, and
/// `CSEPass`. Upstream's `RaiseForLoops` and `LoopUnrolling` between those
/// two runs are not ported.
pub const POST_ELABORATION_PIPELINE: &[Pass] = &[
    Pass::Body(LOWER_ARG_CONVENTIONS),
    Pass::Body(MEM_2_REG),
    Pass::CallGraph(AUTOMATIC_INLINE),
    Pass::Body(SROA),
    Pass::Body(MEM_2_REG),
    Pass::Body(CANONICALIZE),
    Pass::Body(SROA),
    Pass::Body(MEM_2_REG),
    Pass::Body(SCCP),
    Pass::Body(CANONICALIZE),
    Pass::Body(CSE),
];

/// The pipeline the driver runs over the concrete program with the pre-
/// and post-elaboration pipelines off: only `always-inline`, since
/// upstream's `@always_inline` is mandatory, not an optimization.
pub const MANDATORY_PIPELINE: &[Pass] = &[Pass::CallGraph(ALWAYS_INLINE)];

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
/// `functions` but `__toplevel__`, the call-graph pass over them callees
/// first.
///
/// Each pass runs under its own timing span. Returns the index of each
/// function some pass changed, ascending.
pub fn post_elaborate_program(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) -> Vec<usize> {
    run_concrete_pipeline(POST_ELABORATION_PIPELINE, functions, declarations)
}

/// Run [`MANDATORY_PIPELINE`] over every concrete function in `functions`
/// but `__toplevel__`, as [`post_elaborate_program`] runs its own.
pub fn inline_always_program(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) -> Vec<usize> {
    run_concrete_pipeline(MANDATORY_PIPELINE, functions, declarations)
}

/// Run `pipeline` over every concrete function in `functions` but
/// `__toplevel__`, the call-graph passes over them callees first, each
/// pass under its own timing span; returns the index of each function some
/// pass changed, ascending.
fn run_concrete_pipeline(
    pipeline: &[Pass],
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) -> Vec<usize> {
    let mut changed = vec![false; functions.len()];
    for pass in pipeline {
        let _pass = timing::span(pass.name());
        match pass {
            Pass::Program(_) => {}
            Pass::Body(pass) => {
                for ((name, function), changed) in functions.iter_mut().zip(&mut changed) {
                    // Module-scope (`__toplevel__`) variables keep their
                    // final values for the CLI/`bindings()` global dump, so
                    // no pass rewrites them.
                    if name != "__toplevel__" {
                        *changed |= (pass.run)(name, function, declarations);
                    }
                }
            }
            Pass::CallGraph(pass) => {
                for row in run_call_graph_pass(pass, functions, declarations, None) {
                    changed[row] = true;
                }
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
