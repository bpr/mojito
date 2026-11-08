# Upstream's `std/sys/intrinsics.mojo` surface for the proof subset:
# `llvm_intrinsic`, the call of an LLVM intrinsic by name through
# `pop.call_llvm_intrinsic`. The intrinsics Mojito computes are
# `mojito_types::llvm_intrinsic::LlvmIntrinsic`'s; a reached call of any
# other is rejected when its instance is elaborated.
#
# The body is upstream's, except that `args.get_loaded_kgen_pack()` is
# served by the compiler, a `kgen.struct.load_indirect` of the
# `VariadicPack`'s references, where upstream declares it (roadmap R1922).
# `_RegisterPackType`, `expect`, `likely`, `unlikely`, and `assume` are
# upstream's, verbatim: the first's `!kgen.struct` field is Mojito's
# compiler-private tuple storage of the same elements.
# Omitted: the gather, scatter, masked load and store, prefetch, and GPU
# intrinsics.
"""Defines intrinsics.

You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import llvm_intrinsic
```
"""

from std.collections.string.string_span import _get_kgen_string
from std.string import StaticString


@inline(.nodebug)
def llvm_intrinsic[
    intrin: StaticString,
    type: TrivialRegisterPassable,
    *types: AnyType,
    has_side_effect: Bool = True,
](*args: *types) -> type:
    """Calls an LLVM intrinsic with the name `intrin` and return type `type`.

    Parameters:
        intrin: The name of the llvm intrinsic.
        type: The return type of the intrinsic.
        types: The argument types for the function.
        has_side_effect: If `True` the intrinsic will have side effects,
            otherwise its pure.

    Args:
        args: The arguments to the function.

    Returns:
        The result of calling the llvm intrinsic with no arguments.
    """

    var loaded_pack = args.get_loaded_kgen_pack()

    comptime intrin_kgen_string = _get_kgen_string[intrin]()

    comptime if type == NoneType:
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin=intrin_kgen_string,
            _type=None,
            hasSideEffects=has_side_effect.__mlir_i1__(),
        ](loaded_pack)
        return rebind[type](None)

    else:
        return __mlir_op.`pop.call_llvm_intrinsic`[
            intrin=intrin_kgen_string,
            _type=type,
            hasSideEffects=has_side_effect.__mlir_i1__(),
        ](loaded_pack)


# ===----------------------------------------------------------------------=== #
# Transitional type used for llvm_intrinsic
# ===----------------------------------------------------------------------=== #


struct _RegisterPackType[*Ts: TrivialRegisterPassable](TrivialRegisterPassable):
    comptime _mlir_type = __mlir_type[
        `!kgen.struct<`, ~Self.Ts.values, ` isParamPack>`
    ]

    var _mlir_value: Self._mlir_type

    @inline(.nodebug)
    def __getitem_param__[i: Int](self) -> Self.Ts[i]:
        """Get the element.

        Parameters:
            i: The element index.

        Returns:
            The tuple element at the requested index.
        """
        return __mlir_op.`kgen.struct.extract`[index=i.__mlir_index__()](
            self._mlir_value
        )


# ===----------------------------------------------------------------------=== #
# expect
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def expect[T: TrivialRegisterPassable, //, expected_val: T](val: T) -> T:
    """Provides information about expected (the most probable) value of `val`,
    which can be used by optimizers.

    Parameters:
        T: The type of the input value.
        expected_val: The expected value of `val`.

    Args:
        val: The input value.

    Returns:
        The input value.

    Notes:
        Only works with integer/boolean types.
    """
    if __is_run_in_comptime_interpreter:
        return val
    return llvm_intrinsic["llvm.expect", T, has_side_effect=False](
        val, expected_val
    )


# ===----------------------------------------------------------------------=== #
# likely
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def likely(val: Bool) -> Bool:
    """Provides information that the most probable value of `val` is going to be
    `True`. This information can be used by optimizers.

    Args:
        val: The input value which is likely to be `True` most of the time.

    Returns:
        The input value.
    """
    return expect[True](val)


# ===----------------------------------------------------------------------=== #
# unlikely
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def unlikely(val: Bool) -> Bool:
    """Provides information that the most probable value of `val` is going to be
    `False`. This information can be used by optimizers.

    Args:
        val: The input value which is likely to be `False` most of the time.

    Returns:
        The input value.
    """
    return expect[False](val)


# ===----------------------------------------------------------------------=== #
# assume
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def assume(val: Bool):
    """Signals to the optimizer that the condition is always true. This allows
    the optimizer to optimize the code.

    Args:
      val: The input value which is assumed to be `True`.
    """
    if __is_run_in_comptime_interpreter:
        return
    llvm_intrinsic["llvm.assume", NoneType](val)
