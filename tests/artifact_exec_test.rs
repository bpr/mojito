//! Execute verified textual MIR artifacts end-to-end: loading-gate
//! composition, observable output, and equivalence with the direct VM run.

use mojito::analysis::elaborate_drops_program;
use mojito::mir::lower_checked_program;
use mojito::mir::text::disassemble;
use mojito::{ArtifactRunError, BackendKind, Compiler, run_artifact};
use std::path::Path;

const MINIMAL: &[u8] = include_bytes!("snapshots/mir/minimal.mir");
const CONTROL_FLOW: &[u8] = include_bytes!("snapshots/mir/control_flow.mir");
const METADATA: &[u8] = include_bytes!("snapshots/mir/metadata.mir");
const EXEC_PRINT: &[u8] = include_bytes!("snapshots/mir/exec_print.mir");

/// The canonical inspection snapshots define neither `__toplevel__` nor
/// `main`; executing them is a successful no-op.
#[test]
fn canonical_snapshots_execute_to_empty_output() {
    for (name, source) in [
        ("minimal.mir", MINIMAL),
        ("control_flow.mir", CONTROL_FLOW),
        ("metadata.mir", METADATA),
    ] {
        let execution = run_artifact(source, name, BackendKind::Vm).expect("execute artifact");
        assert_eq!(execution.output, "", "{name}");
        assert!(execution.bindings.is_empty(), "{name}");
    }
}

/// A frozen executable artifact runs without any Mojo source or recompile.
#[test]
fn frozen_artifact_prints_through_the_vm() {
    let execution =
        run_artifact(EXEC_PRINT, "exec_print.mir", BackendKind::Vm).expect("execute artifact");
    assert_eq!(execution.output, "42\n");
}

/// Disassembling a compiled fixture and executing the artifact must observe
/// exactly what the direct VM run observes.
#[test]
fn artifact_execution_matches_direct_execution() {
    for fixture in [
        "add.mojo",
        "defines_main.mojo",
        "exceptions.mojo",
        "copyinit_moveinit.mojo",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/ok")
            .join(fixture);
        let compiler = Compiler::default();
        let compiled = compiler.compile_path(&path).expect("compile fixture");
        let direct = compiler.execute(&compiled).expect("direct execution");
        let mir = lower_checked_program(compiled.checked());
        assert!(mir.invariant_errors.is_empty(), "{fixture}");
        let text = disassemble(&elaborate_drops_program(mir)).expect("disassemble");
        let via_artifact =
            run_artifact(text.as_bytes(), fixture, BackendKind::Vm).expect("execute artifact");
        assert_eq!(via_artifact.output, direct.output, "{fixture}");
        let display = |bindings: &[(String, mojito::runtime::Value)]| {
            bindings
                .iter()
                .map(|(name, value)| format!("{name} = {value}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            display(&via_artifact.bindings),
            display(&direct.bindings),
            "{fixture}"
        );
    }
}

#[test]
fn corrupted_artifacts_fail_the_loading_gate() {
    let mut corrupted = MINIMAL.to_vec();
    let position = corrupted
        .windows(4)
        .position(|window| window == b"%r0,")
        .expect("register spelling present");
    corrupted[position..position + 3].copy_from_slice(b"%r9");
    match run_artifact(&corrupted, "corrupted.mir", BackendKind::Vm) {
        Err(ArtifactRunError::Load(report)) => {
            assert!(!report.diagnostics.is_empty());
            assert!(
                report
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.span != (0, 0))
            );
        }
        other => panic!("expected a loading-gate failure, got {other:?}"),
    }
}

#[test]
fn non_vm_backends_refuse_artifact_execution() {
    match run_artifact(MINIMAL, "minimal.mir", BackendKind::Llvm) {
        Err(ArtifactRunError::Backend(message)) => {
            assert!(message.contains("not implemented"));
        }
        other => panic!("expected a backend refusal, got {other:?}"),
    }
}
