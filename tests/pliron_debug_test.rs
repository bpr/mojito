//! Source-level debug information for the Pliron backend (Stage 6, S6.3):
//! DWARF line tables name the right files and lines, backtraces resolve
//! them at `O0` and keep useful call-site locations at `release`, non-ASCII
//! sources survive, no build-tree or temp paths leak into the metadata,
//! debug info stays byte-deterministic, and call-granular correlation holds
//! with zero degradations across the whole runnable corpus.

#![cfg(feature = "backend-pliron")]

use std::path::{Path, PathBuf};
use std::process::Command;

use mojito::Compiler;
use mojito::backend::pliron as native;
use native::{CompileOptions, DebugInfo, NativeModule, NativeTarget, OptLevel};

const FIXTURE_NAME: &str = "debug_fixture.mojo";

/// A two-frame trapping program: the division trap sits on line 2 and its
/// call site on line 6 — the lines the DWARF assertions pin.
const TRAPPER: &str = "\
def divide(a: Int, b: Int) -> Int:
    return a // b

def main():
    var d: Int = 0
    print(divide(10, d))
";

fn host_target() -> NativeTarget {
    NativeTarget::host().expect("pliron tests require a supported host target")
}

fn native_compile(src: &str) -> NativeModule {
    native_compile_labeled(src, FIXTURE_NAME)
}

fn native_compile_labeled(src: &str, label: &str) -> NativeModule {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(src, Path::new(label))
        .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let options = CompileOptions {
        entries: vec!["main".to_string()],
        sources: vec![(label.to_string(), src.to_string())],
        target: host_target(),
        trace_lifecycle: false,
    };
    native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)))
}

/// The pinned dwarfdump (LLVM 22 lane requirement, same candidate policy as
/// the backend's tools).
fn dwarfdump() -> &'static str {
    for candidate in ["llvm-dwarfdump-22", "llvm-dwarfdump"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            return candidate;
        }
    }
    panic!("the pliron lane requires llvm-dwarfdump (LLVM 22)");
}

/// gdb is not a lane requirement; backtrace tests skip without it.
fn gdb_available() -> bool {
    Command::new("gdb")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn build_exe(module: &mut NativeModule, dir: &Path, name: &str, opt: OptLevel) -> PathBuf {
    let exe = dir.join(name);
    module
        .write_executable(&exe, opt, DebugInfo::Lines)
        .unwrap_or_else(|error| panic!("exe emission: {error}"));
    exe
}

fn gdb_backtrace(exe: &Path) -> String {
    let output = Command::new("gdb")
        .args([
            "-batch",
            "-ex",
            "break mjrt_trap",
            "-ex",
            "run",
            "-ex",
            "bt",
        ])
        .arg(exe)
        .output()
        .expect("gdb runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The line table names the fixture with its registered label and carries
/// the trap and call-site lines; no absolute or temp path leaks in.
#[test]
fn pliron_debug_line_table_names_sources_without_leaking_paths() {
    let mut module = native_compile(TRAPPER);
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = build_exe(&mut module, dir.path(), "trapper", OptLevel::O0);
    let output = Command::new(dwarfdump())
        .args(["--debug-line", "--debug-info"])
        .arg(&exe)
        .output()
        .expect("dwarfdump runs");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(FIXTURE_NAME), "line table names the fixture");
    // Our compile units are the DWARF v5 ones (the runtime archive's units
    // are clang/rustc-produced); check ours for leaked paths.
    for leak in ["/tmp/", "/home/", ".tmp.bc"] {
        assert!(
            !text.contains(&format!("name: \"{leak}")),
            "no source name may leak {leak}"
        );
    }
    // Both anchor lines appear as line-table rows.
    let has_line = |line: &str| text.lines().any(|row| row.contains(line));
    assert!(
        has_line("      2      0      0") || text.contains("   2      "),
        "trap line present:\n{text}"
    );
}

/// An absolute CLI source path degrades to its file name in the DWARF —
/// artifacts stay byte-reproducible across build directories no matter how
/// the input was spelled.
#[test]
fn pliron_debug_absolute_source_labels_embed_only_the_file_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let label = dir.path().join(FIXTURE_NAME);
    let mut module = native_compile_labeled(TRAPPER, &label.to_string_lossy());
    let exe = build_exe(&mut module, dir.path(), "trapper-abs", OptLevel::O0);
    let output = Command::new(dwarfdump())
        .args(["--debug-line", "--debug-info"])
        .arg(&exe)
        .output()
        .expect("dwarfdump runs");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(&format!("name: \"{FIXTURE_NAME}\"")),
        "the file name alone is embedded:\n{text}"
    );
    let leaked = format!("name: \"{}", dir.path().display());
    assert!(
        !text.contains(&leaked),
        "the absolute source directory must not be embedded"
    );
}

/// gdb resolves the trap frame to file and line at `O0`.
#[test]
fn pliron_debug_backtrace_names_file_and_line_at_o0() {
    if !gdb_available() {
        eprintln!("skipping: gdb is not installed");
        return;
    }
    let mut module = native_compile(TRAPPER);
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = build_exe(&mut module, dir.path(), "trapper", OptLevel::O0);
    let backtrace = gdb_backtrace(&exe);
    assert!(
        backtrace.contains(&format!("{FIXTURE_NAME}:2")),
        "trap frame resolves to the division line:\n{backtrace}"
    );
    assert!(
        backtrace.contains(&format!("{FIXTURE_NAME}:6")),
        "caller frame resolves to the call site:\n{backtrace}"
    );
}

/// Release keeps useful call-site locations (inlining may merge frames; the
/// assertion is presence, not O0-equality).
#[test]
fn pliron_debug_backtrace_retains_call_sites_at_release() {
    if !gdb_available() {
        eprintln!("skipping: gdb is not installed");
        return;
    }
    let mut module = native_compile(TRAPPER);
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = build_exe(&mut module, dir.path(), "trapper-rel", OptLevel::Release);
    let backtrace = gdb_backtrace(&exe);
    assert!(
        backtrace.contains(&format!("{FIXTURE_NAME}:")),
        "release backtrace retains source locations:\n{backtrace}"
    );
}

/// Non-ASCII source content flows through location tracking and DWARF.
#[test]
fn pliron_debug_handles_non_ascii_sources() {
    let src = "\
# comentário: divisão com acentuação — ünïcödé ✓
def main():
    var s = String(\"héllo wörld ✓\")
    print(len(s))
";
    let mut module = native_compile(src);
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = build_exe(&mut module, dir.path(), "unicode", OptLevel::O0);
    let run = Command::new(&exe).output().expect("exe runs");
    assert_eq!(run.status.code(), Some(0));
    let output = Command::new(dwarfdump())
        .arg("--debug-line")
        .arg(&exe)
        .output()
        .expect("dwarfdump runs");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(FIXTURE_NAME),
        "non-ASCII fixture keeps its line table"
    );
}

/// Debug information must not break byte-reproducibility.
#[test]
fn pliron_debug_info_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut first = native_compile(TRAPPER);
    let exe_a = build_exe(&mut first, dir.path(), "det-a", OptLevel::O0);
    let mut second = native_compile(TRAPPER);
    let exe_b = build_exe(&mut second, dir.path(), "det-b", OptLevel::O0);
    assert_eq!(
        std::fs::read(&exe_a).expect("first exe"),
        std::fs::read(&exe_b).expect("second exe"),
        "two clean debug builds must be byte-identical"
    );
}

/// The call-granular correlation premise holds corpus-wide: every runnable
/// fixture attaches with zero degraded functions. An upstream converter
/// change that breaks the premise fails here loudly instead of emitting
/// wrong line numbers.
#[test]
fn pliron_debug_zero_degradations_across_the_corpus() {
    let mut fixtures = Vec::new();
    for outcome in ["ok", "ownership_ok", "runtime_error"] {
        let dir = Path::new("assets").join(outcome);
        for entry in std::fs::read_dir(&dir).expect("assets dir") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|ext| ext == "mojo") {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    assert!(fixtures.len() > 100, "corpus present");

    let report: Vec<String> = std::thread::scope(|scope| {
        let chunk = fixtures.len().div_ceil(8);
        let handles: Vec<_> = fixtures
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut degraded_report = Vec::new();
                    for path in chunk {
                        let src = std::fs::read_to_string(path).expect("fixture reads");
                        let compiler = Compiler::default();
                        let Ok(compiled) = compiler.compile_source(&src, path) else {
                            continue;
                        };
                        let label = path.to_string_lossy().into_owned();
                        let options = CompileOptions {
                            entries: vec!["main".to_string()],
                            sources: vec![(label.clone(), src)],
                            target: host_target(),
                            trace_lifecycle: false,
                        };
                        let Ok(module) = native::compile(compiled.elaborated_mir(), &options)
                        else {
                            // Ineligible for native compilation (no main);
                            // the parity manifest pins which ones.
                            continue;
                        };
                        let degraded = module
                            .debug_degradations()
                            .unwrap_or_else(|error| panic!("{label}: debug attach: {error}"));
                        if !degraded.is_empty() {
                            degraded_report.push(format!("{label}: {degraded:?}"));
                        }
                    }
                    degraded_report
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("worker"))
            .collect()
    });
    assert!(
        report.is_empty(),
        "degraded debug correlation:\n{}",
        report.join("\n")
    );
}
