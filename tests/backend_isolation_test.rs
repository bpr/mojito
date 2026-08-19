//! The default build must not link any native backend toolchain: the Pliron
//! spike (spikes/pliron-stage0) is a standalone crate, and `mojito` itself
//! must stay free of `pliron`/`llvm-sys` until an optional feature wires them
//! in deliberately (roadmap section 4; docs/notes/pliron-stage0.md).
//!
//! Once Stage 1 adds `pliron` as an optional dependency, these packages will
//! appear in the lockfile even for default builds; replace this guard with a
//! feature-gated check (e.g. `cargo tree --no-default-features`) then.

const LOCKFILE: &str = include_str!("../Cargo.lock");

#[test]
fn lockfile_contains_no_llvm_or_pliron() {
    for package in ["llvm-sys", "pliron", "pliron-llvm"] {
        let needle = format!("name = \"{package}\"");
        assert!(
            !LOCKFILE.contains(&needle),
            "Cargo.lock unexpectedly contains {package}; \
             the default VM build must need no native toolchain"
        );
    }
}
