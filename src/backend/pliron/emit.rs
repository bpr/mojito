//! LLVM-side emission: textual IR, bitcode, objects, and executables.
//!
//! Objects and executables go through `clang` over emitted bitcode (pliron
//! ships no object-emission API); the version-suffixed `clang-22` matching
//! llvm-sys 221 is preferred, with plain `clang` as the fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron::printable::Printable;
use pliron_llvm::llvm_sys::core::{LLVMContext, LLVMModule};
use pliron_llvm::to_llvm_ir;

use super::{PlironError, PlironErrorKind};

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

/// Textual LLVM IR of the converted module.
pub(super) fn llvm_ir(ctx: &Context, module: ModuleOp) -> Result<String, PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
    Ok(llvm_module.to_string())
}

/// Write LLVM bitcode to `path`.
pub(super) fn write_bitcode(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
    bitcode_to(&llvm_module, path)
}

/// Write a relocatable object to `path` (bitcode + `clang -c`).
pub(super) fn write_object(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
) -> Result<(), PlironError> {
    clang_from_bitcode(ctx, module, path, &["-c"])
}

/// Link a host executable at `path` (bitcode + `clang`). The module must
/// already contain the synthesized `main` wrapper.
pub(super) fn write_executable(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
) -> Result<(), PlironError> {
    clang_from_bitcode(ctx, module, path, &[])
}

fn clang_from_bitcode(
    ctx: &Context,
    module: ModuleOp,
    path: &Path,
    extra_args: &[&str],
) -> Result<(), PlironError> {
    let (_llvm_ctx, llvm_module) = to_llvm(ctx, module)?;
    let bitcode = temp_bitcode_path(path);
    bitcode_to(&llvm_module, &bitcode)?;
    let clang = find_clang()?;
    let output = Command::new(clang)
        .args(extra_args)
        .arg(&bitcode)
        .arg("-o")
        .arg(path)
        .output()
        .map_err(|error| emit_error(format!("cannot run {clang}: {error}")));
    let _ = std::fs::remove_file(&bitcode);
    let output = output?;
    if !output.status.success() {
        return Err(emit_error(format!(
            "{clang} failed: {}",
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

fn emit_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}
