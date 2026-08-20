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
use pliron_llvm::llvm_sys::core::{LLVMContext, LLVMModule};
use pliron_llvm::to_llvm_ir;

use super::{OptLevel, PlironError, PlironErrorKind};

/// Convert the pliron module into a verified LLVM module. The returned
/// [`LLVMContext`] owns the module's storage and must stay alive with it.
pub(super) fn to_llvm(
    ctx: &Context,
    module: ModuleOp,
) -> Result<(LLVMContext, LLVMModule), PlironError> {
    let llvm_ctx = LLVMContext::default();
    let llvm_module = to_llvm_ir::convert_module(ctx, &llvm_ctx, module)
        .map_err(|error| emit_error(format!("LLVM conversion failed: {}", error.disp(ctx))))?;
    llvm_module
        .verify()
        .map_err(|error| emit_error(format!("LLVM module verification failed: {error}")))?;
    Ok((llvm_ctx, llvm_module))
}

/// Convert and, at [`OptLevel::O1`], round-trip the module through `opt`
/// bitcode optimization. The optimized module lives in a fresh context that
/// the caller must keep alive with it, exactly like [`to_llvm`].
pub(super) fn to_llvm_optimized(
    ctx: &Context,
    module: ModuleOp,
    opt: OptLevel,
) -> Result<(LLVMContext, LLVMModule), PlironError> {
    let (llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
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
    opt: OptLevel,
) -> Result<String, PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm_optimized(ctx, module, opt)?;
    Ok(llvm_module.to_string())
}

/// Write LLVM bitcode to `path`.
pub(super) fn write_bitcode(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
    bitcode_to(&llvm_module, path)?;
    optimize_bitcode(path, opt)
}

/// Write a relocatable object to `path` (bitcode + `clang -c`).
pub(super) fn write_object(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    clang_from_bitcode(ctx, module, path, &["-c"], opt)
}

/// Link a host executable at `path` (bitcode + `clang`). The module must
/// already contain the synthesized `main` wrapper.
pub(super) fn write_executable(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
    opt: OptLevel,
) -> Result<(), PlironError> {
    clang_from_bitcode(ctx, module, path, &[], opt)
}

fn clang_from_bitcode(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
    extra_args: &[&str],
    opt: OptLevel,
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
    let bitcode = temp_bitcode_path(path);
    let prepared =
        bitcode_to(&llvm_module, &bitcode).and_then(|()| optimize_bitcode(&bitcode, opt));
    let output = prepared.and_then(|()| {
        let clang = find_clang()?;
        let run = Command::new(clang)
            .args(extra_args)
            .arg(&bitcode)
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
