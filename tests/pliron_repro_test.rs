//! Clean-build reproducibility and artifact-contract inspection (Stage 6,
//! S6.4.6): two fully independent compilations of the same source must
//! produce byte-identical bitcode, objects, link manifests, and
//! executables at both profiles; mismatches print a section-level diff
//! instead of a bare hash. Plus the object-consumer path: `--emit obj` +
//! `mojito link` produces a running executable, and manifest tampering is
//! rejected with an actionable diagnostic; and the mechanical ELF facts
//! the artifact contract pins.

#![cfg(feature = "backend-pliron")]

use std::path::{Path, PathBuf};
use std::process::Command;

use mojito::Compiler;
use mojito::backend::pliron as native;
use mojito::backend::pliron::inspect;
use native::{CompileOptions, DebugInfo, NativeModule, NativeTarget, OptLevel};

const FIXTURE_NAME: &str = "repro_fixture.mojo";

/// Covers strings, calls, traps-adjacent arithmetic, and the runtime —
/// enough surface for reproducibility to be meaningful.
const PROGRAM: &str = "\
def triple(n: Int) -> Int:
    return n * 3

def main():
    var total: Int = 0
    for i in range(50):
        total += triple(i)
    print(total)
    var s = String(\"repro\")
    s += \"-check\"
    print(s, len(s))
";

fn host_target() -> NativeTarget {
    NativeTarget::host().expect("pliron tests require a supported host target")
}

/// A fully fresh compilation: new `Compiler`, new backend module.
fn compile_fresh() -> NativeModule {
    let compiler = Compiler::default();
    let compiled = compiler
        .compile_source(PROGRAM, Path::new(FIXTURE_NAME))
        .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let options = CompileOptions {
        entries: vec!["main".to_string()],
        sources: vec![(FIXTURE_NAME.to_string(), PROGRAM.to_string())],
        target: host_target(),
        trace_lifecycle: false,
    };
    native::compile(compiled.elaborated_mir(), &options)
        .unwrap_or_else(|error| panic!("{}", error.display_with_sources(&options.sources)))
}

fn assert_identical(kind: &str, left: &Path, right: &Path) {
    let left_bytes = std::fs::read(left).expect("left artifact");
    let right_bytes = std::fs::read(right).expect("right artifact");
    if left_bytes == right_bytes {
        return;
    }
    // Name the drifting sections rather than failing on a bare mismatch.
    let diff = match (
        inspect::section_report(left),
        inspect::section_report(right),
    ) {
        (Ok(l), Ok(r)) => inspect::section_diff(&l, &r),
        _ => "  (artifact kind has no section table)\n".to_string(),
    };
    panic!("{kind}: two clean builds differ; drifting sections:\n{diff}");
}

/// Two clean builds match byte-for-byte for every artifact kind at both
/// profiles.
#[test]
fn pliron_repro_clean_builds_are_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (level, opt) in [("O0", OptLevel::O0), ("release", OptLevel::Release)] {
        let build = |tag: &str| -> (PathBuf, PathBuf, PathBuf, PathBuf) {
            let mut module = compile_fresh();
            let bc = dir.path().join(format!("{tag}-{level}.bc"));
            let obj = dir.path().join(format!("{tag}-{level}.o"));
            let exe = dir.path().join(format!("{tag}-{level}"));
            module
                .write_bitcode(&bc, opt, DebugInfo::Lines)
                .expect("bitcode");
            module
                .write_object(&obj, opt, DebugInfo::Lines)
                .expect("object");
            module
                .write_executable(&exe, opt, DebugInfo::Lines)
                .expect("exe");
            let manifest = obj.with_file_name(format!(
                "{}.link.tsv",
                obj.file_name().unwrap().to_string_lossy()
            ));
            (bc, obj, exe, manifest)
        };
        let (bc_a, obj_a, exe_a, man_a) = build("a");
        let (bc_b, obj_b, exe_b, man_b) = build("b");
        assert_identical(&format!("bitcode at {level}"), &bc_a, &bc_b);
        assert_identical(&format!("object at {level}"), &obj_a, &obj_b);
        assert_identical(&format!("executable at {level}"), &exe_a, &exe_b);
        assert_eq!(
            std::fs::read_to_string(&man_a).expect("manifest a"),
            std::fs::read_to_string(&man_b).expect("manifest b"),
            "link manifests at {level}"
        );
    }
}

/// The emitted executable satisfies the artifact contract: x86-64, no
/// executable stack, and no dynamic dependencies beyond the C/math
/// libraries and their loader.
#[test]
fn pliron_repro_executable_satisfies_the_artifact_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut module = compile_fresh();
    let exe = dir.path().join("contract");
    module
        .write_executable(&exe, OptLevel::O0, DebugInfo::Lines)
        .expect("exe");
    let facts = inspect::elf_facts(&exe).expect("elf facts");
    assert_eq!(facts.machine, "x86-64");
    assert!(!facts.executable_stack, "{facts:?}");
    for library in &facts.needed {
        assert!(
            library.starts_with("libc.")
                || library.starts_with("libm.")
                || library.starts_with("libgcc_s.")
                || library.starts_with("ld-linux"),
            "unexpected dynamic dependency {library} in {facts:?}"
        );
    }
    let surface = inspect::exported_runtime_surface(&exe).expect("surface");
    assert!(
        surface.iter().any(|name| name == "mjrt_abi_version"),
        "the ABI anchor symbol must survive linking: {surface:?}"
    );
}

/// The CLI consumer path: compile `--emit obj`, `mojito link` it through
/// the sidecar manifest, and run the produced executable.
#[test]
fn pliron_repro_cli_object_consumer_links_and_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join(FIXTURE_NAME);
    std::fs::write(&source, PROGRAM).expect("write fixture");
    let obj = dir.path().join("consumer.o");
    let compile = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "obj", "-o"])
        .arg(&obj)
        .arg(&source)
        .output()
        .expect("mojito compile runs");
    assert!(
        compile.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let exe = dir.path().join("consumer");
    let link = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["link", "--backend", "pliron"])
        .arg(&obj)
        .args(["-o"])
        .arg(&exe)
        .output()
        .expect("mojito link runs");
    assert!(
        link.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    let run = Command::new(&exe).output().expect("linked exe runs");
    assert_eq!(run.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "3675\nrepro-check 11\n"
    );
}

/// Manifest tampering (digest, ABI version, target) fails with an
/// actionable diagnostic and produces no output file.
#[test]
fn pliron_repro_link_rejects_manifest_mismatches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join(FIXTURE_NAME);
    std::fs::write(&source, PROGRAM).expect("write fixture");
    let obj = dir.path().join("tamper.o");
    let compile = Command::new(env!("CARGO_BIN_EXE_mojito"))
        .args(["compile", "--backend", "pliron", "--emit", "obj", "-o"])
        .arg(&obj)
        .arg(&source)
        .output()
        .expect("mojito compile runs");
    assert!(compile.status.success());
    let manifest_path = dir.path().join("tamper.o.link.tsv");
    let pristine = std::fs::read_to_string(&manifest_path).expect("manifest");

    let cases = [
        ("object-sha256", "object-sha256\t0000", "object-sha256"),
        ("abi-version", "abi-version\t999", "abi-version"),
        ("target", "target\tnot-a-triple", "not-a-triple"),
    ];
    for (name, tampered_row, expected_in_error) in cases {
        let tampered: String = pristine
            .lines()
            .map(|line| {
                if line.starts_with(&format!("{}\t", name)) {
                    tampered_row.to_string()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&manifest_path, tampered + "\n").expect("tamper");
        let exe = dir.path().join(format!("tampered-{name}"));
        let link = Command::new(env!("CARGO_BIN_EXE_mojito"))
            .args(["link", "--backend", "pliron"])
            .arg(&obj)
            .args(["-o"])
            .arg(&exe)
            .output()
            .expect("mojito link runs");
        assert!(!link.status.success(), "{name}: tampering must fail");
        let stderr = String::from_utf8_lossy(&link.stderr);
        assert!(
            stderr.contains(expected_in_error),
            "{name}: diagnostic names the mismatch:\n{stderr}"
        );
        assert!(!exe.exists(), "{name}: no output on failure");
    }
}
