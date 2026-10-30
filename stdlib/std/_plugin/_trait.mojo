# Upstream's `std/_plugin/_trait.mojo`, in part: `StdPlugin`,
# `DefaultPlugin`, `DefaultPluginHooks`, and `PluginHooks` with the hooks
# whose types are bundled (`unary_math_fn`, `binary_math_fn`,
# `supports_float64`, `flushes_subnormals`, `lowers_subvector_intrinsics`,
# `has_generic_address_space`, and `_handles_debug_assert`), verbatim.
# `StaticString` imports from `std.string`, as the bundled modules on the
# `std.sys.info` import cycle spell it.
# Omitted, with the imports only they use:
# - `reduce_generator_fn`, with `_ReduceGeneratorPluginHookFnType`:
#   `IndexList` and `std.utils.coord.Coord` are not bundled (roadmap
#   R1702);
# - `default_atomic_ordering`: `std.atomic` is not bundled;
# - `stack_allocation_fn`, `address_space_fn`, and `unsafe_dangling_fn`:
#   `AddressSpace` and their hook types are not bundled;
# - `print_emit_fn`: `std.io`'s `_PrintEmitPluginHookFnType` is not
#   bundled;
# - the `@staticmethod` hooks `abort_fn` and `debug_assert_emit_fn`
#   (R1698).

from std.collections import OptionalReg

from std.math.math import (
    _BinaryMathPluginHookFnType,
    _UnaryMathPluginHookFnType,
)
from std.collections.string.string_span import (
    _get_kgen_string,
)
from std.string import StaticString

from .selector import Plugin
from std.sys.info import CompilationTarget


trait StdPlugin(Plugin):
    comptime HookFor[target: CompilationTarget]: PluginHooks


trait PluginHooks:
    """Compile-time hook interface for pluggable stdlib behavior.

    Most hooks are `comptime OptionalReg[Callable]` fields; call sites invoke
    `comptime if CurrentPlugin.xxx_fn: return comptime(CurrentPlugin.xxx_fn.value())(...)`,
    so implementors that leave a hook at `None` add zero cost.

    A few hooks (`abort_fn`, `debug_assert_emit_fn`) are required
    `@staticmethod` trait methods rather than `OptionalReg` fields, because
    their dispatch sites lie on `OptionalReg.value()`'s own instantiation
    path — an `OptionalReg` field would re-enter that template via its own
    `debug_assert` and deadlock comptime instantiation.
    """

    # ===------------------------------------------------------------------=== #
    # Math & Algorithm Operations
    # ===------------------------------------------------------------------=== #

    comptime unary_math_fn[
        name: StaticString, dtype: DType, width: Int
    ]: OptionalReg[_UnaryMathPluginHookFnType] = None
    """Elementwise override for the `std.math` function called `name`.

    Consulted by `exp`, `exp2`, `sqrt`, `rsqrt`, `sin`, `cos`, and `tanh`
    for floating-point inputs, before the stdlib converts the input to another
    `dtype`, so the hook sees the caller's `dtype`. Returning `None` for a
    `name`, `dtype`, and `width` keeps the stdlib's own lowering for that
    case. The hook is not called when the function is evaluated at compile
    time, because the compile-time interpreter cannot run backend intrinsics.

    Parameters:
        name: The `std.math` function being lowered, such as `"sqrt"`.
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: The input SIMD vector.

    Returns:
        Elementwise `name(x)` computed on the vendor backend.
    """

    comptime binary_math_fn[
        name: StaticString, dtype: DType, width: Int
    ]: OptionalReg[_BinaryMathPluginHookFnType] = None
    """Elementwise override for the two-argument function called `name`.

    Consulted by `pow` when both arguments are floating point, under the same
    rules as `unary_math_fn`: the hook sees the caller's `dtype`, `None` keeps
    the stdlib's own lowering, and compile-time evaluation skips the hook.

    Parameters:
        name: The function being lowered, such as `"pow"`.
        dtype: The `dtype` of the inputs and output SIMD vectors.
        width: The width of the inputs and output SIMD vectors.

    Args:
        x: The first input SIMD vector.
        y: The second input SIMD vector.

    Returns:
        Elementwise `name(x, y)` computed on the vendor backend.
    """

    comptime supports_float64: Bool = True
    """Whether the target supports float64 arithmetic.

    Where it is `False`, `std.math` functions that compute float64 results in
    software, such as `log` and `atan2`, reject float64 inputs with a
    compile-time error instead of emitting float64 code the backend cannot
    lower.
    """

    comptime flushes_subnormals[dtype: DType]: Bool = False
    """Whether the target flushes `dtype` subnormals to zero.

    A target that does reads subnormal operands as zero and flushes subnormal
    results of arithmetic and conversions. `std.math` then rebuilds those
    values from their bits so subnormal results are not lost.

    Parameters:
        dtype: The floating-point `dtype` being queried.
    """

    # ===------------------------------------------------------------------=== #
    # SIMD Operations
    # ===------------------------------------------------------------------=== #

    comptime lowers_subvector_intrinsics: Bool = True
    """Whether the backend lowers LLVM's subvector intrinsics.

    These are `llvm.vector.extract`, `llvm.vector.insert`, and
    `llvm.vector.splice.{left,right}`, which `SIMD.slice`, `SIMD.insert`,
    `SIMD.rotate_{left,right}`, and `SIMD.shift_{left,right}` use. Where this
    is `False`, those methods build the result from element accesses and
    `shuffle` instead.
    """

    # ===------------------------------------------------------------------=== #
    # Memory Operations
    # ===------------------------------------------------------------------=== #

    comptime has_generic_address_space: Bool = True
    """Whether a `GENERIC` pointer can reach memory in every address space.

    NVIDIA and AMD GPUs resolve a generic pointer's address space at run time.
    A target without a generic address space, such as Metal, cannot: a pointer
    rebuilt from an integer address does not reach threadgroup memory. Where
    this is `False`, the stdlib keeps the typed pointer instead of
    round-tripping it through an integer.
    """

    # ===------------------------------------------------------------------=== #
    # IO & System Operations
    # ===------------------------------------------------------------------=== #

    comptime _handles_debug_assert: Bool = False
    """If `True`, `_debug_assert_msg` dispatches to `debug_assert_emit_fn`
    and comptime-elides its `_printf` fallback. Required because the
    fallback's transitive `OptionalReg.value()` → `debug_assert` recurses
    back through `_debug_assert_msg` and deadlocks instantiation when
    assertions are enabled."""


# ===-----------------------------------------------------------------------===#
# DefaultPlugin
# ===-----------------------------------------------------------------------===#


struct DefaultPlugin(StdPlugin):
    """Default `PluginHooks` implementation used when no plugin is active.

    Every hook is left at its `PluginHooks` default, so the built-in stdlib
    code paths are preserved.
    """

    comptime name: __mlir_type.`!kgen.string` = _get_kgen_string["default"]()
    comptime HookFor[target: CompilationTarget] = DefaultPluginHooks[target]


struct DefaultPluginHooks[target: CompilationTarget](PluginHooks):
    pass
