//! Resolved-toolchain reporting and validation for the Pliron backend:
//! the `toolchain_report` key surface, the `--print-toolchain` CLI flag,
//! runtime-archive validation (ABI version, missing-file diagnostics), and
//! failure-atomicity of executable emission observed end to end.
//!
//! Like the rest of the pliron lane, these tests assume the development
//! runtime archive is discoverable (`MOJITO_RUNTIME_LIB` or a built
//! `target/debug/libmojito_runtime.a`) and clang/opt 22 are installed.

use std::process::Command;

use mojito::backend::pliron::{NativeTarget, OptLevel, toolchain_report};
use mojito::native::rt_abi;
use mojito::native::target::Triple;

fn host_target() -> NativeTarget {
    NativeTarget::new(Triple::X86_64UnknownLinuxGnu)
}

/// The report's leading keys are fixed policy; values vary by machine.
#[test]
fn pliron_toolchain_report_leads_with_stable_keys() {
    let report = toolchain_report(&host_target(), OptLevel::O0);
    let keys: Vec<&str> = report
        .lines()
        .map(|line| line.split('\t').next().unwrap())
        .collect();
    assert_eq!(
        &keys[..4],
        ["target", "data-layout", "profile", "llvm-pipeline"],
        "report:\n{report}"
    );
    assert!(
        report.contains("target\tx86_64-unknown-linux-gnu\n"),
        "report:\n{report}"
    );
    assert!(report.contains("profile\t0\n"), "report:\n{report}");
    assert!(
        report.contains("llvm-pipeline\t(none)\n"),
        "report:\n{report}"
    );
}

/// The release profile names its LLVM pipeline and reports the `opt` tool.
#[test]
fn pliron_toolchain_report_release_names_pipeline_and_opt() {
    let report = toolchain_report(&host_target(), OptLevel::Release);
    assert!(
        report.contains("llvm-pipeline\tdefault<O1>\n"),
        "report:\n{report}"
    );
    assert!(report.contains("profile\trelease\n"), "report:\n{report}");
    assert!(
        report.lines().any(|line| line.starts_with("opt\t")),
        "report:\n{report}"
    );
}

/// On this pinned lane the tools and runtime resolve, versions match the
/// LLVM 22 pin, and the archive's embedded ABI version matches the
/// compiler-side contract table.
#[test]
fn pliron_toolchain_report_resolves_the_pinned_lane() {
    let report = toolchain_report(&host_target(), OptLevel::Release);
    let value = |key: &str| -> Option<&str> {
        report
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}\t")))
    };
    let clang_version = value("clang-version").unwrap_or_else(|| {
        panic!("clang did not resolve:\n{report}");
    });
    assert!(clang_version.contains("22."), "{clang_version}");
    let opt_version = value("opt-version").unwrap_or_else(|| {
        panic!("opt did not resolve:\n{report}");
    });
    assert!(opt_version.contains("22."), "{opt_version}");
    assert_eq!(
        value("runtime-abi-version"),
        Some(rt_abi::MJRT_ABI_VERSION.to_string().as_str()),
        "report:\n{report}"
    );
    let sha256 = value("runtime-sha256").expect("runtime digest present");
    assert_eq!(sha256.len(), 64, "{sha256}");
    assert!(sha256.chars().all(|c| c.is_ascii_hexdigit()), "{sha256}");
}

/// `--print-toolchain` reports and exits 0 without reading any input.
#[test]
fn pliron_cli_print_toolchain_reports_without_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--print-toolchain"])
        .output()
        .expect("mojito runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("target\tx86_64-unknown-linux-gnu\n"),
        "{stdout}"
    );
}

/// The flag stays compile-only.
#[test]
fn pliron_cli_print_toolchain_rejects_other_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["run", "--print-toolchain"])
        .output()
        .expect("mojito runs");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--print-toolchain is only valid with the compile command"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A bad `MOJITO_RUNTIME_LIB` fails executable emission before any frontend
/// work, names the missing path, and leaves an existing output untouched.
#[test]
fn pliron_cli_exe_emission_with_missing_runtime_fails_early_and_preserves_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("out");
    std::fs::write(&dest, b"prior artifact").expect("seed prior output");
    let missing = dir.path().join("no-such-libmojito_runtime.a");
    let output = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "exe", "-o"])
        .arg(&dest)
        .arg("-")
        .env("MOJITO_RUNTIME_LIB", &missing)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("mojito runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("MOJITO_RUNTIME_LIB points at a missing file"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("no-such-libmojito_runtime.a"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read(&dest).expect("prior output survives"),
        b"prior artifact"
    );
}
