//! Pass tests in the shape of MLIR's `mlir-opt` golden tests: each case
//! under `tests/mir_passes/<pass>/` is a canonical MIR artifact, `<case>.mir`,
//! run through the pass its directory names; the printed `functions:`
//! section is compared with `<case>.expected` (`UPDATE_EXPECT=1` rewrites
//! it). A case with a `<case>.error` file instead expects `opt` to fail
//! with that report, after `mlir-opt -verify-diagnostics`. See
//! `tests/mir_passes/README.md`.

use libtest_mimic::{Arguments, Failed, Trial};
use mojito::analysis::{opt, pass_named};
use mojito::mir::text::disassemble_functions;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mir_passes");
    let cases = cases(&root);
    let count = cases.len();
    let mut trials = vec![Trial::test("mir_pass::guard", move || {
        if count == 0 {
            return Err(Failed::from("no MIR pass cases under tests/mir_passes"));
        }
        Ok(())
    })];
    for (pass, path) in cases {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        trials.push(Trial::test(
            format!("mir_pass::{pass}::{stem}"),
            move || run_case(&pass, &path),
        ));
    }
    libtest_mimic::run(&Arguments::from_args(), trials).exit();
}

/// Every `<pass>/<case>.mir` under `root`, sorted.
fn cases(root: &Path) -> Vec<(String, PathBuf)> {
    let mut cases = Vec::new();
    for directory in std::fs::read_dir(root).expect("read tests/mir_passes") {
        let directory = directory.expect("read tests/mir_passes entry").path();
        if !directory.is_dir() {
            continue;
        }
        let pass = directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for file in std::fs::read_dir(&directory).expect("read a pass directory") {
            let file = file.expect("read a pass directory entry").path();
            if file.extension().is_some_and(|extension| extension == "mir") {
                cases.push((pass.clone(), file));
            }
        }
    }
    cases.sort();
    cases
}

fn run_case(pass_name: &str, path: &Path) -> Result<(), Failed> {
    let pass = pass_named(pass_name)
        .ok_or_else(|| format!("directory `{pass_name}` names no registered pass"))?;
    let input = std::fs::read(path).map_err(|error| format!("read case: {error}"))?;
    let label = format!(
        "{pass_name}/{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let error_file = path.with_extension("error");
    if error_file.exists() {
        let error = opt(&input, &label, &[pass])
            .err()
            .ok_or("the case has an `.error` file but `opt` accepts it")?;
        expect_test::expect_file![error_file].assert_eq(&format!("{error}\n"));
        return Ok(());
    }
    let loaded = opt(&input, &label, &[]).map_err(|error| error.to_string())?;
    let canonical = disassemble_functions(&loaded).map_err(|error| error.to_string())?;
    if functions_section(&input) != Some(canonical.as_str()) {
        return Err(Failed::from(
            "input is not canonical: pass it through `mojito opt` first",
        ));
    }
    let program = opt(&input, &label, &[pass]).map_err(|error| error.to_string())?;
    let printed = disassemble_functions(&program).map_err(|error| error.to_string())?;
    expect_test::expect_file![path.with_extension("expected")].assert_eq(&printed);
    Ok(())
}

/// The `functions: [ … ]` section of an artifact's text, as
/// [`disassemble_functions`] prints it: from its opening line through the
/// line before the artifact's closing brace.
fn functions_section(input: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(input).ok()?;
    let start = text.find("\n  functions: [\n")? + 1;
    text[start..].strip_suffix("}\n")
}
