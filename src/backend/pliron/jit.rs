//! In-process execution of compiled modules through ORC LLJIT — the
//! differential-testing harness entry (VM output vs native return value).

use pliron::builtin::ops::ModuleOp;
use pliron::context::Context;
use pliron_llvm::llvm_sys::lljit::{JITSymbolGenericFlags, LLVMLLJIT};
use pliron_llvm::llvm_sys::target::initialize_native;

use crate::native::target::NativeTarget;

use super::{JitValue, OptLevel, PlironError, PlironErrorKind, RetKind, emit};

/// JIT-execute a zero-argument compiled function by its native symbol and
/// return its typed result. `ret` is the compiled function's checked return
/// kind; an i1 result is read through `u8` with the low bit significant (the
/// upper bits of the return register are undefined for LLVM `i1`). `symbols`
/// maps external symbols the module references (runtime-contract functions)
/// to in-process addresses — explicit and deterministic, rather than relying
/// on process-symbol resolution. The caller has already checked that `target`
/// is the host.
pub(super) fn run_value(
    ctx: &Context,
    module: ModuleOp,
    target: &NativeTarget,
    symbol: &str,
    ret: RetKind,
    opt: OptLevel,
    symbols: &[(&str, u64)],
) -> Result<JitValue, PlironError> {
    ensure_native_target()?;
    // The reparse context of the optimized path must outlive the JIT'd call.
    let (_llvm_ctx, llvm_module) = emit::to_llvm_optimized(ctx, module, target, opt)?;
    let jit = LLVMLLJIT::new_with_default_builder()
        .map_err(|error| jit_error(format!("LLJIT construction: {error}")))?;
    for (name, address) in symbols {
        jit.add_symbol_mapping(
            name,
            *address,
            JITSymbolGenericFlags::JITSymbolGenericFlagsCallable
                | JITSymbolGenericFlags::JITSymbolGenericFlagsExported,
        )
        .map_err(|error| jit_error(format!("LLJIT symbol mapping of `{name}`: {error}")))?;
    }
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
        RetKind::Ptr => Err(jit_error(format!(
            "JIT entry `{symbol}` returns a pointer; raw addresses have no VM display analog"
        ))),
        // A sized scalar reads back as the VM's mathematical lane value.
        RetKind::Sized(crate::ast::Dtype::Float32) => {
            let function = unsafe { std::mem::transmute::<u64, extern "C" fn() -> f32>(address) };
            Ok(JitValue::Float64(f64::from(function())))
        }
        RetKind::Sized(dtype) => {
            let Some((bits, signed)) = crate::runtime::integer_dtype_bits(dtype) else {
                return Err(jit_error(format!(
                    "JIT entry `{symbol}` returns an unreadable sized scalar ({dtype:?})"
                )));
            };
            macro_rules! read_ret {
                ($ty:ty) => {{
                    let function =
                        unsafe { std::mem::transmute::<u64, extern "C" fn() -> $ty>(address) };
                    function()
                }};
            }
            if signed {
                Ok(JitValue::Int(match bits {
                    8 => i64::from(read_ret!(i8)),
                    16 => i64::from(read_ret!(i16)),
                    32 => i64::from(read_ret!(i32)),
                    _ => read_ret!(i64),
                }))
            } else {
                Ok(JitValue::UInt(match bits {
                    8 => u64::from(read_ret!(u8)),
                    16 => u64::from(read_ret!(u16)),
                    32 => u64::from(read_ret!(u32)),
                    _ => read_ret!(u64),
                }))
            }
        }
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
