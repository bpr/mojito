//! In-process execution of compiled modules through ORC LLJIT — the
//! differential-testing harness entry (VM output vs native return value).

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron_llvm::llvm_sys::lljit::LLVMLLJIT;
use pliron_llvm::llvm_sys::target::initialize_native;

use super::{PlironError, PlironErrorKind, emit};

/// JIT-execute a zero-argument `Int`-returning compiled function by its
/// native symbol and return its result.
pub(super) fn run_i64(ctx: &Context, module: ModuleOp, symbol: &str) -> Result<i64, PlironError> {
    initialize_native().map_err(|error| jit_error(format!("native target init: {error}")))?;
    let (_llvm_ctx, llvm_module) = emit::to_llvm(ctx, module)?;
    let jit = LLVMLLJIT::new_with_default_builder()
        .map_err(|error| jit_error(format!("LLJIT construction: {error}")))?;
    jit.add_module(llvm_module)
        .map_err(|error| jit_error(format!("LLJIT add_module: {error}")))?;
    let address = jit
        .lookup_symbol(symbol)
        .map_err(|error| jit_error(format!("LLJIT lookup of `{symbol}`: {error}")))?;
    if address == 0 {
        return Err(jit_error(format!("symbol `{symbol}` resolved to null")));
    }
    let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> i64>(address) };
    Ok(function())
}

fn jit_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}
