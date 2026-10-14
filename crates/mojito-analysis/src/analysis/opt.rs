//! The pass-test driver, after MLIR's `mlir-opt`: load a MIR artifact, run
//! named passes over its bodies in order, and verify the program after
//! each one, in every build.

use super::pre_elaboration::{BodyPass, pass_finding};
use mojito_mir::mir::MirProgram;
use mojito_mir::mir::text::{ArtifactReport, load_artifact};
use std::fmt;

/// Why [`opt`] produced no program.
#[derive(Debug)]
pub enum OptError {
    /// The input did not parse or verify.
    Load(ArtifactReport),
    /// The program did not verify after the pass `pass`.
    Verify {
        pass: &'static str,
        findings: Vec<String>,
    },
}

impl fmt::Display for OptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(report) => write!(formatter, "{report}"),
            Self::Verify { pass, findings } => write!(
                formatter,
                "MIR does not verify after pass `{pass}`: {}",
                findings.join("; ")
            ),
        }
    }
}

impl std::error::Error for OptError {}

/// Load the artifact `input` and run `passes` in order over every body but
/// `__toplevel__`, as the pre-elaboration pipeline does, verifying the
/// whole program after each pass.
pub fn opt(
    input: &[u8],
    source_name: &str,
    passes: &[&'static BodyPass],
) -> Result<MirProgram, OptError> {
    let mut program = load_artifact(input, source_name)
        .map_err(OptError::Load)?
        .program;
    for pass in passes {
        let MirProgram {
            functions,
            declarations,
            ..
        } = &mut program;
        for (name, function) in functions.iter_mut() {
            if name != "__toplevel__" {
                (pass.run)(name, function, declarations);
            }
        }
        let findings: Vec<_> = mojito_mir::mir::verify::verify(&program)
            .iter()
            .map(|finding| pass_finding(pass, finding))
            .collect();
        if !findings.is_empty() {
            return Err(OptError::Verify {
                pass: pass.name,
                findings,
            });
        }
    }
    Ok(program)
}
