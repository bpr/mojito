//! In-process execution of compiled modules through ORC LLJIT — the
//! differential-testing harness entry (VM output vs native return value).

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron_llvm::llvm_sys::lljit::LLVMLLJIT;
use pliron_llvm::llvm_sys::target::initialize_native;

use super::{JitValue, OptLevel, PlironError, PlironErrorKind, RetKind, emit};

/// JIT-execute a zero-argument compiled function by its native symbol and
/// return its typed result. `ret` is the compiled function's checked return
/// kind; an i1 result is read through `u8` with the low bit significant (the
/// upper bits of the return register are undefined for LLVM `i1`).
pub(super) fn run_value(
    ctx: &Context,
    module: ModuleOp,
    symbol: &str,
    ret: RetKind,
    opt: OptLevel,
) -> Result<JitValue, PlironError> {
    ensure_native_target()?;
    // The reparse context of the optimized path must outlive the JIT'd call.
    let (_llvm_ctx, llvm_module) = emit::to_llvm_optimized(ctx, module, opt)?;
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
    match ret {
        RetKind::I64 => {
            let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> i64>(address) };
            Ok(JitValue::Int(function()))
        }
        RetKind::U64 => {
            let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> u64>(address) };
            Ok(JitValue::UInt(function()))
        }
        RetKind::F64 => {
            let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> f64>(address) };
            Ok(JitValue::Float64(function()))
        }
        RetKind::Bool => {
            let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> u8>(address) };
            Ok(JitValue::Bool(function() & 1 != 0))
        }
        RetKind::Void => Err(jit_error(format!(
            "JIT entry `{symbol}` returns no value; only value-returning entries are supported"
        ))),
    }
}

/// Initialize the native target exactly once — LLVM's target registration
/// mutates global state and is not itself thread-safe, while concurrent JIT
/// runs over separate contexts are.
fn ensure_native_target() -> Result<(), PlironError> {
    use std::sync::OnceLock;
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(initialize_native)
        .clone()
        .map_err(|error| jit_error(format!("native target init: {error}")))
}

fn jit_error(message: String) -> PlironError {
    PlironError {
        function: None,
        kind: PlironErrorKind::Emit(message),
        location: None,
    }
}
