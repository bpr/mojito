//! The pre-elaboration pipeline: passes over parametric MIR whose drops are
//! inserted, run once per body before the elaborator instantiates it, as
//! upstream runs its pre-elaboration pass pipeline on parametric IR after
//! destructor insertion.
//!
//! It runs at two seams: over a program's own bodies
//! ([`pre_elaborate_program`]) and over each imported package body as it is
//! finished ([`finish_package_body`]). Never per instance.

use super::elaborate_drops_function;
use mojito_mir::mir::{MirDeclarations, MirFunction, MirProgram};

/// One pre-elaboration pass over a parametric body whose drops are
/// inserted.
pub struct BodyPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    pub run: fn(&str, &mut MirFunction, &MirDeclarations),
}

/// The pre-elaboration pipeline, in order: upstream's pass list
/// (`docs/pre-elaboration-optimization-plan.md`) as Mojito adopts it.
pub const PIPELINE: &[BodyPass] = &[];

/// Run [`PIPELINE`] over the body `name`. In debug builds the body is
/// verified after each pass, and each finding, naming the pass, goes to
/// `findings`.
pub fn run_pipeline(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) {
    for pass in PIPELINE {
        (pass.run)(name, function, declarations);
        if cfg!(debug_assertions) {
            findings.extend(
                mojito_mir::mir::verify::verify_function_in(name, function, declarations)
                    .into_iter()
                    .map(|finding| {
                        format!("after pre-elaboration pass `{}`: {finding}", pass.name)
                    }),
            );
        }
    }
}

/// The program seam: [`run_pipeline`] over every function of `program` but
/// `__toplevel__`, its findings folded into `invariant_errors`.
pub fn pre_elaborate_program(program: &mut MirProgram) {
    let MirProgram {
        functions,
        declarations,
        invariant_errors,
    } = program;
    for (name, function) in functions.iter_mut() {
        // Module-scope (`__toplevel__`) variables keep their final values
        // for the CLI/`bindings()` global dump, so no pass rewrites them.
        if name != "__toplevel__" {
            run_pipeline(name, function, declarations, invariant_errors);
        }
    }
}

/// The package seam's finishing hook: [`elaborate_drops_function`], then
/// [`run_pipeline`].
pub fn finish_package_body(
    name: &str,
    function: MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) -> MirFunction {
    let mut function = elaborate_drops_function(name, function, declarations);
    run_pipeline(name, &mut function, declarations, findings);
    function
}

/// [`finish_package_body`] with the pipeline switched off: drops only.
pub fn finish_package_body_without_pipeline(
    name: &str,
    function: MirFunction,
    declarations: &MirDeclarations,
    _findings: &mut Vec<String>,
) -> MirFunction {
    elaborate_drops_function(name, function, declarations)
}
