//! The resolved external toolchain: exact clang/`opt` paths and versions,
//! the target and its pinned data layout, and the runtime archive with its
//! provenance, digest, and embedded ABI version. Resolution happens before
//! any tool runs, so a missing or incompatible component fails early with
//! the expected and found values named, and every later invocation uses the
//! recorded absolute path rather than re-discovering tools mid-build.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::native::rt_abi;
use crate::native::target::{EmitKind, NativeTarget};

use super::pipeline::Pipeline;
use super::{OptLevel, PlironError, PlironErrorKind};

/// The LLVM major version every external tool must match — the same pin as
/// the in-process `llvm-sys = "221"` dependency (LLVM 22).
pub(super) const EXPECTED_LLVM_MAJOR: u32 = 22;

/// A human-readable report of everything `resolve` would use, one stable
/// `key\tvalue` line per component. Never fails: a missing or incompatible
/// component reports its error text as the value, so the report is usable
/// exactly when something is wrong. The CLI's `--print-toolchain` surface.
pub fn toolchain_report(target: &NativeTarget, profile: OptLevel) -> String {
    let pipeline = Pipeline::for_profile(profile);
    let mut out = String::new();
    let mut line = |key: &str, value: &str| {
        out.push_str(&format!("{key}\t{value}\n"));
    };
    line("target", target.triple.name());
    line("data-layout", target.triple.data_layout());
    line("profile", profile.name());
    line(
        "llvm-pipeline",
        pipeline.llvm_pipeline().unwrap_or("(none)"),
    );
    match find_tool_cached("clang", CLANG_CANDIDATES, &CLANG_CACHE) {
        Ok(tool) => {
            line("clang", &tool.path.display().to_string());
            line("clang-version", &tool.version);
        }
        Err(error) => line("clang", &error),
    }
    if pipeline.llvm_pipeline().is_some() {
        match find_tool_cached("opt", OPT_CANDIDATES, &OPT_CACHE) {
            Ok(tool) => {
                line("opt", &tool.path.display().to_string());
                line("opt-version", &tool.version);
            }
            Err(error) => line("opt", &error),
        }
    }
    match resolve_runtime_cached() {
        Ok(runtime) => {
            line("runtime", &runtime.path.display().to_string());
            line("runtime-provenance", runtime.provenance.name());
            line("runtime-sha256", &runtime.sha256);
            line("runtime-abi-version", &runtime.abi_version.to_string());
        }
        Err(error) => line("runtime", &error),
    }
    out
}

/// Validate every external component the emit kind will need, before any
/// frontend or lowering work runs. The CLI's fail-fast front door; emission
/// re-resolves internally against the process-memoized caches (no repeated
/// probes or archive hashing), so this adds safety without threading state.
pub fn check_toolchain(
    target: &NativeTarget,
    profile: OptLevel,
    emit: EmitKind,
) -> Result<(), String> {
    let needs = match emit {
        // Canonical pliron text needs no external tool at any profile.
        EmitKind::Plir => return Ok(()),
        EmitKind::LlvmIr | EmitKind::Bitcode => ToolchainNeeds {
            clang: false,
            runtime: false,
        },
        EmitKind::Object => ToolchainNeeds {
            clang: true,
            runtime: false,
        },
        EmitKind::Exe => ToolchainNeeds {
            clang: true,
            runtime: true,
        },
    };
    ResolvedToolchain::resolve(target, profile, needs)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Which optional components a build actually needs; the `opt` tool's need
/// comes from the profile's pipeline, never from a caller flag.
#[derive(Debug, Clone, Copy)]
pub(super) struct ToolchainNeeds {
    /// Object and executable emission compile bitcode through clang.
    pub(super) clang: bool,
    /// Executable links include the runtime archive.
    pub(super) runtime: bool,
}

/// The checked toolchain for one build: absolute tool paths with verified
/// versions, and the validated runtime archive when linking.
pub(super) struct ResolvedToolchain {
    pub(super) profile: OptLevel,
    pub(super) clang: Option<ResolvedTool>,
    pub(super) opt: Option<ResolvedTool>,
    pub(super) runtime: Option<ResolvedRuntime>,
}

impl ResolvedToolchain {
    /// Resolve and validate exactly the components `needs` asks for (plus
    /// `opt` when the profile's pipeline requires it). Missing tools, wrong
    /// major versions, a missing archive, and a runtime ABI mismatch all
    /// fail here, before any lowering or emission work.
    pub(super) fn resolve(
        _target: &NativeTarget,
        profile: OptLevel,
        needs: ToolchainNeeds,
    ) -> Result<ResolvedToolchain, PlironError> {
        let clang = if needs.clang {
            Some(
                find_tool_cached("clang", CLANG_CANDIDATES, &CLANG_CACHE)
                    .map_err(toolchain_error)?,
            )
        } else {
            None
        };
        let opt = if Pipeline::for_profile(profile).llvm_pipeline().is_some() {
            Some(find_tool_cached("opt", OPT_CANDIDATES, &OPT_CACHE).map_err(toolchain_error)?)
        } else {
            None
        };
        let runtime = if needs.runtime {
            let runtime = resolve_runtime_cached().map_err(toolchain_error)?;
            if runtime.abi_version != rt_abi::MJRT_ABI_VERSION {
                return Err(toolchain_error(format!(
                    "runtime archive {} has ABI version {} but this compiler \
                     requires {}; rebuild it with `cargo build -p mojito-runtime` \
                     from the matching source tree",
                    runtime.path.display(),
                    runtime.abi_version,
                    rt_abi::MJRT_ABI_VERSION,
                )));
            }
            Some(runtime)
        } else {
            None
        };
        Ok(ResolvedToolchain {
            profile,
            clang,
            opt,
            runtime,
        })
    }

    pub(super) fn require_clang(&self) -> Result<&ResolvedTool, PlironError> {
        self.clang
            .as_ref()
            .ok_or_else(|| toolchain_error("toolchain resolved without clang".to_string()))
    }

    pub(super) fn require_opt(&self) -> Result<&ResolvedTool, PlironError> {
        self.opt
            .as_ref()
            .ok_or_else(|| toolchain_error("toolchain resolved without opt".to_string()))
    }
}

/// One external tool: its absolute path and the version its probe reported.
#[derive(Clone)]
pub(super) struct ResolvedTool {
    pub(super) path: PathBuf,
    pub(super) version: String,
}

/// The runtime archive selected for linking, with how it was found, its
/// digest, and the ABI version read from its `mjrt_abi_version` data symbol.
#[derive(Clone)]
pub(super) struct ResolvedRuntime {
    pub(super) path: PathBuf,
    pub(super) provenance: RuntimeProvenance,
    pub(super) sha256: String,
    pub(super) abi_version: u32,
}

/// Where the runtime archive came from, in resolution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeProvenance {
    /// `--runtime-lib PATH` named it explicitly on the command line.
    CliFlag,
    /// `MOJITO_RUNTIME_LIB` named it explicitly.
    EnvVar,
    /// Found through the installation-relative bundle layout
    /// (`<exe>/../lib/libmojito_runtime.a`).
    Bundle,
    /// Found next to the compiler executable in a development target tree.
    DevTree,
}

impl RuntimeProvenance {
    pub(super) fn name(self) -> &'static str {
        match self {
            RuntimeProvenance::CliFlag => "--runtime-lib",
            RuntimeProvenance::EnvVar => "MOJITO_RUNTIME_LIB",
            RuntimeProvenance::Bundle => "installation bundle",
            RuntimeProvenance::DevTree => "development target tree",
        }
    }
}

/// Record the CLI's `--runtime-lib` override for this process; it takes
/// precedence over every other discovery step. Called once by the CLI
/// before any backend work.
pub fn set_runtime_override(path: PathBuf) {
    let _ = RUNTIME_OVERRIDE.set(path);
}

static RUNTIME_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

const CLANG_CANDIDATES: &[&str] = &["clang-22", "clang"];
const OPT_CANDIDATES: &[&str] = &["opt-22", "opt"];

/// Process-memoized [`find_tool`]. Discovery depends only on `PATH`, which
/// is stable for the process, so the subprocess version probes run once even
/// though the fail-fast gate, emission, and the link manifest each resolve
/// the toolchain. Only successes are cached; failures re-probe so a repaired
/// environment is never masked by a stale error.
fn find_tool_cached(
    kind: &str,
    candidates: &[&str],
    cache: &'static std::sync::OnceLock<ResolvedTool>,
) -> Result<ResolvedTool, String> {
    if let Some(tool) = cache.get() {
        return Ok(tool.clone());
    }
    let tool = find_tool(kind, candidates)?;
    let _ = cache.set(tool.clone());
    Ok(tool)
}

static CLANG_CACHE: std::sync::OnceLock<ResolvedTool> = std::sync::OnceLock::new();
static OPT_CACHE: std::sync::OnceLock<ResolvedTool> = std::sync::OnceLock::new();

/// Process-memoized [`resolve_runtime`]: the discovery inputs (the CLI
/// override, `MOJITO_RUNTIME_LIB`, the executable's location) are fixed for
/// the process, so the archive is read, hashed, and parsed once per build
/// rather than once per resolution. Success-only, like [`find_tool_cached`].
fn resolve_runtime_cached() -> Result<ResolvedRuntime, String> {
    static CACHE: std::sync::OnceLock<ResolvedRuntime> = std::sync::OnceLock::new();
    if let Some(runtime) = CACHE.get() {
        return Ok(runtime.clone());
    }
    let runtime = resolve_runtime()?;
    let _ = CACHE.set(runtime.clone());
    Ok(runtime)
}

/// Locate the first candidate on `PATH` whose version probe succeeds and
/// reports the pinned LLVM major. Candidates that probe successfully but
/// report another major are recorded and named in the failure message.
fn find_tool(kind: &str, candidates: &[&str]) -> Result<ResolvedTool, String> {
    let mut rejected = Vec::new();
    for candidate in candidates {
        let Some(path) = which(candidate) else {
            continue;
        };
        let Some(version) = probe_version(&path) else {
            rejected.push(format!("{} (version probe failed)", path.display()));
            continue;
        };
        match parse_llvm_major(&version) {
            Some(major) if major == EXPECTED_LLVM_MAJOR => {
                return Ok(ResolvedTool { path, version });
            }
            Some(major) => {
                rejected.push(format!("{} (major {major}: {version})", path.display()));
            }
            None => {
                rejected.push(format!(
                    "{} (unrecognized version output: {version})",
                    path.display()
                ));
            }
        }
    }
    let found = if rejected.is_empty() {
        "none found on PATH".to_string()
    } else {
        format!("found {}", rejected.join("; "))
    };
    Err(format!(
        "no {kind} matching LLVM {EXPECTED_LLVM_MAJOR} \
         (candidates: {}): {found}",
        candidates.join(", "),
    ))
}

/// Resolve a bare command name to an absolute path through `PATH`, skipping
/// entries that exist but are not executable — a non-executable shadow early
/// on `PATH` must not hide a runnable tool in a later directory.
fn which(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(command);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

/// The first line of `tool --version` under a pinned locale, when it runs.
fn probe_version(path: &Path) -> Option<String> {
    let output = Command::new(path)
        .arg("--version")
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // clang prints "... clang version 22.x.y ..." on the first line; opt
    // prints an "LLVM (...):" banner with "  LLVM version 22.x.y" below it.
    // Return the line carrying the version so the report shows it verbatim.
    text.lines()
        .map(str::trim)
        .find(|line| line.contains("version"))
        .map(str::to_string)
}

/// The LLVM major in a version line: the integer following the last
/// `"version "` marker (`"clang version 22.1.0"`, `"LLVM version 22.1.0"`).
fn parse_llvm_major(version_line: &str) -> Option<u32> {
    let rest = version_line.rsplit("version ").next()?;
    let major: String = rest.chars().take_while(char::is_ascii_digit).collect();
    major.parse().ok()
}

/// Locate and validate the `mojito-runtime` static archive: an explicit
/// `MOJITO_RUNTIME_LIB` wins; otherwise search the compiler executable's
/// directory and its ancestors (which covers `target/debug` and
/// `target/debug/deps` in development builds). The selected archive's
/// digest and embedded ABI version are recorded for validation and
/// reporting.
fn resolve_runtime() -> Result<ResolvedRuntime, String> {
    let (path, provenance) = find_runtime_archive()?;
    let data = std::fs::read(&path)
        .map_err(|error| format!("cannot read runtime archive {}: {error}", path.display()))?;
    let sha256 = sha256_hex(&data);
    let abi_version = archive_abi_version(&data).map_err(|error| {
        format!(
            "runtime archive {} is unusable: {error}; rebuild it with \
             `cargo build -p mojito-runtime`",
            path.display()
        )
    })?;
    Ok(ResolvedRuntime {
        path,
        provenance,
        sha256,
        abi_version,
    })
}

/// The ordered discovery contract: the CLI's `--runtime-lib` override,
/// then `MOJITO_RUNTIME_LIB`, then the installation-relative bundle path
/// (`<exe>/../lib/`), then the development target-tree walk. Explicit
/// steps that name a missing file are hard errors, never fallthroughs.
fn find_runtime_archive() -> Result<(PathBuf, RuntimeProvenance), String> {
    if let Some(path) = RUNTIME_OVERRIDE.get() {
        if path.is_file() {
            return Ok((path.clone(), RuntimeProvenance::CliFlag));
        }
        return Err(format!(
            "--runtime-lib points at a missing file: {}",
            path.display()
        ));
    }
    if let Ok(path) = std::env::var("MOJITO_RUNTIME_LIB") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok((path, RuntimeProvenance::EnvVar));
        }
        return Err(format!(
            "MOJITO_RUNTIME_LIB points at a missing file: {}",
            path.display()
        ));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bundle_lib) = exe
            .parent()
            .and_then(|bin| bin.parent())
            .map(|root| root.join("lib").join("libmojito_runtime.a"))
            && bundle_lib.is_file()
        {
            return Ok((bundle_lib, RuntimeProvenance::Bundle));
        }
        for dir in exe.ancestors().skip(1).take(3) {
            let candidate = dir.join("libmojito_runtime.a");
            if candidate.is_file() {
                return Ok((candidate, RuntimeProvenance::DevTree));
            }
        }
    }
    Err(
        "cannot find libmojito_runtime.a; build it with `cargo build -p mojito-runtime` \
         or point MOJITO_RUNTIME_LIB at the archive"
            .to_string(),
    )
}

/// Read the `mjrt_abi_version` `u32` data symbol out of the archive's
/// members. This is the mechanical ground truth for what the archive was
/// built against, independent of any sidecar metadata.
fn archive_abi_version(archive_data: &[u8]) -> Result<u32, String> {
    use object::read::archive::ArchiveFile;
    use object::{Object, ObjectSection, ObjectSymbol};

    let archive = ArchiveFile::parse(archive_data)
        .map_err(|error| format!("not a static archive: {error}"))?;
    for member in archive.members() {
        let member = member.map_err(|error| format!("corrupt archive member: {error}"))?;
        let member_data = member
            .data(archive_data)
            .map_err(|error| format!("corrupt archive member data: {error}"))?;
        let Ok(file) = object::File::parse(member_data) else {
            continue;
        };
        for symbol in file.symbols() {
            if symbol.name() != Ok(rt_abi::ABI_VERSION_SYMBOL) || !symbol.is_definition() {
                continue;
            }
            let section_index = symbol
                .section_index()
                .ok_or_else(|| format!("{} has no section", rt_abi::ABI_VERSION_SYMBOL))?;
            let section = file
                .section_by_index(section_index)
                .map_err(|error| format!("cannot read section: {error}"))?;
            let data = section
                .data()
                .map_err(|error| format!("cannot read section data: {error}"))?;
            let offset = symbol
                .address()
                .checked_sub(section.address())
                .and_then(|delta| usize::try_from(delta).ok())
                .ok_or_else(|| "symbol offset out of range".to_string())?;
            let bytes: [u8; 4] = data
                .get(offset..offset + 4)
                .and_then(|slice| slice.try_into().ok())
                .ok_or_else(|| format!("{} data out of range", rt_abi::ABI_VERSION_SYMBOL))?;
            return Ok(u32::from_le_bytes(bytes));
        }
    }
    Err(format!(
        "no {} symbol in any archive member",
        rt_abi::ABI_VERSION_SYMBOL
    ))
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn toolchain_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pliron_toolchain_parses_llvm_majors() {
        assert_eq!(
            parse_llvm_major("Ubuntu clang version 22.1.0 (++2026)"),
            Some(22)
        );
        assert_eq!(parse_llvm_major("LLVM version 22.1.0"), Some(22));
        assert_eq!(parse_llvm_major("clang version 17.0.6"), Some(17));
        assert_eq!(parse_llvm_major("no marker here"), None);
    }

    #[test]
    fn pliron_toolchain_rejects_corrupt_archives() {
        let error = archive_abi_version(b"definitely not an archive").unwrap_err();
        assert!(error.contains("not a static archive"), "{error}");
    }

    #[test]
    fn pliron_toolchain_sha256_is_stable() {
        // The empty-input SHA-256 test vector pins the digest encoding.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
