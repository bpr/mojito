//! The default build must not link any native backend toolchain. `pliron`,
//! `pliron-llvm`, and (transitively) `llvm-sys` exist in the lockfile as
//! optional dependencies behind the `backend-pliron` feature, so this guard
//! asserts the *resolved default dependency graph* — not the lockfile —
//! stays free of them (roadmap section 4; docs/notes/pliron-stage1.md).

use std::process::Command;

#[test]
fn default_feature_graph_contains_no_llvm_or_pliron() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "tree",
            "-p",
            "mojito",
            "--locked",
            "--offline",
            "--no-default-features",
            "-e",
            "no-dev",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("RUSTC_WRAPPER", "")
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout);
    for package in ["llvm-sys", "pliron", "pliron-llvm"] {
        let hit = tree
            .lines()
            .any(|line| line.trim_start().starts_with(&format!("{package} v")));
        assert!(
            !hit,
            "default dependency graph unexpectedly contains {package}; \
             the default VM build must need no native toolchain:\n{tree}"
        );
    }
}
