//! Per-fixture corpus tests over `assets/` — one libtest-mimic trial per
//! `.mojo` file, so nextest (and `cargo test`'s thread pool) schedule fixture
//! compiles across cores instead of serializing them inside a single sweep.
//!
//! Each group deliberately pins a distinct pipeline entry path; do not merge
//! their per-fixture work even when they visit the same file:
//!
//! - `assets_<category>::<name>` — outcome classification through the
//!   snippet-mode whole-program `Compiler` (`with_snippet_module_scope`),
//!   asserting the fixture lands at the outcome its folder names (see
//!   `assets/README.md`).
//! - `vm_ok::<name>` — the production `Compiler::default()` compile+execute
//!   path over `assets/ok`.
//! - `verify::<category>::<name>` — raw phase functions (link → elaborate →
//!   check_program → lower_checked_program → elaborate_drops_program →
//!   verify) over every executable fixture category.
//! - `origin_ok::<name>` / `origin_error::<name>` — production `Compiler`
//!   accept/reject for checked-origin fixtures.
//! - `ownership_ok::<name>` / `ownership_error::<name>` — the standalone
//!   ownership analysis (parse → elaborate → check → check_ownership); every
//!   error fixture must pin its message with `# expect:`.
//!
//! A fixture may pin the reported message with a top `# expect: <substring>`
//! comment (valid Mojo — the lexer skips it). Corpus non-emptiness invariants
//! are synthetic `*::guard_*` trials so a violation reads as an ordinary test
//! failure.

use libtest_mimic::{Arguments, Failed, Trial};
use mojito::analysis::elaborate_drops_program;
use mojito::mir::verify::verify;
use mojito::{
    Compiler, CompilerError, OwnershipError, check, check_ownership, check_program, elaborate,
    link, parse,
};
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let arguments = Arguments::from_args();
    let mut trials = Vec::new();
    assets_outcome_trials(&mut trials);
    vm_ok_trials(&mut trials);
    verify_trials(&mut trials);
    origin_trials(&mut trials);
    ownership_trials(&mut trials);
    libtest_mimic::run(&arguments, trials).exit();
}

/// The `.mojo` files in `assets/<category>/`, sorted for deterministic
/// enumeration. A missing corpus directory is a repository defect, reported
/// loudly rather than yielding a silently empty group.
fn fixtures(category: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(category);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("fixture directory {}: {error}", dir.display()))
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "mojo")
        })
        .collect();
    files.sort();
    files
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .expect("fixture stem")
        .to_string_lossy()
        .into_owned()
}

/// `input.mojo` is an interactive smoke fixture. Running it from a test
/// inherits Cargo's stdin and can block indefinitely.
fn reads_stdin(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "input.mojo")
}

/// The `# expect: <substring>` directive (if any), pinning the error message.
fn expected_substring(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix('#')?
            .trim_start()
            .strip_prefix("expect:")
            .map(|s| s.trim().to_string())
    })
}

/// The `# requires: discovery` directive marks a fixture whose semantics need
/// the `Compiler`'s whole-program discovery/specialization handoff (e.g. the
/// checker-inferred scalar-range constructor rewrite). The phase-composed
/// `verify::*` seam is documented as non-authoritative for exactly that
/// handoff (AGENTS.md), so such fixtures pin lowering and verification
/// through the authoritative `vm_ok`/`assets_ok` Compiler trials instead.
fn requires_discovery(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|source| {
        source.lines().any(|line| {
            line.trim_start()
                .strip_prefix('#')
                .and_then(|rest| rest.trim_start().strip_prefix("requires:"))
                .is_some_and(|value| value.trim() == "discovery")
        })
    })
}

fn fail(message: String) -> Failed {
    Failed::from(message)
}

/// The pipeline stage at which a program is first rejected (or `Ok`).
#[derive(Debug, PartialEq, Clone, Copy)]
enum Outcome {
    Ok,
    ParseError,
    TypeError,
    OwnershipError,
    RuntimeError,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::ParseError => "parse_error",
            Outcome::TypeError => "type_error",
            Outcome::OwnershipError => "ownership_error",
            Outcome::RuntimeError => "runtime_error",
        }
    }
}

/// Run the full pipeline, returning where it first fails and the message.
fn classify(path: &Path) -> (Outcome, String) {
    // Historical fixture files include isolated executable snippets at module
    // scope. Production compilation rejects that non-Mojo convenience; this
    // test opts in explicitly until those fixtures are migrated into `main`.
    let compiler = Compiler::default().with_snippet_module_scope();
    let compiled = match compiler.compile_path(path) {
        Ok(program) => program,
        Err(CompilerError::Module(mojito::ModuleError::Parse { err, .. })) => {
            return (Outcome::ParseError, err.to_string());
        }
        Err(CompilerError::Parse(error)) => return (Outcome::ParseError, error.to_string()),
        Err(CompilerError::Ownership(error)) => {
            return (Outcome::OwnershipError, error.to_string());
        }
        Err(error) => return (Outcome::TypeError, error.to_string()),
    };
    match compiler.execute(&compiled) {
        Ok(_) => (Outcome::Ok, String::new()),
        Err(error) => (Outcome::RuntimeError, error.to_string()),
    }
}

fn assets_outcome_trials(trials: &mut Vec<Trial>) {
    for (category, expected) in [
        ("ok", Outcome::Ok),
        ("parse_error", Outcome::ParseError),
        ("type_error", Outcome::TypeError),
        ("ownership_error", Outcome::OwnershipError),
        ("runtime_error", Outcome::RuntimeError),
    ] {
        for path in fixtures(category) {
            if category == "ok" && reads_stdin(&path) {
                continue;
            }
            let name = format!("assets_{category}::{}", stem(&path));
            trials.push(Trial::test(name, move || {
                let source = fs::read_to_string(&path).expect("read fixture");
                let (got, message) = classify(&path);
                let shown = if message.is_empty() {
                    "no error"
                } else {
                    message.as_str()
                };
                if got != expected {
                    return Err(fail(format!(
                        "expected {}, got {} ({shown})",
                        expected.label(),
                        got.label(),
                    )));
                }
                if let Some(substring) = expected_substring(&source)
                    && !message.contains(&substring)
                {
                    return Err(fail(format!(
                        "error did not contain '{substring}' (got: {shown})"
                    )));
                }
                Ok(())
            }));
        }
    }
}

fn vm_ok_trials(trials: &mut Vec<Trial>) {
    let paths: Vec<PathBuf> = fixtures("ok")
        .into_iter()
        .filter(|path| !reads_stdin(path))
        .collect();
    let count = paths.len();
    trials.push(Trial::test("vm_ok::guard_nonempty", move || {
        if count == 0 {
            return Err(fail("expected some ok fixtures".to_string()));
        }
        Ok(())
    }));
    // The VM is the sole executor: every `assets/ok/*.mojo` fixture must pass
    // the authoritative whole-program discovery/specialization pipeline and
    // run without error. (Exact-output correctness is asserted by targeted
    // tests.)
    for path in paths {
        trials.push(Trial::test(format!("vm_ok::{}", stem(&path)), move || {
            let compiler = Compiler::default();
            let program = compiler
                .compile_path(&path)
                .map_err(|error| fail(format!("compile failed on ok fixture: {error}")))?;
            compiler
                .execute(&program)
                .map_err(|error| fail(format!("vm failed on ok fixture: {error}")))?;
            Ok(())
        }));
    }
}

fn verify_trials(trials: &mut Vec<Trial>) {
    let mut total = 0usize;
    for category in ["ok", "origin_ok", "ownership_ok"] {
        for path in fixtures(category) {
            if requires_discovery(&path) {
                continue;
            }
            total += 1;
            let name = format!("verify::{category}::{}", stem(&path));
            trials.push(Trial::test(name, move || {
                let program = link(&path).map_err(|error| fail(format!("link: {error}")))?;
                let program =
                    elaborate(program).map_err(|error| fail(format!("elaborate: {error}")))?;
                let checked =
                    check_program(&program).map_err(|error| fail(format!("check: {error:?}")))?;
                let mir = mojito::mir::lower_checked_program(&checked);
                if !mir.invariant_errors.is_empty() {
                    return Err(fail(format!(
                        "invariant errors: {:?}",
                        mir.invariant_errors
                    )));
                }
                // Drop elaboration must preserve every verification invariant
                // the VM relies on: the executed program is the elaborated one.
                let elaborated = elaborate_drops_program(mir);
                let errors = verify(&elaborated);
                if !errors.is_empty() {
                    return Err(fail(format!("{errors:?}")));
                }
                Ok(())
            }));
        }
    }
    trials.push(Trial::test("verify::guard_corpus_size", move || {
        if total <= 40 {
            return Err(fail(format!("fixture corpus unexpectedly small: {total}")));
        }
        Ok(())
    }));
}

fn origin_trials(trials: &mut Vec<Trial>) {
    for path in fixtures("origin_ok") {
        trials.push(Trial::test(
            format!("origin_ok::{}", stem(&path)),
            move || {
                let compiler = Compiler::default();
                let program = compiler
                    .compile_path(&path)
                    .map_err(|error| fail(error.to_string()))?;
                compiler
                    .execute(&program)
                    .map_err(|error| fail(error.to_string()))?;
                Ok(())
            },
        ));
    }
    for path in fixtures("origin_error") {
        let name = format!("origin_error::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            let source = fs::read_to_string(&path).expect("read origin fixture");
            let message = match Compiler::default().compile_path(&path) {
                Err(error) => error.to_string(),
                Ok(_) => {
                    return Err(fail(
                        "origin error fixture must fail compilation".to_string(),
                    ));
                }
            };
            if let Some(expected) = expected_substring(&source)
                && !message.contains(&expected)
            {
                return Err(fail(format!("expected '{expected}' in '{message}'")));
            }
            Ok(())
        }));
    }
}

/// Elaborate and type-check `src` (the production stage order), then run the
/// ownership analysis. (Mirrors the helper `tests/ownership_test.rs` keeps for
/// its targeted tests.)
fn own(src: &str) -> Result<(), OwnershipError> {
    let program = parse(src).expect("parse error");
    let program = elaborate(program).expect("comptime error");
    check(&program).expect("type error");
    check_ownership(&program)
}

fn ownership_trials(trials: &mut Vec<Trial>) {
    let error_paths = fixtures("ownership_error");
    let count = error_paths.len();
    trials.push(Trial::test(
        "ownership_error::guard_corpus_size",
        move || {
            if count < 4 {
                return Err(fail(format!(
                    "expected several ownership-error fixtures, found {count}"
                )));
            }
            Ok(())
        },
    ));
    // Every `assets/ownership_error/*.mojo` must be a move violation whose
    // message contains its required `# expect:` substring.
    for path in error_paths {
        let name = format!("ownership_error::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            let src = fs::read_to_string(&path).expect("read fixture");
            let expect = expected_substring(&src)
                .expect("ownership_error fixture must pin a `# expect:` substring");
            match own(&src) {
                Err(error) => {
                    let message = error.to_string();
                    if !message.contains(&expect) {
                        return Err(fail(format!(
                            "message {message:?} lacks expected {expect:?}"
                        )));
                    }
                    Ok(())
                }
                Ok(()) => Err(fail("expected an ownership error".to_string())),
            }
        }));
    }
    // Every `assets/ownership_ok/*.mojo` must pass the ownership analysis
    // (analysis only — no VM execution).
    for path in fixtures("ownership_ok") {
        let name = format!("ownership_ok::{}", stem(&path));
        trials.push(Trial::test(name, move || {
            let src = fs::read_to_string(&path).expect("read fixture");
            own(&src).map_err(|error| fail(format!("expected no ownership error, got {error:?}")))
        }));
    }
}
