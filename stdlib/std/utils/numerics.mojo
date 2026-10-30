# Upstream's `std/utils/numerics.mojo`, in part: `inf`, `neg_inf`,
# `max_finite`, `min_finite`, `max_or_inf`, and `min_or_neg_inf`, the
# functions behind `SIMD`'s `MAX`, `MIN`, `MAX_FINITE`, and `MIN_FINITE`.
# The `comptime match` cases for the `float8_*` and `bfloat16` dtypes, and
# `max_finite`'s `float4_e2m1fn`, `float8_*`, and `bfloat16` arms, are
# omitted: Mojito has none of those dtypes (R65). The two `bool` arms spell
# `Scalar[DType.bool](...)` where upstream spells `Scalar(...)`, since a `_`
# the selected initializer's `out self` binds is not yet solved (R1544).
# `FPUtils`, `nan`, `isnan`, `isinf`, `isfinite`, `get_accum_type`,
# `nextafter`, and `FlushDenormals` are not ported.
"""Defines utilities to work with numeric types.

You can import these APIs from the `utils` package. For example:

```mojo
from std.utils.numerics import FPUtils
```
"""

from std.builtin.dtype import _unsigned_integral_type_of

# ===----------------------------------------------------------------------=== #
# inf
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def inf[dtype: DType]() -> Scalar[dtype]:
    """Gets a +inf value for the given dtype.

    Constraints:
        Can only be used for FP dtypes.

    Parameters:
        dtype: The value dtype.

    Returns:
        The +inf value of the given dtype.
    """
    comptime assert (
        dtype.is_floating_point()
    ), "Only floating point dtypes support +inf."

    comptime match dtype:
        case .float16:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"inf"> : !kgen.scalar<f16>`,
            )
        case .float32:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"inf"> : !kgen.scalar<f32>`,
            )
        case .float64:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"inf"> : !kgen.scalar<f64>`,
            )
        case _:
            comptime assert False, "unsupported float type"


# ===----------------------------------------------------------------------=== #
# neg_inf
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def neg_inf[dtype: DType]() -> Scalar[dtype]:
    """Gets a -inf value for the given dtype.

    Constraints:
        Can only be used for FP dtypes.

    Parameters:
        dtype: The value dtype.

    Returns:
        The -inf value of the given dtype.
    """
    comptime assert (
        dtype.is_floating_point()
    ), "Only floating point dtypes support -inf."

    comptime match dtype:
        case .float16:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"-inf"> : !kgen.scalar<f16>`,
            )
        case .float32:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"-inf"> : !kgen.scalar<f32>`,
            )
        case .float64:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"-inf"> : !kgen.scalar<f64>`,
            )
        case _:
            comptime assert False, "unsupported float type"


# ===----------------------------------------------------------------------=== #
# max_finite
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def max_finite[dtype: DType]() -> Scalar[dtype]:
    """Returns the maximum finite value of type.

    Parameters:
        dtype: The value dtype.

    Returns:
        The maximum representable value of the type. Does not include infinity
        for floating-point types.
    """

    comptime if dtype.is_unsigned():
        return ~Scalar[dtype](0)
    elif dtype.is_integral():
        return Scalar[dtype](
            ~Scalar[_unsigned_integral_type_of[dtype]()](0) >> 1
        )
    elif dtype == .float16:
        return 65504
    elif dtype == .float32:
        return 3.40282346638528859812e38
    elif dtype == .float64:
        return 1.79769313486231570815e308
    elif dtype == .bool:
        return Scalar[DType.bool](True)._refine[dtype]()
    else:
        comptime assert False, "max_finite() called on unsupported dtype"


# ===----------------------------------------------------------------------=== #
# min_finite
# ===----------------------------------------------------------------------=== #


@inline(.always)
def min_finite[dtype: DType]() -> Scalar[dtype]:
    """Returns the minimum (lowest) finite value of type.

    Parameters:
        dtype: The value dtype.

    Returns:
        The minimum representable value of the type. Does not include negative
        infinity for floating-point types.
    """

    comptime if dtype.is_unsigned():
        return 0
    elif dtype.is_integral():
        return -max_finite[dtype]() - 1
    elif dtype.is_floating_point():
        return -max_finite[dtype]()
    elif dtype == .bool:
        return Scalar[DType.bool](False)._refine[dtype]()
    else:
        comptime assert False, "min_finite() called on unsupported dtype"


# ===----------------------------------------------------------------------=== #
# max_or_inf
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def max_or_inf[dtype: DType]() -> Scalar[dtype]:
    """Returns the maximum (potentially infinite) value of type.

    Parameters:
        dtype: The value dtype.

    Returns:
        The maximum representable value of the type. Can include infinity for
        floating-point types.
    """

    comptime if dtype.is_floating_point():
        # TODO: some floating point types don't support inf
        return inf[dtype]()
    else:
        return max_finite[dtype]()


# ===----------------------------------------------------------------------=== #
# min_or_neg_inf
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def min_or_neg_inf[dtype: DType]() -> Scalar[dtype]:
    """Returns the minimum (potentially negative infinite) value of type.

    Parameters:
        dtype: The value dtype.

    Returns:
        The minimum representable value of the type. Can include negative
        infinity for floating-point types.
    """

    comptime if dtype.is_floating_point():
        # TODO: some floating point types don't support inf
        return neg_inf[dtype]()
    else:
        return min_finite[dtype]()
