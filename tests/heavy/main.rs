//! The memory-heavy Pliron corpus sweeps, kept out of the ordinary test
//! gate and run on their own by `scripts/check-pliron-heavy`.
//!
//! Each test here compiles the whole fixture corpus — several hundred
//! programs — through the production front end and the native backend, with
//! a worker pool whose every thread holds a checked standard library and an
//! LLVM module. One sweep peaks around 5 GB. Run as sibling nextest
//! processes alongside each other and the rest of the Pliron suite, they
//! exhausted a 31 GB machine and the OOM killer took the parity harness
//! (gate run 2026-09-12). Two things follow, and both are load-bearing:
//!
//! - This is a separate test target (`heavy`), excluded from
//!   `scripts/check-pliron` and from the overnight gate. Run it deliberately.
//! - `support::compile_jobs` sizes the fan-out against free memory rather
//!   than against the core count, so a sweep throttles itself on a loaded
//!   machine instead of being killed.
//!
//! What lives here is exactly the sweeps: the two generated manifests
//! (`conformance/pliron-scalar.tsv`, `conformance/pliron-parity.tsv`) with
//! their coverage ratchets, the negative-ownership corpus check, and the
//! corpus-wide debug-correlation premise. Everything else about the backend
//! stays in `tests/pliron_backend_test.rs` and `tests/pliron_debug_test.rs`.

#![cfg(feature = "backend-pliron")]

use std::fmt::Write as _;
use std::path::Path;

use mojito::backend::pliron as native;
use mojito::{Compiler, CompilerError, RuntimeError};
use native::{CompileOptions, DebugInfo, OptLevel, TrapCategory};

#[path = "../pliron_support/mod.rs"]
mod support;

use support::{
    assert_jit_matches, fixture_sources, fixture_stdin, has_compute_entry, host_target,
    parallel_map, ret_kind_name, run_executable, runtime_jit_symbols,
};

/// The Stage 2 acceptance gate in one pass (one production compile per
/// eligible fixture — the compile dominates the cost, so manifest generation,
/// the O0/O1 value differential, and the trap-category differential share it):
///
/// - Every `assets/ok` fixture with the compute-entry shape either compiles
///   natively and must match the VM at `O0` and `O1`, or is recorded as
///   `excluded` with its first rejection diagnostic.
/// - Every `assets/runtime_error/pliron_trap_*` fixture must trap in the VM
///   with a recognized [`TrapCategory`] message and exit a native executable
///   with that category's exit code (`64 + code`) at both levels, printing
///   nothing on stdout while the runtime reports the category on stderr.
///   (Traps run only as subprocesses — an in-process JIT trap would exit the
///   test runner.)
/// - Everything else is recorded `ineligible`, so
///   `conformance/pliron-scalar.tsv` names every fixture exactly once.
///
/// The checked-in manifest must match regeneration byte-exactly
/// (`UPDATE_EXPECT=1` with `CARGO_WORKSPACE_DIR=$PWD` refreshes it), and the
/// trailing guards fail if eligible coverage unexpectedly shrinks.
#[test]
fn scalar_capability_manifest_and_differential() {
    let mut scalar_sources = fixture_sources("assets/ok");
    scalar_sources.extend(fixture_sources("assets/extensions/ok"));
    let ok_rows = parallel_map(scalar_sources, |(rel, src)| {
        if !has_compute_entry(&src) {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-scalar-entry-shape".into(),
            );
        }
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(&rel))
            .unwrap_or_else(|error| panic!("{rel}: ok fixture must compile: {error}"));
        let options = CompileOptions {
            entries: vec!["compute".to_string()],
            sources: vec![(rel.clone(), src)],
            target: host_target(),
            trace_lifecycle: false,
        };
        match native::compile(compiled.elaborated_mir(), &options) {
            Err(error) => {
                let detail = error.display_with_sources(&options.sources);
                (rel, "compute".into(), "excluded".to_string(), detail)
            }
            Ok(module) => {
                let execution = compiler
                    .execute(&compiled)
                    .unwrap_or_else(|error| panic!("{rel}: fixture must run on the VM: {error}"));
                let printed = execution.output.trim().to_string();
                let symbols = runtime_jit_symbols();
                let at_o0 = module
                    .jit_value_with_symbols("compute", OptLevel::O0, &symbols)
                    .unwrap_or_else(|error| panic!("{rel}: JIT at O0 failed: {error}"));
                let at_o1 = module
                    .jit_value_with_symbols("compute", OptLevel::Release, &symbols)
                    .unwrap_or_else(|error| panic!("{rel}: JIT at O1 failed: {error}"));
                assert_jit_matches(&rel, "O0", at_o0, &printed);
                assert_jit_matches(&rel, "O1", at_o1, &printed);
                let detail = format!("ret={}", ret_kind_name(at_o0));
                (rel, "compute".into(), "differential".to_string(), detail)
            }
        }
    });

    let trap_rows = parallel_map(fixture_sources("assets/runtime_error"), |(rel, src)| {
        let is_trap_fixture = rel
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("pliron_trap_"));
        if !is_trap_fixture {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-scalar-entry-shape".into(),
            );
        }
        let compiler = Compiler::default();
        let compiled = compiler
            .compile_source(&src, Path::new(&rel))
            .unwrap_or_else(|error| panic!("{rel}: trap fixture must compile: {error}"));
        let vm_error = compiler
            .execute(&compiled)
            .expect_err("trap fixture must fail on the VM")
            .to_string();
        let category = TrapCategory::from_vm_message(&vm_error).unwrap_or_else(|| {
            panic!("{rel}: VM error carries no recognized trap category: {vm_error}")
        });
        let options = CompileOptions {
            entries: vec!["main".to_string()],
            sources: vec![(rel.clone(), src)],
            target: host_target(),
            trace_lifecycle: false,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
            let exe = dir.path().join(format!("trap-{level}"));
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
                .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
            let run = std::process::Command::new(&exe)
                .output()
                .expect("trap executable runs");
            assert_eq!(
                run.status.code(),
                Some(i32::from(category.exit_code())),
                "{rel}: native trap exit status diverges at {level} (VM: {vm_error})"
            );
            assert!(
                run.stdout.is_empty(),
                "{rel}: trapping executable must print nothing on stdout at {level}"
            );
            let stderr = String::from_utf8_lossy(&run.stderr);
            assert!(
                stderr.contains(category.runtime_message()),
                "{rel}: trap stderr lacks the runtime message at {level}: {stderr}"
            );
        }
        let detail = format!("category={category:?}");
        (rel, "main".into(), "trap-differential".to_string(), detail)
    });
    let rows: Vec<(String, String, String, String)> =
        ok_rows.into_iter().chain(trap_rows).collect();

    let mut manifest = String::from(
        "# Pliron scalar capability manifest (generated; schema-version 1).\n\
         # One row per assets/ok and assets/runtime_error fixture:\n\
         #   fixture <TAB> entry <TAB> status <TAB> detail\n\
         # status: differential (VM/native value oracle, O0+O1) |\n\
         #         trap-differential (VM/native trap-category oracle, O0+O1) |\n\
         #         excluded (native rejection diagnostic) |\n\
         #         ineligible (no zero-arg value-returning `compute` entry)\n\
         # Regenerate: UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD \\\n\
         #   cargo nextest run --features backend-pliron scalar_capability_manifest\n",
    );
    for (fixture, entry, status, detail) in &rows {
        writeln!(manifest, "{fixture}\t{entry}\t{status}\t{detail}").expect("String write");
    }
    expect_test::expect_file!["../../conformance/pliron-scalar.tsv"].assert_eq(&manifest);

    // Coverage guards: the eligible sets must never silently shrink, and a
    // pliron-named fixture must never regress from differential to excluded.
    let count = |status: &str| rows.iter().filter(|(_, _, s, _)| s == status).count();
    let differential = count("differential");
    let traps = count("trap-differential");
    assert!(
        differential >= 20,
        "differential coverage unexpectedly shrank: {differential} < 20"
    );
    assert!(
        traps >= 4,
        "trap-differential coverage unexpectedly shrank: {traps} < 4"
    );
    for (fixture, _, status, detail) in &rows {
        let name = fixture.rsplit('/').next().unwrap_or(fixture);
        assert!(
            !(name.starts_with("pliron_") && status == "excluded"),
            "{fixture}: pliron fixture regressed to excluded: {detail}"
        );
    }
}

/// The Stage 4 acceptance gate in one pass, one production compile per
/// fixture:
///
/// - Every `assets/ok` and `assets/ownership_ok` fixture with a `main` entry
///   either compiles natively — then its executable's stdout at `O0` and
///   `O1` must equal the VM's execution output byte-for-byte with exit 0 and
///   empty stderr (handled raises and `finally` paths included), and its
///   `O0` AddressSanitizer/LeakSanitizer build must run equally clean (no
///   leak, double free, or invalid access anywhere in the run) — or is
///   recorded `excluded` with its first rejection diagnostic.
/// - Every `assets/runtime_error/pliron_raise_*` fixture must raise in the VM
///   and exit natively with the unhandled-error category (69), reporting
///   `unhandled error: <message>` on stderr at both levels. (The VM's `run`
///   discards buffered partial stdout on an error while native executables
///   stream it — a recorded CLI-level divergence, so stdout is not compared
///   on the raise rows.)
///
/// The checked-in `conformance/pliron-parity.tsv` manifest must match
/// regeneration byte-exactly, and the trailing guards fail if eligible
/// coverage unexpectedly shrinks or exclusions grow.
#[test]
fn parity_exe_manifest_and_differential() {
    let mut runnable = fixture_sources("assets/ok");
    runnable.extend(fixture_sources("assets/ownership_ok"));
    runnable.extend(fixture_sources("assets/extensions/ok"));
    runnable.extend(fixture_sources("assets/extensions/ownership_ok"));
    let ok_rows = parallel_map(runnable, |(rel, src)| {
        let compiler = Compiler::default();
        let Ok(compiled) = compiler.compile_source(&src, Path::new(&rel)) else {
            // Historical module-scope snippets compile only through the test
            // suite's non-conforming snippet mode.
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "non-conforming-snippet".into(),
            );
        };
        if !compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "main")
        {
            return (
                rel,
                "-".into(),
                "ineligible".to_string(),
                "no-main-entry".into(),
            );
        }
        let mut entries = vec!["main".to_string()];
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "__toplevel__")
        {
            entries.push("__toplevel__".to_string());
        }
        let options = CompileOptions {
            entries,
            sources: vec![(rel.clone(), src)],
            target: host_target(),
            trace_lifecycle: false,
        };
        match native::compile(compiled.elaborated_mir(), &options) {
            Err(error) => {
                let detail = error.display_with_sources(&options.sources);
                (rel, "main".into(), "excluded".to_string(), detail)
            }
            Ok(mut module) => {
                let stdin = fixture_stdin(&rel);
                let vm_output = match stdin {
                    // `input()` fixtures run on a VM with the same bytes the
                    // executables get piped, prompts captured in the output.
                    Some(bytes) => {
                        let mut vm = mojito::backend::VmBackend::new();
                        vm.set_input_override(bytes.to_vec());
                        vm.run_elaborated(compiled.elaborated_mir().clone())
                            .unwrap_or_else(|error| {
                                panic!("{rel}: fixture must run on the VM: {error}")
                            });
                        vm.output()
                    }
                    None => {
                        compiler
                            .execute(&compiled)
                            .unwrap_or_else(|error| {
                                panic!("{rel}: fixture must run on the VM: {error}")
                            })
                            .output
                    }
                };
                let dir = tempfile::tempdir().expect("tempdir");
                let asan_exe = dir.path().join("parity-asan");
                module
                    .write_executable_sanitized(&asan_exe, OptLevel::O0, DebugInfo::Lines)
                    .unwrap_or_else(|error| panic!("{rel}: sanitized emission: {error}"));
                let run = run_executable(&asan_exe, stdin, &[("ASAN_OPTIONS", "detect_leaks=1")]);
                assert_eq!(
                    run.status.code(),
                    Some(0),
                    "{rel}: sanitizer run failed:\n{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                assert!(
                    run.stderr.is_empty(),
                    "{rel}: sanitizer diagnostics:\n{}",
                    String::from_utf8_lossy(&run.stderr)
                );
                for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
                    let exe = dir.path().join(format!("parity-{level}"));
                    module
                        .write_executable(&exe, opt, DebugInfo::Lines)
                        .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
                    let run = run_executable(&exe, stdin, &[]);
                    assert_eq!(
                        run.status.code(),
                        Some(0),
                        "{rel}: exit at {level}: {:?}\nstdout:\n{}\nstderr:\n{}",
                        run.status,
                        String::from_utf8_lossy(&run.stdout),
                        String::from_utf8_lossy(&run.stderr)
                    );
                    assert_eq!(
                        String::from_utf8_lossy(&run.stdout),
                        vm_output,
                        "{rel}: stdout bytes diverge from the VM at {level}"
                    );
                    assert!(
                        run.stderr.is_empty(),
                        "{rel}: stderr must be empty at {level}: {}",
                        String::from_utf8_lossy(&run.stderr)
                    );
                }
                (
                    rel,
                    "main".into(),
                    "exe-differential".to_string(),
                    "sanitized".into(),
                )
            }
        }
    });

    let error_rows = parallel_map(fixture_sources("assets/runtime_error"), |(rel, src)| {
        let compiler = Compiler::default();
        let Ok(compiled) = compiler.compile_source(&src, Path::new(&rel)) else {
            return (
                rel,
                "-".into(),
                "ineligible".into(),
                "non-conforming-snippet".into(),
            );
        };
        let vm_error = match compiler
            .execute(&compiled)
            .expect_err("runtime-error fixture must fail on the VM")
        {
            CompilerError::Runtime(error) => error,
            error => panic!("{rel}: expected a runtime error, got {error}"),
        };
        let category = match &vm_error {
            RuntimeError::Raised(_) => TrapCategory::UnhandledError,
            RuntimeError::Abort(_) => TrapCategory::Abort,
            RuntimeError::TypeError(message) => TrapCategory::from_vm_message(message)
                .unwrap_or_else(|| panic!("{rel}: unmapped VM runtime error: {vm_error}")),
            _ => panic!("{rel}: unmapped VM runtime error: {vm_error}"),
        };
        let vm_error = vm_error.to_string();
        let mut entries = Vec::new();
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "main")
        {
            entries.push("main".to_string());
        }
        if compiled
            .elaborated_mir()
            .functions
            .iter()
            .any(|(name, _)| name == "__toplevel__")
        {
            entries.push("__toplevel__".to_string());
        }
        let entry_detail = entries.join(",");
        let options = CompileOptions {
            entries,
            sources: vec![(rel.clone(), src)],
            target: host_target(),
            trace_lifecycle: false,
        };
        let mut module = native::compile(compiled.elaborated_mir(), &options)
            .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)));
        let dir = tempfile::tempdir().expect("tempdir");
        for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
            let exe = dir.path().join(format!("error-{level}"));
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
                .unwrap_or_else(|error| panic!("{rel}: exe emission at {level}: {error}"));
            let run = run_executable(&exe, fixture_stdin(&rel), &[]);
            assert_eq!(
                run.status.code(),
                Some(i32::from(category.exit_code())),
                "{rel}: runtime-error category diverges at {level}"
            );
            let expected_stderr = match category {
                TrapCategory::UnhandledError | TrapCategory::Abort => format!("{vm_error}\n"),
                _ => format!("mojito runtime trap: {}\n", category.runtime_message()),
            };
            assert_eq!(
                String::from_utf8_lossy(&run.stderr),
                expected_stderr,
                "{rel}: runtime-error stderr diverges at {level}"
            );
        }
        let sanitizer = dir.path().join("error-asan");
        module
            .write_executable_sanitized(&sanitizer, OptLevel::O0, DebugInfo::Lines)
            .unwrap_or_else(|error| panic!("{rel}: sanitized error emission: {error}"));
        let run = run_executable(
            &sanitizer,
            fixture_stdin(&rel),
            &[("ASAN_OPTIONS", "detect_leaks=0")],
        );
        assert_eq!(
            run.status.code(),
            Some(i32::from(category.exit_code())),
            "{rel}: sanitized runtime-error category diverges:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&run.stderr).contains("AddressSanitizer"),
            "{rel}: sanitizer diagnosed native memory misuse:\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
        (
            rel,
            entry_detail,
            "error-differential".to_string(),
            format!("category={category:?}"),
        )
    });
    let rows: Vec<(String, String, String, String)> =
        ok_rows.into_iter().chain(error_rows).collect();

    let mut manifest = String::from(
        "# Pliron native-parity manifest (generated; schema-version 1).\n\
         # One row per assets/ok, assets/ownership_ok, and assets/runtime_error fixture:\n\
         #   fixture <TAB> entry <TAB> status <TAB> detail\n\
         # status: exe-differential (VM/native stdout-byte oracle, O0+O1, ASan/LSan-clean) |\n\
         #         error-differential (VM/native runtime-error category oracle, O0+O1+ASan) |\n\
         #         excluded (native rejection diagnostic) |\n\
         #         ineligible (no runnable `main` shape for this gate)\n\
         # Regenerate: UPDATE_EXPECT=1 CARGO_WORKSPACE_DIR=$PWD \\\n\
         #   cargo nextest run --features backend-pliron parity_exe_manifest\n",
    );
    for (fixture, entry, status, detail) in &rows {
        writeln!(manifest, "{fixture}\t{entry}\t{status}\t{detail}").expect("String write");
    }
    let focused = std::env::var_os("MOJITO_PARITY_ONLY").is_some();
    assert!(
        !(focused && std::env::var_os("UPDATE_EXPECT").is_some()),
        "MOJITO_PARITY_ONLY cannot be combined with UPDATE_EXPECT"
    );
    if !focused {
        expect_test::expect_file!["../../conformance/pliron-parity.tsv"].assert_eq(&manifest);
    }

    // Coverage guards: the eligible sets must never silently shrink, the
    // exclusion count only ratchets down toward the Stage 5 zero-exclusion
    // target, and a pliron-named fixture must never regress to excluded.
    let count = |status: &str| rows.iter().filter(|(_, _, s, _)| s == status).count();
    let differential = count("exe-differential");
    let errors = count("error-differential");
    let excluded = count("excluded");
    if !focused {
        assert!(
            differential == 525,
            "exe-differential coverage must cover the complete runnable inventory: {differential} != 525"
        );
        assert!(
            errors == 34,
            "error-differential coverage must cover every runnable runtime-error fixture: {errors} != 34"
        );
        assert!(
            excluded == 0,
            "native exclusions remain after the zero-exclusion burn-down: {excluded}"
        );
    }
    for (fixture, _, status, detail) in &rows {
        let name = fixture.rsplit('/').next().unwrap_or(fixture);
        assert!(
            !(name.starts_with("pliron_") && status == "excluded"),
            "{fixture}: pliron fixture regressed to excluded: {detail}"
        );
    }
}

/// Negative ownership cases fail in the front end, before any backend runs:
/// the production pipeline rejects them during ownership analysis, so
/// `run --backend pliron` (which compiles through the same pipeline) can
/// never hand them to the native backend.
#[test]
fn negative_ownership_fixtures_fail_before_the_backend() {
    let mut negatives = fixture_sources("assets/ownership_error");
    negatives.extend(fixture_sources("assets/extensions/ownership_error"));
    parallel_map(negatives, |(rel, src)| {
        let compiler = Compiler::default();
        let error = compiler
            .compile_source(&src, Path::new(&rel))
            .err()
            .unwrap_or_else(|| panic!("{rel}: ownership-error fixture must be rejected"));
        // The rejection is a front-end diagnostic, not a backend one.
        let message = error.to_string();
        assert!(
            !message.contains("pliron"),
            "{rel}: rejection unexpectedly reached the native backend: {message}"
        );
    });
}

/// The call-granular correlation premise holds corpus-wide: every runnable
/// fixture attaches with zero degraded functions. An upstream converter
/// change that breaks the premise fails here loudly instead of emitting
/// wrong line numbers.
#[test]
fn pliron_debug_zero_degradations_across_the_corpus() {
    let mut fixtures = fixture_sources("assets/ok");
    fixtures.extend(fixture_sources("assets/ownership_ok"));
    fixtures.extend(fixture_sources("assets/runtime_error"));
    assert!(fixtures.len() > 100, "corpus present");

    let report: Vec<String> = parallel_map(fixtures, |(rel, src)| {
        let compiler = Compiler::default();
        let Ok(compiled) = compiler.compile_source(&src, Path::new(&rel)) else {
            return None;
        };
        let options = CompileOptions {
            entries: vec!["main".to_string()],
            sources: vec![(rel.clone(), src)],
            target: host_target(),
            trace_lifecycle: false,
        };
        // Ineligible for native compilation (no main); the parity manifest
        // pins which ones.
        let module = native::compile(compiled.elaborated_mir(), &options).ok()?;
        let degraded = module
            .debug_degradations()
            .unwrap_or_else(|error| panic!("{rel}: debug attach: {error}"));
        (!degraded.is_empty()).then(|| format!("{rel}: {degraded:?}"))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(
        report.is_empty(),
        "degraded debug correlation:\n{}",
        report.join("\n")
    );
}
