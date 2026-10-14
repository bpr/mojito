//! Stamp the compiler build's identity, `MOJITO_BUILD_ID`: a digest of every
//! compiler source, manifest, the lockfile, and the bundled library's source,
//! with the profile. A compiled package records the identity of the build
//! that wrote it, and a build reads only a package it wrote itself.

use std::path::{Path, PathBuf};

fn main() {
    let root =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let mut files = Vec::new();
    for input in [
        "src",
        "crates",
        "stdlib",
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={input}");
        collect(&root.join(input), &mut files);
    }
    files.sort();
    let mut digest = Fnv::default();
    for file in &files {
        let relative = file.strip_prefix(&root).unwrap_or(file);
        digest.write(relative.to_string_lossy().as_bytes());
        digest.write(&std::fs::read(file).unwrap_or_default());
    }
    let profile = std::env::var("PROFILE").unwrap_or_default();
    println!(
        "cargo:rustc-env=MOJITO_BUILD_ID={}-{:016x}-{profile}",
        env!("CARGO_PKG_VERSION"),
        digest.0
    );
}

/// Every Rust source, manifest, and Mojo source under `path`, skipping build
/// output.
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" {
            continue;
        }
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| matches!(extension.to_str(), Some("rs" | "toml" | "mojo")))
        {
            files.push(path);
        }
    }
}

/// 64-bit FNV-1a, stable across toolchains.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        // Separate fields, so `ab`+`c` and `a`+`bc` differ.
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }
}
