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

/// Drop elaboration as a named pass, so a pass test can run it over MIR
/// text whose drops are not yet inserted. The pipeline does not list it:
/// every pipeline body already had its drops inserted.
pub const ELABORATE_DROPS: BodyPass = BodyPass {
    name: "elaborate-drops",
    run: |name, function, declarations| {
        *function = elaborate_drops_function(name, std::mem::take(function), declarations);
    },
};

/// The pass registered under `name`: a [`PIPELINE`] pass, or one run only
/// on request (drop elaboration).
pub fn pass_named(name: &str) -> Option<&'static BodyPass> {
    PIPELINE
        .iter()
        .chain(REGISTERED)
        .find(|pass| pass.name == name)
}

/// Every registered pass's name, pipeline order first.
pub fn pass_names() -> impl Iterator<Item = &'static str> {
    PIPELINE.iter().chain(REGISTERED).map(|pass| pass.name)
}

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
                    .map(|finding| pass_finding(pass, &finding)),
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

/// A verifier finding on a body after `pass` ran, naming the pass.
pub(super) fn pass_finding(pass: &BodyPass, finding: &str) -> String {
    format!("after pre-elaboration pass `{}`: {finding}", pass.name)
}

/// Passes outside [`PIPELINE`] that [`pass_named`] still answers.
const REGISTERED: &[BodyPass] = &[ELABORATE_DROPS];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_pass_resolves_by_a_unique_name() {
        let names: Vec<_> = pass_names().collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for name in names {
            assert_eq!(pass_named(name).map(|pass| pass.name), Some(name));
        }
        assert!(pass_named("elaborate-drops").is_some());
        assert!(pass_named("no-such-pass").is_none());
    }
}
