//! The pass-test driver, after MLIR's `mlir-opt`: load a MIR artifact, run
//! named passes over its bodies in order, and verify the program after
//! each one, in every build.

use super::pre_elaboration::{Pass, pass_finding};
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
            Self::Load(report) => {
                write!(formatter, "{report}")?;
                report
                    .diagnostics
                    .iter()
                    .try_for_each(|diagnostic| write!(formatter, "\n  {}", diagnostic.message))
            }
            Self::Verify { pass, findings } => write!(
                formatter,
                "MIR does not verify after pass `{pass}`: {}",
                findings.join("; ")
            ),
        }
    }
}

impl std::error::Error for OptError {}

/// Load the artifact `input` and run `passes` in order, verifying the
/// whole program after each pass.
///
/// As in the pre-elaboration pipeline, a program pass runs over the
/// program and a body pass over every body but `__toplevel__`.
pub fn opt(
    input: &[u8],
    source_name: &str,
    passes: &[&'static Pass],
) -> Result<MirProgram, OptError> {
    let mut program = load_artifact(input, source_name)
        .map_err(OptError::Load)?
        .program;
    for pass in passes {
        match pass {
            Pass::Program(pass) => (pass.run)(&mut program),
            Pass::Body(pass) => {
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
            }
        }
        let findings: Vec<_> = mojito_mir::mir::verify::verify(&program)
            .iter()
            .map(|finding| pass_finding(pass.name(), finding))
            .collect();
        if !findings.is_empty() {
            return Err(OptError::Verify {
                pass: pass.name(),
                findings,
            });
        }
    }
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::{OptError, Pass, opt};
    use crate::analysis::{BodyPass, ELABORATE_DROPS};
    use mojito_mir::mir::text::disassemble_functions;
    use mojito_mir::mir::{MirDeclarations, MirFunction, MirInstr};

    /// A pass that drops every dropped slot twice.
    const DOUBLE_DROP: Pass = Pass::Body(BodyPass {
        name: "double-drop",
        run: double_drops,
    });

    fn double_drops(_: &str, function: &mut MirFunction, _: &MirDeclarations) -> bool {
        for block in &mut function.blocks {
            block.instrs = std::mem::take(&mut block.instrs)
                .into_iter()
                .flat_map(|instruction| match instruction {
                    MirInstr::DropVar { var } => vec![instruction, MirInstr::DropVar { var }],
                    other => vec![other],
                })
                .collect();
        }
        true
    }

    #[test]
    fn a_pass_that_drops_an_ended_slot_fails_verification() {
        let input =
            include_bytes!("../../../../tests/mir_passes/elaborate-drops/already_elaborated.mir");
        let error = opt(input, "already_elaborated.mir", &[&DOUBLE_DROP])
            .expect_err("a doubled drop does not verify");
        let OptError::Verify { pass, findings } = error else {
            panic!("expected a verification failure after the pass, got {error}");
        };
        assert_eq!(pass, "double-drop");
        assert!(
            findings.iter().any(|finding| finding
                .contains("drop.var of slot $v1 ('b') that every path has already ended")),
            "{findings:?}"
        );
    }

    /// A return that crosses no `finally` holds nothing past its own
    /// block: elaboration ends a listed slot at its last use and leaves a
    /// plain `return`.
    #[test]
    fn a_top_level_return_lists_nothing() {
        let input =
            include_str!("../../../../tests/mir_passes/elaborate-drops/already_elaborated.mir")
                .replace("drop.var { var: $v2 },\n              binary", "binary")
                .replace(
                    "return { value: present(%r5) }",
                    "return.cleanup { value: present(%r5), cleanup: [$v2] }",
                );
        assert!(input.contains("return.cleanup"));
        let program = opt(
            input.as_bytes(),
            "already_elaborated.mir",
            &[&Pass::Body(ELABORATE_DROPS)],
        )
        .expect("the listed slot is held at the return");
        let printed = disassemble_functions(&program).expect("prints");
        assert!(!printed.contains("return.cleanup"), "{printed}");
        assert!(printed.contains("drop.var { var: $v2 }"), "{printed}");
    }
}
