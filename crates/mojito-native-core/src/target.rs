//! Checked native build configuration: target triple, target CPU,
//! optimization level, output kind, and output path.
//!
//! Every knob is a checked type parsed up front — backends never receive raw
//! strings. The supported-target set is deliberately honest: exactly the
//! triples the differential harness runs against, each carrying the pinned
//! LLVM data-layout string that `docs/native-abi.md` specifies and the
//! cross-check tests verify against the installed toolchain.

use std::path::PathBuf;

use x86_cpus::CpuModel;

mod x86_cpus;

/// A supported native target triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triple {
    X86_64UnknownLinuxGnu,
}

impl Triple {
    /// Parse a target-triple spelling.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "x86_64-unknown-linux-gnu" => Ok(Self::X86_64UnknownLinuxGnu),
            other => Err(format!(
                "unsupported target triple '{other}' (supported: x86_64-unknown-linux-gnu)"
            )),
        }
    }

    /// The canonical triple spelling stamped on emitted modules.
    pub const fn name(self) -> &'static str {
        match self {
            Self::X86_64UnknownLinuxGnu => "x86_64-unknown-linux-gnu",
        }
    }

    /// The pinned LLVM data-layout string for this triple. Must match what
    /// the pinned toolchain (LLVM 23.1 clang) produces for [`Triple::name`];
    /// the pliron-lane cross-check test enforces that.
    pub const fn data_layout(self) -> &'static str {
        match self {
            Self::X86_64UnknownLinuxGnu => {
                "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
            }
        }
    }

    /// The triple of the build host, when it is a supported target.
    pub const fn host() -> Option<Self> {
        if cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) {
            Some(Self::X86_64UnknownLinuxGnu)
        } else {
            None
        }
    }

    /// Pointer size in bytes.
    pub const fn pointer_size(self) -> u64 {
        match self {
            Self::X86_64UnknownLinuxGnu => 8,
        }
    }

    /// Pointer alignment in bytes.
    pub const fn pointer_align(self) -> u64 {
        self.pointer_size()
    }
}

/// The target CPU: a CPU name from the pin's table, plus the user's
/// `--target-features` edits over that CPU's table features.
///
/// The effective feature list follows the pin's merge rule: each edited name
/// keeps its last sign at its first position; the `+` edits come first in
/// the user's order, then
/// the CPU's table features the user did not name, then the `-` edits in the
/// user's order. No implication is applied (`-sse2` keeps `+sse`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCpu {
    model: &'static CpuModel,
    edits: Vec<(String, bool)>,
}

impl TargetCpu {
    /// The `x86-64` baseline CPU with its table features.
    pub const fn baseline() -> Self {
        Self {
            model: x86_cpus::X86_64,
            edits: Vec::new(),
        }
    }

    /// Parse `--target-cpu` and `--target-features`. An absent CPU is the
    /// default, `x86-64`; the features start from the chosen CPU's table.
    /// A feature item is `+name`, `-name`, or a bare `name` (enabled), and
    /// empty items are skipped.
    pub fn parse(cpu: Option<&str>, features: Option<&str>) -> Result<Self, String> {
        let model = match cpu {
            None => x86_cpus::X86_64,
            Some(name) => x86_cpus::X86_CPUS
                .binary_search_by(|model| model.name.cmp(name))
                .map(|index| &x86_cpus::X86_CPUS[index])
                .map_err(|_| format!("unknown target CPU '{name}'"))?,
        };
        let mut edits: Vec<(String, bool)> = Vec::new();
        for item in features
            .unwrap_or("")
            .split(',')
            .filter(|item| !item.is_empty())
        {
            let (name, enabled) = item.strip_prefix('-').map_or_else(
                || (item.strip_prefix('+').unwrap_or(item), true),
                |name| (name, false),
            );
            match edits.iter_mut().find(|(edited, _)| edited == name) {
                Some(edit) => edit.1 = enabled,
                None => edits.push((name.to_string(), enabled)),
            }
        }
        Ok(Self { model, edits })
    }

    /// The CPU's LLVM name, stamped as `"target-cpu"`.
    pub const fn name(&self) -> &'static str {
        self.model.name
    }

    /// The effective feature list, each item `+name` or `-name`.
    pub fn features(&self) -> Vec<String> {
        let enabled = self
            .edits
            .iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(name, _)| format!("+{name}"));
        let table = self
            .model
            .features
            .iter()
            .filter(|feature| !self.edits.iter().any(|(name, _)| name == *feature))
            .map(|feature| format!("+{feature}"));
        let disabled = self
            .edits
            .iter()
            .filter(|(_, enabled)| !*enabled)
            .map(|(name, _)| format!("-{name}"));
        enabled.chain(table).chain(disabled).collect()
    }

    /// The effective feature list joined with `,`, stamped as
    /// `"target-features"`.
    pub fn features_string(&self) -> String {
        self.features().join(",")
    }

    /// Whether the effective list enables `name`: literal `+name`
    /// membership, as the pin's `target_has_feature` answers.
    pub fn has_feature(&self, name: &str) -> bool {
        self.edits
            .iter()
            .find(|(edited, _)| edited == name)
            .map_or_else(
                || self.model.features.contains(&name),
                |(_, enabled)| *enabled,
            )
    }

    /// The edited feature names LLVM does not know. The pin warns about
    /// each and keeps it in the list.
    pub fn unknown_features(&self) -> impl Iterator<Item = (&str, bool)> {
        self.edits
            .iter()
            .filter(|(name, _)| {
                x86_cpus::LLVM_X86_FEATURES
                    .binary_search(&name.as_str())
                    .is_err()
            })
            .map(|(name, enabled)| (name.as_str(), *enabled))
    }
}

impl Default for TargetCpu {
    fn default() -> Self {
        Self::baseline()
    }
}

/// A fully checked native target: triple plus target CPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTarget {
    pub triple: Triple,
    pub cpu: TargetCpu,
}

impl NativeTarget {
    pub const fn new(triple: Triple) -> Self {
        Self {
            triple,
            cpu: TargetCpu::baseline(),
        }
    }

    /// The build host as a native target, when supported.
    pub fn host() -> Option<Self> {
        Triple::host().map(Self::new)
    }

    /// This target with `cpu` as its target CPU.
    #[must_use]
    pub fn with_cpu(self, cpu: TargetCpu) -> Self {
        Self { cpu, ..self }
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

/// The native optimization profile.
///
/// `O0` runs only the backend's baseline cleanup (for Pliron: mem2reg/DCE);
/// `Release` additionally runs LLVM's pinned release pipeline over the emitted
/// bitcode before JIT, object, or executable production. `--native-opt` spells
/// them `0` and `release`, with `1` as a permanent alias of `release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    #[default]
    O0,
    Release,
}

impl OptLevel {
    /// The profile's public CLI spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::O0 => "0",
            Self::Release => "release",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "0" => Ok(Self::O0),
            "release" | "1" => Ok(Self::Release),
            other => Err(format!(
                "unknown native opt level '{other}' (expected: 0, release (alias: 1))"
            )),
        }
    }
}

/// How much debug information native binary artifacts carry.
///
/// `Lines` — the default — attaches DWARF subprograms and call-site file/line
/// locations to objects and executables; textual IR and JIT paths never carry
/// debug information. `--native-debug` spells them `lines`/`none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebugInfo {
    None,
    #[default]
    Lines,
}

impl DebugInfo {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "none" => Ok(Self::None),
            "lines" => Ok(Self::Lines),
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
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "plir" => Ok(Self::Plir),
            "ll" => Ok(Self::LlvmIr),
            "bc" => Ok(Self::Bitcode),
            "obj" => Ok(Self::Object),
            "exe" => Ok(Self::Exe),
            other => Err(format!(
                "unknown emit kind '{other}' (expected: plir, ll, bc, obj, exe)"
            )),
        }
    }

    /// Binary kinds must go to a file; text kinds may print to stdout.
    pub const fn is_binary(self) -> bool {
        matches!(self, Self::Bitcode | Self::Object | Self::Exe)
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

    fn features(cpu: &str, features: &str) -> String {
        TargetCpu::parse(Some(cpu), Some(features))
            .expect("pin CPU")
            .features_string()
    }

    #[test]
    fn target_cpu_merges_feature_edits_as_the_pin_does() {
        const X86_64: &str = "+cmov,+cx8,+fxsr,+mmx,+sse,+sse2,+x87";
        assert_eq!(TargetCpu::baseline().features_string(), X86_64);
        assert_eq!(features("x86-64", ""), X86_64);
        assert_eq!(
            features("x86-64", "+sse3,+avx,-mmx,-cx8"),
            "+sse3,+avx,+cmov,+fxsr,+sse,+sse2,+x87,-mmx,-cx8"
        );
        assert_eq!(
            features("x86-64", "-mmx,+avx,-cx8,+cmov"),
            "+avx,+cmov,+fxsr,+sse,+sse2,+x87,-mmx,-cx8"
        );
        assert_eq!(features("x86-64", "sse3"), format!("+sse3,{X86_64}"));
        assert_eq!(features("x86-64", "+avx,+avx"), format!("+avx,{X86_64}"));
        assert_eq!(features("x86-64", "+avx,"), format!("+avx,{X86_64}"));
        assert_eq!(
            features("x86-64", "+avx512f,-avx512f"),
            format!("{X86_64},-avx512f")
        );
        let skylake = "+adx,+aes,+avx,+avx2,+bmi,+bmi2,+clflushopt,+cmov,+crc32,+cx16,+cx8,+f16c,+fma,+fsgsbase,+fxsr,+invpcid,+lzcnt,+mmx,+movbe,+pclmul,+popcnt,+prfchw,+rdrnd,+rdseed,+sahf,+sgx,+sse,+sse2,+sse3,+sse4.1,+sse4.2,+ssse3,+x87,+xsave,+xsavec,+xsaveopt,+xsaves";
        assert_eq!(features("skylake", ""), skylake);
        assert_eq!(
            features("skylake", "+avx2"),
            format!("+avx2,{}", skylake.replace(",+avx2", ""))
        );
        assert_eq!(
            features("skylake", "-avx2"),
            format!("{},-avx2", skylake.replace(",+avx2", ""))
        );
        assert_eq!(features("skylake", "+bogus"), format!("+bogus,{skylake}"));
        assert_eq!(
            features("x86-64", "-avx512f,+amx-tile,avx512f,-sse"),
            "+avx512f,+amx-tile,+cmov,+cx8,+fxsr,+mmx,+sse2,+x87,-sse"
        );
    }

    #[test]
    fn target_cpu_rejects_unknown_cpus_and_reports_unknown_features() {
        assert_eq!(
            TargetCpu::parse(Some("bogus"), None),
            Err("unknown target CPU 'bogus'".to_string())
        );
        assert!(TargetCpu::parse(Some("i686"), None).is_err());
        assert_eq!(TargetCpu::parse(None, None), Ok(TargetCpu::baseline()));
        let cpu = TargetCpu::parse(Some("skylake"), Some("+bogus,-avx2,-alsobad")).unwrap();
        assert_eq!(cpu.name(), "skylake");
        assert_eq!(
            cpu.unknown_features().collect::<Vec<_>>(),
            [("bogus", true), ("alsobad", false)]
        );
    }

    #[test]
    fn target_cpu_has_feature_is_literal_membership() {
        let cpu = TargetCpu::parse(Some("x86-64"), Some("+avx2,+bogus,-sse2")).unwrap();
        assert!(cpu.has_feature("avx2"));
        assert!(!cpu.has_feature("avx"));
        assert!(cpu.has_feature("bogus"));
        assert!(cpu.has_feature("sse"));
        assert!(!cpu.has_feature("sse2"));
        assert!(!cpu.has_feature("64bit"));
    }

    #[test]
    fn binary_emit_kinds_require_an_output_path() {
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        for emit in [EmitKind::Bitcode, EmitKind::Object, EmitKind::Exe] {
            let config = BuildConfig {
                target: target.clone(),
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
