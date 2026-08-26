//! Distribution contract tests (Stage 6, S6.5): installation-relative
//! runtime discovery from a relocated bundle layout (paths with spaces and
//! non-ASCII included), the `--runtime-lib` precedence step, compiler-free
//! execution of emitted executables in an empty environment, and early
//! actionable rejection of missing, corrupt, and ABI-mismatched runtimes.
//!
//! These tests assemble a fake bundle from development artifacts; the real
//! bundle is produced by `scripts/package-pliron`, whose own smoke check
//! covers the packaged binary.

#![cfg(feature = "backend-pliron")]

use std::path::{Path, PathBuf};
use std::process::Command;

use mojito::backend::pliron::inspect;

const PROGRAM: &str = "\
def main():
    print(\"relocated\", 6 * 7)
";

/// The development runtime archive backing the fake bundles.
fn dev_runtime() -> PathBuf {
    if let Ok(path) = std::env::var("MOJITO_RUNTIME_LIB") {
        return PathBuf::from(path);
    }
    let exe = std::env::current_exe().expect("test exe path");
    for dir in exe.ancestors().skip(1).take(4) {
        let candidate = dir.join("libmojito_runtime.a");
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("no development runtime archive; build with `cargo build -p mojito-runtime`");
}

/// A bundle-shaped directory: `bin/mojito` + `lib/libmojito_runtime.a`.
fn fake_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("mojito-bundle");
    std::fs::create_dir_all(bundle.join("bin")).expect("bin dir");
    std::fs::create_dir_all(bundle.join("lib")).expect("lib dir");
    std::fs::copy(env!("CARGO_BIN_EXE_mojito"), bundle.join("bin/mojito")).expect("copy compiler");
    std::fs::copy(dev_runtime(), bundle.join("lib/libmojito_runtime.a")).expect("copy runtime");
    bundle
}

/// Relocated-bundle discovery: with no env override, the bundle-relative
/// `../lib` step finds the runtime — from a path with spaces and non-ASCII
/// — and the emitted executable runs in a fully empty environment with no
/// dynamic dependencies beyond libc/libm and the loader.
#[test]
fn pliron_dist_relocated_bundle_compiles_and_runs_compiler_free() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = fake_bundle(&dir.path().join("réloc dir with spaces"));
    let source = dir.path().join("hello.mojo");
    std::fs::write(&source, PROGRAM).expect("fixture");
    let exe = dir.path().join("hello");
    let compile = Command::new(bundle.join("bin/mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "exe", "-o"])
        .arg(&exe)
        .arg(&source)
        .env_remove("MOJITO_RUNTIME_LIB")
        .output()
        .expect("bundle compiler runs");
    assert!(
        compile.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Provenance is reported as the installation bundle.
    let report = Command::new(bundle.join("bin/mojito"))
        .args(["compile", "--backend", "pliron", "--print-toolchain"])
        .env_remove("MOJITO_RUNTIME_LIB")
        .output()
        .expect("print-toolchain runs");
    let report = String::from_utf8_lossy(&report.stdout).into_owned();
    assert!(
        report.contains("runtime-provenance\tinstallation bundle"),
        "{report}"
    );

    // Compiler-free execution: empty environment, nothing from the bundle.
    let run = Command::new(&exe)
        .env_clear()
        .output()
        .expect("relocated exe runs");
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "relocated 42\n");
    let facts = inspect::elf_facts(&exe).expect("elf facts");
    for library in &facts.needed {
        assert!(
            library.starts_with("libc.")
                || library.starts_with("libm.")
                || library.starts_with("libgcc_s.")
                || library.starts_with("ld-linux"),
            "unexpected dynamic dependency {library}"
        );
    }
}

/// With the development tree hidden behind a private mount namespace, the
/// bundle's `share/mojito/stdlib` serves the prelude — the relocation
/// proof for the compiler's bundled-support fallback. Skips where
/// unprivileged user namespaces are unavailable.
#[test]
fn pliron_dist_bundle_ships_a_usable_stdlib() {
    let namespaces_work = Command::new("unshare")
        .args(["-rm", "true"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !namespaces_work {
        eprintln!("skipping: unprivileged user namespaces unavailable");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = fake_bundle(dir.path());
    let dev_stdlib = Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib");
    copy_tree(&dev_stdlib, &bundle.join("share/mojito/stdlib"));
    let source = dir.path().join("uses_stdlib.mojo");
    std::fs::write(
        &source,
        "def main():\n    var xs: List[Int] = [1, 2, 3]\n    print(len(xs))\n",
    )
    .expect("fixture");
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).expect("empty dir");
    let script = format!(
        "mount --bind '{}' '{}' && exec '{}' run '{}'",
        empty.display(),
        dev_stdlib.display(),
        bundle.join("bin/mojito").display(),
        source.display(),
    );
    let run = Command::new("unshare")
        .args(["-rm", "sh", "-c", &script])
        .env_remove("MOJITO_RUNTIME_LIB")
        .output()
        .expect("unshare runs");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n");
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// `--runtime-lib` outranks the environment and reports its provenance;
/// pointing it at a missing file is a named hard error, not a fallthrough.
#[test]
fn pliron_dist_runtime_lib_flag_takes_precedence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("copied-runtime.a");
    std::fs::copy(dev_runtime(), &copy).expect("copy runtime");
    let report = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args([
            "compile",
            "--backend",
            "pliron",
            "--print-toolchain",
            "--runtime-lib",
        ])
        .arg(&copy)
        .env(
            "MOJITO_RUNTIME_LIB",
            "/nonexistent/ignored-because-flag-wins",
        )
        .output()
        .expect("print-toolchain runs");
    let text = String::from_utf8_lossy(&report.stdout).into_owned();
    assert!(text.contains("runtime-provenance\t--runtime-lib"), "{text}");

    let missing = dir.path().join("no-such.a");
    let source = dir.path().join("hello.mojo");
    std::fs::write(&source, PROGRAM).expect("fixture");
    let compile = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "exe", "-o"])
        .arg(dir.path().join("out"))
        .arg(&source)
        .arg("--runtime-lib")
        .arg(&missing)
        .output()
        .expect("compile runs");
    assert!(!compile.status.success());
    let stderr = String::from_utf8_lossy(&compile.stderr);
    assert!(
        stderr.contains("--runtime-lib points at a missing file") && stderr.contains("no-such.a"),
        "stderr: {stderr}"
    );
}

/// A corrupt runtime archive fails before any lowering, naming the file
/// and the remedy.
#[test]
fn pliron_dist_corrupt_runtime_is_rejected_early() {
    let dir = tempfile::tempdir().expect("tempdir");
    let corrupt = dir.path().join("libmojito_runtime.a");
    std::fs::write(&corrupt, b"definitely not an archive").expect("corrupt archive");
    let source = dir.path().join("hello.mojo");
    std::fs::write(&source, PROGRAM).expect("fixture");
    let compile = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "exe", "-o"])
        .arg(dir.path().join("out"))
        .arg(&source)
        .arg("--runtime-lib")
        .arg(&corrupt)
        .output()
        .expect("compile runs");
    assert!(!compile.status.success());
    let stderr = String::from_utf8_lossy(&compile.stderr);
    assert!(
        stderr.contains("not a static archive") && stderr.contains("cargo build -p mojito-runtime"),
        "stderr: {stderr}"
    );
}

/// An archive whose embedded `mjrt_abi_version` disagrees with this
/// compiler is rejected with both versions named. The fixture archive is
/// built with the lane's clang + ar from one C definition.
#[test]
fn pliron_dist_abi_mismatched_runtime_is_rejected() {
    let ar = ["llvm-ar-22", "llvm-ar", "ar"]
        .into_iter()
        .find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
        })
        .expect("an ar tool (the pliron lane ships llvm-ar-22)");
    let clang = ["clang-22", "clang"]
        .into_iter()
        .find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
        })
        .expect("clang");

    let dir = tempfile::tempdir().expect("tempdir");
    let c_src = dir.path().join("abi.c");
    std::fs::write(&c_src, "unsigned int mjrt_abi_version = 999;\n").expect("c source");
    let object = dir.path().join("abi.o");
    let compiled = Command::new(clang)
        .arg("-c")
        .arg(&c_src)
        .arg("-o")
        .arg(&object)
        .output()
        .expect("clang runs");
    assert!(compiled.status.success());
    let archive = dir.path().join("libmojito_runtime.a");
    let archived = Command::new(ar)
        .arg("rcs")
        .arg(&archive)
        .arg(&object)
        .output()
        .expect("ar runs");
    assert!(archived.status.success());

    let source = dir.path().join("hello.mojo");
    std::fs::write(&source, PROGRAM).expect("fixture");
    let compile = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "exe", "-o"])
        .arg(dir.path().join("out"))
        .arg(&source)
        .arg("--runtime-lib")
        .arg(&archive)
        .output()
        .expect("compile runs");
    assert!(!compile.status.success());
    let stderr = String::from_utf8_lossy(&compile.stderr);
    assert!(
        stderr.contains("ABI version 999") && stderr.contains("requires 6"),
        "stderr: {stderr}"
    );
}
