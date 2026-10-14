//! The pre-elaboration pipeline: passes over parametric MIR whose drops are
//! inserted, run before the elaborator instantiates it, as upstream runs its
//! pre-elaboration pass pipeline on parametric IR after destructor
//! insertion.
//!
//! It runs at two seams: over a program's own bodies
//! ([`pre_elaborate_program`]), where a [`ProgramPass`] runs once over the
//! whole program and a [`BodyPass`] over each body, and over each imported
//! package body as it is finished ([`finish_package_body`]), where only the
//! body passes run. Never per instance.

use super::canonicalize::CANONICALIZE;
use super::dead_symbols::ELIMINATE_DEAD_SYMBOLS;
use super::elaborate_drops_function;
use super::sccp::SCCP;
use mojito_common::timing;
use mojito_mir::mir::{MirDeclarations, MirFunction, MirProgram};

/// One pre-elaboration pass over a parametric body whose drops are
/// inserted.
pub struct BodyPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    pub run: fn(&str, &mut MirFunction, &MirDeclarations),
}

/// One pre-elaboration pass over a whole program, which may remove its
/// functions: upstream's module passes, such as `EliminateDeadSymbols`.
pub struct ProgramPass {
    /// The pass's name, as the verifier's findings after it spell it.
    pub name: &'static str,
    pub run: fn(&mut MirProgram),
}

/// A registered pass, by the unit it rewrites.
pub enum Pass {
    Program(ProgramPass),
    Body(BodyPass),
}

impl Pass {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Program(pass) => pass.name,
            Self::Body(pass) => pass.name,
        }
    }
}

/// The pre-elaboration pipeline, in order: upstream's pass rounds
/// (`docs/pre-elaboration-optimization-plan.md`) as Mojito adopts them.
///
/// Upstream's first round runs `RemoveUnusedParams`, `EliminateDeadSymbols`,
/// then `SROA`, `Mem2Reg`, and `Canonicalizer` on each generator; its second
/// runs `SROA`, `Mem2Reg`, `SCCP`, and `Canonicalizer` again. Mojito has
/// neither `RemoveUnusedParams` nor the two scalar-replacement passes, and,
/// as upstream does, runs `canonicalize` once in each round.
pub const PIPELINE: &[Pass] = &[
    Pass::Program(ELIMINATE_DEAD_SYMBOLS),
    Pass::Body(CANONICALIZE),
    Pass::Body(SCCP),
    Pass::Body(CANONICALIZE),
];

/// Drop elaboration as a named pass, so a pass test can run it over MIR
/// text whose drops are not yet inserted. The pipeline does not list it:
/// every pipeline body already had its drops inserted.
pub const ELABORATE_DROPS: BodyPass = BodyPass {
    name: "elaborate-drops",
    run: |name, function, declarations| {
        *function = elaborate_drops_function(name, std::mem::take(function), declarations);
    },
};

/// Structured control flow lowered to blocks: upstream's `LowerControlFlow`.
///
/// Upstream lowers `hlcf` as it reaches LLVM. The pipeline does not list it:
/// lowering still builds a CFG, so no pipeline body holds a region op.
pub const LOWER_CONTROL_FLOW: BodyPass = BodyPass {
    name: "lower-control-flow",
    run: |_, function, _| mojito_mir::mir::lower_control_flow(function),
};

/// The pass registered under `name`: a [`PIPELINE`] pass, or one run only
/// on request (drop elaboration, control-flow lowering).
pub fn pass_named(name: &str) -> Option<&'static Pass> {
    PIPELINE
        .iter()
        .chain(REGISTERED)
        .find(|pass| pass.name() == name)
}

/// Every registered pass's name once, in the order the pipeline first runs
/// it, then the passes run only on request.
pub fn pass_names() -> impl Iterator<Item = &'static str> {
    let mut seen = std::collections::HashSet::new();
    PIPELINE
        .iter()
        .chain(REGISTERED)
        .map(Pass::name)
        .filter(move |name| seen.insert(*name))
}

/// Run [`PIPELINE`]'s body passes over the body `name`. In debug builds
/// the body is verified after each pass, and each finding, naming the pass,
/// goes to `findings`.
///
/// A program pass has nothing to do here: a package body is finished only
/// when the elaborator first reaches it, so no finished body is dead.
pub fn run_pipeline(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) {
    for pass in PIPELINE {
        if let Pass::Body(pass) = pass {
            run_body_pass(pass, name, function, declarations, findings);
        }
    }
}

/// The program seam: [`PIPELINE`] over `program`, each program pass once
/// and each body pass over every function but `__toplevel__`, its findings
/// folded into `invariant_errors`.
pub fn pre_elaborate_program(program: &mut MirProgram) {
    for pass in PIPELINE {
        match pass {
            Pass::Program(pass) => {
                let _pass = timing::span(pass.name);
                (pass.run)(program);
            }
            Pass::Body(pass) => {
                let _pass = timing::span(pass.name);
                let MirProgram {
                    functions,
                    declarations,
                    invariant_errors,
                } = &mut *program;
                for (name, function) in functions.iter_mut() {
                    // Module-scope (`__toplevel__`) variables keep their final
                    // values for the CLI/`bindings()` global dump, so no pass
                    // rewrites them.
                    if name != "__toplevel__" {
                        run_body_pass(pass, name, function, declarations, invariant_errors);
                    }
                }
            }
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

/// A verifier finding after the pass `pass` ran, naming the pass.
pub(super) fn pass_finding(pass: &str, finding: &str) -> String {
    format!("after pre-elaboration pass `{pass}`: {finding}")
}

/// Passes outside [`PIPELINE`] that [`pass_named`] still answers.
const REGISTERED: &[Pass] = &[Pass::Body(ELABORATE_DROPS), Pass::Body(LOWER_CONTROL_FLOW)];

/// Run the body pass `pass` over the body `name`, verifying it after the
/// pass in debug builds.
fn run_body_pass(
    pass: &BodyPass,
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    findings: &mut Vec<String>,
) {
    (pass.run)(name, function, declarations);
    if cfg!(debug_assertions) {
        findings.extend(
            mojito_mir::mir::verify::verify_function_in(name, function, declarations)
                .into_iter()
                .map(|finding| pass_finding(pass.name, &finding)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_pass_resolves_by_a_unique_name() {
        let names: Vec<_> = pass_names().collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for name in names {
            assert_eq!(pass_named(name).map(Pass::name), Some(name));
        }
        assert!(pass_named("elaborate-drops").is_some());
        assert!(pass_named("eliminate-dead-symbols").is_some());
        assert!(pass_named("canonicalize").is_some());
        assert!(pass_named("sccp").is_some());
        assert!(pass_named("lower-control-flow").is_some());
        assert!(pass_named("no-such-pass").is_none());
    }
}
