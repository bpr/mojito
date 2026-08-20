//! LLVM-side emission: textual IR, bitcode, objects, and executables.
//!
//! Objects and executables go through `clang` over emitted bitcode (pliron
//! ships no object-emission API); the version-suffixed `clang-22` matching
//! llvm-sys 221 is preferred, with plain `clang` as the fallback. The
//! optimized level runs the pinned `opt` (same candidate policy) with
//! `-passes='default<O1>'` over the bitcode — pliron-llvm 0.17 keeps its raw
//! `LLVMModuleRef` private, so the new-pass-manager is unreachable in-process.

use std::path::{Path, PathBuf};
use std::process::Command;

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::printable::Printable;
use pliron_llvm::llvm_sys::core::{LLVMContext, LLVMMemoryBuffer, LLVMModule};
use pliron_llvm::to_llvm_ir;

use crate::native::target::NativeTarget;

use super::{OptLevel, PlironError, PlironErrorKind};

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
    let stamped_text = format!(
        "target datalayout = \"{}\"\ntarget triple = \"{}\"\n{text}",
        target.triple.data_layout(),
        target.triple.name(),
    );
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

/// Convert and, at [`OptLevel::O1`], round-trip the module through `opt`
/// bitcode optimization. The optimized module lives in a fresh context that
/// the caller must keep alive with it, exactly like [`to_llvm`].
pub(super) fn to_llvm_optimized(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    opt: OptLevel,
) -> Result<(LLVMContext, LLVMModule), PlironError> {
    let (llvm_ctx, llvm_module) = to_llvm(ctx, module, target)?;
    if matches!(opt, OptLevel::O0) {
        return Ok((llvm_ctx, llvm_module));
    }
    let bitcode = scratch_bitcode_path();
    bitcode_to(&llvm_module, &bitcode)?;
    let optimized = optimize_bitcode(&bitcode, opt).and_then(|()| {
        let path = bitcode
            .to_str()
            .ok_or_else(|| emit_error(format!("non-UTF-8 temp path {}", bitcode.display())))?;
        let reparse_ctx = LLVMContext::default();
        let reparsed = LLVMModule::from_ir_in_file(&reparse_ctx, path)
            .map_err(|error| emit_error(format!("optimized bitcode reparse failed: {error}")))?;
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

/// Write LLVM bitcode to `path`.
pub(super) fn write_bitcode(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module, target)?;
    bitcode_to(&llvm_module, path)?;
    optimize_bitcode(path, opt)
}

/// Write a relocatable object to `path` (bitcode + `clang -c`).
pub(super) fn write_object(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    clang_from_bitcode(ctx, module, target, path, &["-c"], &[], opt)
}

/// Link an executable at `path` (bitcode + `clang`), linking the versioned
/// `mojito-runtime` static archive. The module must already contain the
/// synthesized `main` wrapper (which references the runtime's version symbol).
pub(super) fn write_executable(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    let runtime = find_runtime_archive()?;
    clang_from_bitcode(ctx, module, target, path, &[], &[runtime], opt)
}

fn clang_from_bitcode(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    path: &Path,
    extra_args: &[&str],
    link_inputs: &[PathBuf],
    opt: OptLevel,
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module, target)?;
    let bitcode = temp_bitcode_path(path);
    let prepared =
        bitcode_to(&llvm_module, &bitcode).and_then(|()| optimize_bitcode(&bitcode, opt));
    let output = prepared.and_then(|()| {
        let clang = find_clang()?;
        let run = Command::new(clang)
            .arg(format!("--target={}", target.triple.name()))
            .args(extra_args)
            .arg(&bitcode)
            .args(link_inputs)
            .arg("-o")
            .arg(path)
            .output()
            .map_err(|error| emit_error(format!("cannot run {clang}: {error}")))?;
        Ok((clang, run))
    });
    let _ = std::fs::remove_file(&bitcode);
    let (clang, output) = output?;
    if !output.status.success() {
        return Err(emit_error(format!(
            "{clang} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Locate the `mojito-runtime` static archive to link into executables:
/// an explicit `MOJITO_RUNTIME_LIB` wins; otherwise search the compiler
/// executable's directory and its ancestors (which covers `target/debug` and
/// `target/debug/deps` in development builds).
fn find_runtime_archive() -> Result<PathBuf, PlironError> {
    if let Ok(path) = std::env::var("MOJITO_RUNTIME_LIB") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(emit_error(format!(
            "MOJITO_RUNTIME_LIB points at a missing file: {}",
            path.display()
        )));
    }
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1).take(3) {
            let candidate = dir.join("libmojito_runtime.a");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(emit_error(
        "cannot find libmojito_runtime.a; build it with `cargo build -p mojito-runtime` \
         or point MOJITO_RUNTIME_LIB at the archive"
            .to_string(),
    ))
}

/// Run the conservative optimization pipeline over a bitcode file in place.
/// [`OptLevel::O0`] is a no-op; [`OptLevel::O1`] runs `opt` with the standard
/// `default<O1>` pipeline.
fn optimize_bitcode(path: &Path, opt: OptLevel) -> Result<(), PlironError> {
    if matches!(opt, OptLevel::O0) {
        return Ok(());
    }
    let opt_bin = find_opt()?;
    let output = Command::new(opt_bin)
        .arg("-passes=default<O1>")
        .arg(path)
        .arg("-o")
        .arg(path)
        .output()
        .map_err(|error| emit_error(format!("cannot run {opt_bin}: {error}")))?;
    if !output.status.success() {
        return Err(emit_error(format!(
            "{opt_bin} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
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

/// Prefer the clang matching llvm-sys 221; fall back to plain `clang`.
fn find_clang() -> Result<&'static str, PlironError> {
    for candidate in ["clang-22", "clang"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            return Ok(candidate);
        }
    }
    Err(emit_error(
        "no clang found; object and executable emission need clang (LLVM 22)".to_string(),
    ))
}

/// Prefer the opt matching llvm-sys 221; fall back to plain `opt`.
fn find_opt() -> Result<&'static str, PlironError> {
    for candidate in ["opt-22", "opt"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            return Ok(candidate);
        }
    }
    Err(emit_error(
        "no opt found; the optimized native level needs opt (LLVM 22)".to_string(),
    ))
}

fn emit_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}
