//! Checked native build configuration: target triple, CPU features,
//! optimization level, output kind, and output path.
//!
//! Every knob is a checked type parsed up front — backends never receive raw
//! strings. The supported-target set is deliberately honest: exactly the
//! triples the differential harness runs against, each carrying the pinned
//! LLVM data-layout string that `docs/native-abi.md` specifies and the
//! cross-check tests verify against the installed toolchain.

use std::path::PathBuf;

/// A supported native target triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triple {
    X86_64UnknownLinuxGnu,
}

impl Triple {
    /// Parse a target-triple spelling.
    pub fn parse(s: &str) -> Result<Triple, String> {
        match s {
            "x86_64-unknown-linux-gnu" => Ok(Triple::X86_64UnknownLinuxGnu),
            other => Err(format!(
                "unsupported target triple '{other}' (supported: x86_64-unknown-linux-gnu)"
            )),
        }
    }

    /// The canonical triple spelling stamped on emitted modules.
    pub fn name(self) -> &'static str {
        match self {
            Triple::X86_64UnknownLinuxGnu => "x86_64-unknown-linux-gnu",
        }
    }

    /// The pinned LLVM data-layout string for this triple. Must match what
    /// the pinned toolchain (LLVM 22 clang) produces for [`Triple::name`];
    /// the pliron-lane cross-check test enforces that.
    pub fn data_layout(self) -> &'static str {
        match self {
            Triple::X86_64UnknownLinuxGnu => {
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
            }
        }
    }

    /// The triple of the build host, when it is a supported target.
    pub fn host() -> Option<Triple> {
        if cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) {
            Some(Triple::X86_64UnknownLinuxGnu)
        } else {
            None
        }
    }

    /// Pointer size in bytes.
    pub fn pointer_size(self) -> u64 {
        match self {
            Triple::X86_64UnknownLinuxGnu => 8,
        }
    }

    /// Pointer alignment in bytes.
    pub fn pointer_align(self) -> u64 {
        self.pointer_size()
    }
}

/// Requested CPU feature set. Only the target's baseline feature set is
/// currently supported; the type exists so growing beyond baseline is a
/// checked configuration change rather than a stringly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuFeatures;

impl CpuFeatures {
    pub fn parse(s: &str) -> Result<CpuFeatures, String> {
        match s {
            "" | "baseline" => Ok(CpuFeatures),
            other => Err(format!(
                "unsupported CPU feature set '{other}' (supported: baseline)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        "baseline"
    }
}

/// A fully checked native target: triple plus CPU features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeTarget {
    pub triple: Triple,
    pub cpu: CpuFeatures,
}

impl NativeTarget {
    pub fn new(triple: Triple) -> NativeTarget {
        NativeTarget {
            triple,
            cpu: CpuFeatures,
        }
    }

    /// The build host as a native target, when supported.
    pub fn host() -> Option<NativeTarget> {
        Triple::host().map(NativeTarget::new)
    }
}

/// A complete, checked native build configuration.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub target: NativeTarget,
    pub opt: OptLevel,
    pub emit: EmitKind,
    /// Required whenever [`EmitKind::is_binary`] holds; text kinds print to
    /// stdout when absent.
    pub output: Option<PathBuf>,
}

impl BuildConfig {
    /// Reject configurations whose output kind demands a path but got none.
    pub fn validate(&self) -> Result<(), String> {
        if self.emit.is_binary() && self.output.is_none() {
            return Err(format!(
                "emit kind '{:?}' produces a binary artifact and requires --output",
                self.emit
            ));
        }
        Ok(())
    }
}

/// The native optimization profile. `O0` runs only the backend's baseline
/// cleanup (for Pliron: mem2reg/DCE); `Release` additionally runs LLVM's
/// pinned release pipeline over the emitted bitcode before JIT, object, or
/// executable production. `--native-opt` spells them `0` and `release`,
/// with `1` as a permanent alias of `release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    #[default]
    O0,
    Release,
}

impl OptLevel {
    /// The profile's public CLI spelling.
    pub fn name(self) -> &'static str {
        match self {
            OptLevel::O0 => "0",
            OptLevel::Release => "release",
        }
    }

    pub fn parse(s: &str) -> Result<OptLevel, String> {
        match s {
            "0" => Ok(OptLevel::O0),
            "release" | "1" => Ok(OptLevel::Release),
            other => Err(format!(
                "unknown native opt level '{other}' (expected: 0, release (alias: 1))"
            )),
        }
    }
}

/// How much debug information native binary artifacts carry. `Lines` —
/// the default — attaches DWARF subprograms and call-site file/line
/// locations to objects and executables; textual IR and JIT paths never
/// carry debug information. `--native-debug` spells them `lines`/`none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebugInfo {
    None,
    #[default]
    Lines,
}

impl DebugInfo {
    pub fn parse(s: &str) -> Result<DebugInfo, String> {
        match s {
            "none" => Ok(DebugInfo::None),
            "lines" => Ok(DebugInfo::Lines),
            other => Err(format!(
                "unknown native debug level '{other}' (expected: none, lines)"
            )),
        }
    }
}

/// What a native compilation should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    /// Canonical Pliron textual IR (stdout-friendly).
    Plir,
    /// Textual LLVM IR (stdout-friendly).
    LlvmIr,
    /// LLVM bitcode (requires an output path).
    Bitcode,
    /// A relocatable object file (requires an output path).
    Object,
    /// A linked executable (requires an output path).
    Exe,
}

impl EmitKind {
    pub fn parse(s: &str) -> Result<EmitKind, String> {
        match s {
            "plir" => Ok(EmitKind::Plir),
            "ll" => Ok(EmitKind::LlvmIr),
            "bc" => Ok(EmitKind::Bitcode),
            "obj" => Ok(EmitKind::Object),
            "exe" => Ok(EmitKind::Exe),
            other => Err(format!(
                "unknown emit kind '{other}' (expected: plir, ll, bc, obj, exe)"
            )),
        }
    }

    /// Binary kinds must go to a file; text kinds may print to stdout.
    pub fn is_binary(self) -> bool {
        matches!(self, EmitKind::Bitcode | EmitKind::Object | EmitKind::Exe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_parses_its_own_name() {
        let triple = Triple::X86_64UnknownLinuxGnu;
        assert_eq!(Triple::parse(triple.name()), Ok(triple));
        assert!(Triple::parse("aarch64-apple-darwin").is_err());
        assert!(Triple::parse("").is_err());
    }

    #[test]
    fn cpu_features_accept_only_baseline() {
        assert_eq!(CpuFeatures::parse(""), Ok(CpuFeatures));
        assert_eq!(CpuFeatures::parse("baseline"), Ok(CpuFeatures));
        assert!(CpuFeatures::parse("avx2").is_err());
    }

    #[test]
    fn binary_emit_kinds_require_an_output_path() {
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        for emit in [EmitKind::Bitcode, EmitKind::Object, EmitKind::Exe] {
            let config = BuildConfig {
                target,
                opt: OptLevel::O0,
                emit,
                output: None,
            };
            assert!(config.validate().is_err());
        }
        let config = BuildConfig {
            target,
            opt: OptLevel::O0,
            emit: EmitKind::LlvmIr,
            output: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn opt_and_emit_parse_round_trip() {
        assert_eq!(OptLevel::parse("0"), Ok(OptLevel::O0));
        assert_eq!(OptLevel::parse("release"), Ok(OptLevel::Release));
        assert_eq!(OptLevel::parse("1"), Ok(OptLevel::Release));
        assert!(OptLevel::parse("2").is_err());
        assert_eq!(EmitKind::parse("plir"), Ok(EmitKind::Plir));
        assert_eq!(EmitKind::parse("ll"), Ok(EmitKind::LlvmIr));
        assert_eq!(EmitKind::parse("bc"), Ok(EmitKind::Bitcode));
        assert_eq!(EmitKind::parse("obj"), Ok(EmitKind::Object));
        assert_eq!(EmitKind::parse("exe"), Ok(EmitKind::Exe));
        assert!(EmitKind::parse("asm").is_err());
    }
}
