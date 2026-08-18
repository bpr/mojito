//! Backend-independent artifact contracts for every shared runnable
//! conformance case.

use libtest_mimic::{Arguments, Failed, Trial};
use mojito::mir::text::{disassemble, parse_artifact};
use mojito::{BackendKind, Compiler, run_artifact};
use std::path::{Path, PathBuf};

fn main() {
    let mut trials = Vec::new();
    let cases = run_cases();
    let count = cases.len();
    for (name, path) in cases {
        trials.push(Trial::test(format!("artifact::{name}"), move || {
            artifact_contract(&path)
        }));
    }
    trials.push(Trial::test("artifact::guard_case_count", move || {
        if count < 100 {
            return Err(fail(format!(
                "runnable conformance artifact set unexpectedly small: {count}"
            )));
        }
        Ok(())
    }));
    libtest_mimic::run(&Arguments::from_args(), trials).exit();
}

fn run_cases() -> Vec<(String, PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("conformance/cases.tsv"))
        .expect("read conformance cases");
    manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("id\t"))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?;
            let mode = fields.next()?;
            let fixture = fields.next()?;
            (mode == "run").then(|| (name.to_string(), root.join(fixture)))
        })
        .collect()
}

fn artifact_contract(path: &Path) -> Result<(), Failed> {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_path(path)
        .map_err(|error| fail(format!("compile: {error}")))?;
    let direct = compiler
        .execute(&compiled)
        .map_err(|error| fail(format!("direct execution: {error}")))?;
    let text = compiled
        .emit_mir()
        .map_err(|error| fail(format!("emit MIR: {error}")))?;
    let parsed = parse_artifact(text.as_bytes(), path.display().to_string())
        .map_err(|error| fail(format!("parse emitted MIR: {error}")))?;
    let reprinted =
        disassemble(&parsed.program).map_err(|error| fail(format!("re-disassemble: {error}")))?;
    if text != reprinted {
        return Err(fail("emitted MIR is not byte-stable".to_string()));
    }
    let artifact = run_artifact(text.as_bytes(), path.display().to_string(), BackendKind::Vm)
        .map_err(|error| fail(format!("artifact execution: {error}")))?;
    if direct.output != artifact.output {
        return Err(fail(format!(
            "output mismatch: direct {:?}, artifact {:?}",
            direct.output, artifact.output
        )));
    }
    let display = |bindings: &[(String, mojito::Value)]| {
        bindings
            .iter()
            .map(|(name, value)| format!("{name} = {value}"))
            .collect::<Vec<_>>()
    };
    if display(&direct.bindings) != display(&artifact.bindings) {
        return Err(fail("final bindings differ".to_string()));
    }
    Ok(())
}

fn fail(message: String) -> Failed {
    Failed::from(message)
}
