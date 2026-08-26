//! LLVM-side emission: textual IR, bitcode, objects, and executables.
//!
//! Objects and executables go through `clang` over emitted bitcode (pliron
//! ships no object-emission API); tools are resolved and version-checked up
//! front by [`ResolvedToolchain`] and invoked through recorded absolute
//! paths with a pinned locale and deterministic, target-explicit argument
//! lists. The `release` profile runs the resolved `opt` with the LLVM
//! pipeline selected by [`Pipeline`] over the bitcode — pliron-llvm 0.17
//! keeps its raw `LLVMModuleRef` private, so the new-pass-manager is
//! unreachable in-process. Every binary artifact is written failure-
//! atomically through [`write_atomic`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::printable::Printable;
use pliron_llvm::llvm_sys::core::{LLVMContext, LLVMMemoryBuffer, LLVMModule};
use pliron_llvm::to_llvm_ir;

use crate::native::target::NativeTarget;

use super::artifact::write_atomic;
use super::debug::{DebugPolicy, DebugTable};
use super::pipeline::{Pipeline, timing};
use super::toolchain::{ResolvedToolchain, ToolchainNeeds};
use super::{DebugInfo, OptLevel, PlironError, PlironErrorKind};

/// Convert the pliron module into a verified LLVM module stamped with the
/// target's triple and pinned data-layout string. The returned [`LLVMContext`]
/// owns the module's storage and must stay alive with it.
///
/// Stamping goes through print → prepend → reparse: pliron-llvm 0.17 keeps
/// its raw `LLVMModuleRef` private, so `LLVMSetTarget`/`LLVMSetDataLayout`
/// are unreachable in-process. The reparsed module re-verifies before use.
pub(super) fn to_llvm(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
) -> Result<(LLVMContext, LLVMModule), PlironError> {
    let stamped_text = to_stamped_ir(ctx, module, target)?;
    let stamped_ctx = LLVMContext::default();
    let buffer = LLVMMemoryBuffer::from_str(&stamped_text, "mojito-target-stamp");
    let stamped_module = LLVMModule::from_ir_in_memory_buffer(&stamped_ctx, buffer)
        .map_err(|error| emit_error(format!("target-stamped module reparse failed: {error}")))?;
    stamped_module.verify().map_err(|error| {
        emit_error(format!(
            "target-stamped module verification failed: {error}"
        ))
    })?;
    Ok((stamped_ctx, stamped_module))
}

/// Convert the pliron module to verified LLVM IR text stamped with the
/// target header — the shared front half of [`to_llvm`] and the debug-attach
/// path, which parses and re-verifies the text itself and so takes the
/// string directly rather than paying an intermediate reparse/verify.
fn to_stamped_ir(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
) -> Result<String, PlironError> {
    timing("llvm-convert", || {
        let llvm_ctx = LLVMContext::default();
        let llvm_module = to_llvm_ir::convert_module(ctx, &llvm_ctx, module)
            .map_err(|error| emit_error(format!("LLVM conversion failed: {}", error.disp(ctx))))?;
        llvm_module
            .verify()
            .map_err(|error| emit_error(format!("LLVM module verification failed: {error}")))?;
        let text = llvm_module.to_string();
        if text.contains("target datalayout") || text.contains("target triple") {
            return Err(emit_error(
                "converted module unexpectedly carries a target header already".to_string(),
            ));
        }
        Ok(format!(
            "target datalayout = \"{}\"\ntarget triple = \"{}\"\n{text}",
            target.triple.data_layout(),
            target.triple.name(),
        ))
    })
}

/// Convert and, when the profile has an LLVM pipeline, round-trip the module
/// through `opt` bitcode optimization. The optimized module lives in a fresh
/// context that the caller must keep alive with it, exactly like [`to_llvm`].
pub(super) fn to_llvm_optimized(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    opt: OptLevel,
) -> Result<(LLVMContext, LLVMModule), PlironError> {
    let (llvm_ctx, llvm_module) = to_llvm(ctx, module, target)?;
    if Pipeline::for_profile(opt).llvm_pipeline().is_none() {
        return Ok((llvm_ctx, llvm_module));
    }
    let toolchain = ResolvedToolchain::resolve(
        target,
        opt,
        ToolchainNeeds {
            clang: false,
            runtime: false,
        },
    )?;
    let bitcode = scratch_bitcode_path();
    let optimized = bitcode_to(&llvm_module, &bitcode)
        .and_then(|()| optimize_bitcode(&bitcode, &toolchain))
        .and_then(|()| {
            let path = bitcode
                .to_str()
                .ok_or_else(|| emit_error(format!("non-UTF-8 temp path {}", bitcode.display())))?;
            let reparse_ctx = LLVMContext::default();
            let reparsed = LLVMModule::from_ir_in_file(&reparse_ctx, path).map_err(|error| {
                emit_error(format!("optimized bitcode reparse failed: {error}"))
            })?;
            Ok((reparse_ctx, reparsed))
        });
    let _ = std::fs::remove_file(&bitcode);
    optimized
}

/// Textual LLVM IR of the converted module.
pub(super) fn llvm_ir(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    opt: OptLevel,
) -> Result<String, PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm_optimized(ctx, module, target, opt)?;
    Ok(llvm_module.to_string())
}

/// Write LLVM bitcode to `path`, failure-atomically.
pub(super) fn write_bitcode(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    let toolchain = ResolvedToolchain::resolve(
        target,
        opt,
        ToolchainNeeds {
            clang: false,
            runtime: false,
        },
    )?;
    write_atomic(path, |temp| {
        emit_bitcode(ctx, module, target, temp, debug)?;
        optimize_bitcode(temp, &toolchain)
    })
}

/// Report which functions would degrade to subprogram-only debug
/// correlation — the corpus test's channel; the artifact goes to a scratch
/// path and is discarded.
pub(super) fn debug_degradations(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    table: &DebugTable,
) -> Result<Vec<String>, PlironError> {
    let stamped_text = to_stamped_ir(ctx, module, target)?;
    let scratch = scratch_bitcode_path();
    let degraded = super::debug::write_bitcode_with_debug(&stamped_text, table, &scratch);
    let _ = std::fs::remove_file(&scratch);
    degraded
}

/// Write a relocatable object to `path` (bitcode + `clang -c`),
/// failure-atomically.
pub(super) fn write_object(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    let toolchain = ResolvedToolchain::resolve(
        target,
        opt,
        ToolchainNeeds {
            clang: true,
            runtime: false,
        },
    )?;
    clang_from_bitcode(ctx, module, target, path, &["-c"], &toolchain, debug)?;
    write_link_manifest(target, path, &toolchain)
}

/// Link a saved object into an executable through its sidecar manifest:
/// validate schema, target, ABI version, object digest, runtime digest,
/// and toolchain major, then issue the same deterministic clang link line
/// `write_executable` uses. The CLI's `link` verb.
pub fn link_object(object_path: &Path, output: &Path) -> Result<(), PlironError> {
    use crate::native::rt_abi;
    use crate::native::target::Triple;

    let manifest_path = link_manifest_path(object_path);
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| {
        emit_error(format!(
            "cannot read link manifest {}: {error}; emit the object with \
             `mojito compile --backend pliron --emit obj` to produce one",
            manifest_path.display()
        ))
    })?;
    let field = |key: &str| -> Result<&str, PlironError> {
        manifest
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}	")))
            .ok_or_else(|| {
                emit_error(format!(
                    "link manifest {} lacks the '{key}' field",
                    manifest_path.display()
                ))
            })
    };
    let expect = |key: &str, expected: &str| -> Result<(), PlironError> {
        let found = field(key)?;
        if found != expected {
            return Err(emit_error(format!(
                "link manifest {}: {key} records '{found}' but this link resolves it \
                 to '{expected}'; re-emit the object (or fix the mismatched input)",
                manifest_path.display()
            )));
        }
        Ok(())
    };
    expect("schema", "1")?;
    let triple = Triple::parse(field("target")?).map_err(emit_error)?;
    let target = NativeTarget::new(triple);
    expect("abi-version", &rt_abi::MJRT_ABI_VERSION.to_string())?;
    expect(
        "clang-major",
        &super::toolchain::EXPECTED_LLVM_MAJOR.to_string(),
    )?;
    expect("libs", "m")?;
    let object_sha = sha256_file(object_path)?;
    expect("object-sha256", &object_sha)?;

    let toolchain = ResolvedToolchain::resolve(
        &target,
        OptLevel::O0,
        ToolchainNeeds {
            clang: true,
            runtime: true,
        },
    )?;
    let clang = toolchain.require_clang()?;
    let runtime = toolchain
        .runtime
        .as_ref()
        .expect("resolve with runtime need populates it");
    let recorded_runtime = field("runtime-sha256")?;
    if recorded_runtime != "-" && recorded_runtime != runtime.sha256 {
        return Err(emit_error(format!(
            "runtime archive {} (sha256 {}) does not match the manifest's \
             recorded runtime (sha256 {recorded_runtime}); rebuild the runtime \
             or re-emit the object",
            runtime.path.display(),
            runtime.sha256,
        )));
    }
    let link_inputs = vec![runtime.path.clone()];
    write_atomic(output, |temp| {
        let args = clang_args(&target, EXE_LINK_ARGS, object_path, &link_inputs, temp);
        let run = Command::new(&clang.path)
            .args(&args)
            .env("LC_ALL", "C")
            .output()
            .map_err(|error| emit_error(format!("cannot run {}: {error}", clang.path.display())))?;
        if !run.status.success() {
            return Err(emit_error(format!(
                "{} failed: {}",
                clang.path.display(),
                String::from_utf8_lossy(&run.stderr)
            )));
        }
        Ok(())
    })
}

/// The sidecar link manifest `<obj>.link.tsv`: everything `link_object`
/// needs to validate and reconstruct the deterministic link line, so users
/// never rebuild the clang command by hand. The runtime digest is recorded
/// when an archive is discoverable at emission time and validated at link
/// time when present.
fn write_link_manifest(
    target: &NativeTarget,
    object_path: &Path,
    toolchain: &ResolvedToolchain,
) -> Result<(), PlironError> {
    use crate::native::rt_abi;

    let object_sha = sha256_file(object_path)?;
    // Object emission does not require a runtime; record its digest only
    // when one resolves so relocated links can still validate by ABI.
    let runtime_sha = ResolvedToolchain::resolve(
        target,
        toolchain.profile,
        ToolchainNeeds {
            clang: false,
            runtime: true,
        },
    )
    .ok()
    .and_then(|tc| tc.runtime.map(|runtime| runtime.sha256));
    let clang_major = super::toolchain::EXPECTED_LLVM_MAJOR;
    let manifest = [
        "schema\t1".to_string(),
        format!("target\t{}", target.triple.name()),
        format!("abi-version\t{}", rt_abi::MJRT_ABI_VERSION),
        format!("object-sha256\t{object_sha}"),
        format!("runtime-sha256\t{}", runtime_sha.as_deref().unwrap_or("-")),
        "libs\tm".to_string(),
        format!("clang-major\t{clang_major}"),
    ]
    .join("\n")
        + "\n";
    let manifest_path = link_manifest_path(object_path);
    write_atomic(&manifest_path, |temp| {
        std::fs::write(temp, &manifest).map_err(|error| {
            emit_error(format!(
                "cannot write link manifest {}: {error}",
                manifest_path.display()
            ))
        })
    })
}

/// `<obj>.link.tsv`, next to the object.
fn link_manifest_path(object_path: &Path) -> PathBuf {
    let mut name = object_path.file_name().unwrap_or_default().to_os_string();
    name.push(".link.tsv");
    object_path.with_file_name(name)
}

/// The hex sha256 of a file's bytes.
fn sha256_file(path: &Path) -> Result<String, PlironError> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path)
        .map_err(|error| emit_error(format!("cannot read {}: {error}", path.display())))?;
    Ok(Sha256::digest(&data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Link an executable at `path` (bitcode + `clang`), linking the versioned
/// `mojito-runtime` static archive, failure-atomically. The module must
/// already contain the synthesized `main` wrapper (which references the
/// runtime's version symbol).
pub(super) fn write_executable(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    let toolchain = ResolvedToolchain::resolve(
        target,
        opt,
        ToolchainNeeds {
            clang: true,
            runtime: true,
        },
    )?;
    // `-lm`: float `**` lowers to `llvm.pow.f64`, which selects to libm.
    clang_from_bitcode(ctx, module, target, path, EXE_LINK_ARGS, &toolchain, debug)
}

/// [`write_executable`] instrumented with AddressSanitizer (whose interposed
/// allocator also gives LeakSanitizer coverage of the runtime's `std::alloc`
/// allocations) — the sanitizer acceptance lane's link mode.
pub(super) fn write_executable_sanitized(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    let toolchain = ResolvedToolchain::resolve(
        target,
        opt,
        ToolchainNeeds {
            clang: true,
            runtime: true,
        },
    )?;
    clang_from_bitcode(
        ctx,
        module,
        target,
        path,
        SANITIZED_LINK_ARGS,
        &toolchain,
        debug,
    )
}

/// Extra clang arguments for plain executable links. `--build-id=none`
/// keeps the linked artifact byte-reproducible across build directories.
const EXE_LINK_ARGS: &[&str] = &["-lm", "-Wl,--build-id=none"];

/// Extra clang arguments for the sanitizer lane's links.
const SANITIZED_LINK_ARGS: &[&str] = &["-fsanitize=address", "-g", "-lm", "-Wl,--build-id=none"];

fn clang_from_bitcode(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    extra_args: &[&str],
    toolchain: &ResolvedToolchain,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    let clang = toolchain.require_clang()?;
    let link_inputs: Vec<PathBuf> = toolchain
        .runtime
        .as_ref()
        .map(|runtime| vec![runtime.path.clone()])
        .unwrap_or_default();
    let bitcode = temp_bitcode_path(path);
    let prepared = emit_bitcode(ctx, module, target, &bitcode, debug)
        .and_then(|()| optimize_bitcode(&bitcode, toolchain))
        .and_then(|()| {
            write_atomic(path, |temp| {
                let args = clang_args(target, extra_args, &bitcode, &link_inputs, temp);
                let run = timing("clang", || {
                    Command::new(&clang.path)
                        .args(&args)
                        .env("LC_ALL", "C")
                        .output()
                })
                .map_err(|error| {
                    emit_error(format!("cannot run {}: {error}", clang.path.display()))
                })?;
                if !run.status.success() {
                    return Err(emit_error(format!(
                        "{} failed: {}",
                        clang.path.display(),
                        String::from_utf8_lossy(&run.stderr)
                    )));
                }
                Ok(())
            })
        });
    let _ = std::fs::remove_file(&bitcode);
    prepared
}

/// The deterministic clang argument list: target-explicit, config-file-free,
/// inputs and outputs in fixed positions. Argument-order changes are policy
/// changes; the snapshot test below pins the rendering.
fn clang_args(
    target: &NativeTarget,
    extra_args: &[&str],
    bitcode: &Path,
    link_inputs: &[PathBuf],
    output: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    args.push(format!("--target={}", target.triple.name()).into());
    args.push("--no-default-config".into());
    args.extend(extra_args.iter().map(OsString::from));
    args.push(bitcode.as_os_str().to_owned());
    args.extend(link_inputs.iter().map(|input| input.as_os_str().to_owned()));
    args.push("-o".into());
    args.push(output.as_os_str().to_owned());
    args
}

/// Run the profile's LLVM pipeline over a bitcode file in place. Profiles
/// without one ([`OptLevel::O0`]) are a no-op; the pipeline selection is
/// owned by [`Pipeline`] and the tool by the resolved toolchain.
fn optimize_bitcode(path: &Path, toolchain: &ResolvedToolchain) -> Result<(), PlironError> {
    let Some(passes) = Pipeline::for_profile(toolchain.profile).llvm_pipeline() else {
        return Ok(());
    };
    let opt_bin = toolchain.require_opt()?;
    let output = timing("llvm-opt", || {
        Command::new(&opt_bin.path)
            .arg(format!("-passes={passes}"))
            .arg(path)
            .arg("-o")
            .arg(path)
            .env("LC_ALL", "C")
            .output()
    })
    .map_err(|error| emit_error(format!("cannot run {}: {error}", opt_bin.path.display())))?;
    if !output.status.success() {
        return Err(emit_error(format!(
            "{} failed: {}",
            opt_bin.path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Convert the module and emit bitcode at `path`, attaching debug
/// information when the policy asks for it. The debug path hands the stamped
/// IR text straight to the attach (whose own parse and verify stand in for
/// [`to_llvm`]'s), so the module is serialized and verified once either way.
/// Production emission discards the degradation report; the corpus test pins
/// it through [`debug_degradations`].
fn emit_bitcode(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    debug: DebugPolicy<'_>,
) -> Result<(), PlironError> {
    match debug.level {
        DebugInfo::None => {
            let (_llvm_ctx, llvm_module) = to_llvm(ctx, module, target)?;
            bitcode_to(&llvm_module, path)
        }
        DebugInfo::Lines => {
            let stamped_text = to_stamped_ir(ctx, module, target)?;
            super::debug::write_bitcode_with_debug(&stamped_text, debug.table, path)
                .map(|_degraded| ())
        }
    }
}

fn bitcode_to(llvm_module: &LLVMModule, path: &Path) -> Result<(), PlironError> {
    let path_str = path
        .to_str()
        .ok_or_else(|| emit_error(format!("non-UTF-8 output path {}", path.display())))?;
    llvm_module
        .bitcode_to_file(path_str)
        .map_err(|error| emit_error(format!("bitcode emission failed: {error}")))
}

/// A sibling temp path for intermediate bitcode, unique per process. The
/// `.bc` suffix is load-bearing: clang infers the input kind from it.
fn temp_bitcode_path(target: &Path) -> PathBuf {
    let stem = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(".{stem}.{}.tmp.bc", std::process::id()))
}

/// A unique temp-directory bitcode path for output-less pipelines (the JIT's
/// optimization round trip). Same load-bearing `.bc` suffix.
fn scratch_bitcode_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        ".mojito-pliron.{}.{unique}.tmp.bc",
        std::process::id()
    ))
}

fn emit_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::target::Triple;

    #[test]
    fn pliron_clang_args_render_deterministically() {
        let target = NativeTarget::new(Triple::X86_64UnknownLinuxGnu);
        let args = clang_args(
            &target,
            EXE_LINK_ARGS,
            Path::new("/tmp/in.bc"),
            &[PathBuf::from("/rt/libmojito_runtime.a")],
            Path::new("/tmp/out"),
        );
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        expect_test::expect![[r#"
            [
                "--target=x86_64-unknown-linux-gnu",
                "--no-default-config",
                "-lm",
                "-Wl,--build-id=none",
                "/tmp/in.bc",
                "/rt/libmojito_runtime.a",
                "-o",
                "/tmp/out",
            ]
        "#]]
        .assert_debug_eq(&rendered);
    }
}
