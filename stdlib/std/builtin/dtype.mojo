# Upstream's `std/builtin/dtype.mojo`, in part: `_uint_type_of_width`,
# `_integral_type_of`, and `_unsigned_integral_type_of`. `DType` itself
# stays a compiler type (R160). The chain's `_uint1`,
# `_uint2`, `_uint4`, `uint128`, and `uint256` arms are omitted until R65
# admits those dtypes; the second assert rejects their widths explicitly.
"""Implements the DType class.

These are Mojo built-ins, so you don't need to import them.
"""

from std.sys import bit_width_of


@always_inline("builtin")
def _uint_type_of_width[width: Int]() -> DType:
    # fmt: off
    comptime assert width in (
        1, 2, 4, 8, 16, 32, 64, 128, 256,
    ), "width must be 1, 2, 4, 8, 16, 32, 64, 128, or 256"
    comptime assert width in (
        8, 16, 32, 64,
    ), "a uint DType of width 1, 2, 4, 128, or 256 is not supported yet"
    return (
        DType.uint8 if width == 8 else
        DType.uint16 if width == 16 else
        DType.uint32 if width == 32 else
        DType.uint64
    )
    # fmt: on


# ===-------------------------------------------------------------------===#
# integral_type_of
# ===-------------------------------------------------------------------===#


@inline(.nodebug)
def _integral_type_of[dtype: DType]() -> DType:
    """Gets the integral type which has the same bitwidth as the input type."""

    comptime if dtype.is_integral():
        return dtype
    elif dtype.is_float8():
        return DType.int8
    elif dtype.is_half_float():
        return DType.int16
    elif dtype == DType.float32:
        return DType.int32
    elif dtype == DType.float64:
        return DType.int64
    else:
        comptime assert False, "unexpected dtype in _integral_type_of"


# ===-------------------------------------------------------------------===#
# _unsigned_integral_type_of
# ===-------------------------------------------------------------------===#


@inline(.nodebug)
def _unsigned_integral_type_of[dtype: DType]() -> DType:
    """Gets the unsigned integral type which has the same bitwidth as
    the input type."""

    comptime if dtype.is_unsigned():
        return dtype
    elif dtype.is_integral():
        return _uint_type_of_width[bit_width_of[dtype]()]()
    elif dtype.is_float8():
        return DType.uint8
    elif dtype.is_half_float():
        return DType.uint16
    elif dtype == DType.float32:
        return DType.uint32
    elif dtype == DType.float64:
        return DType.uint64
    else:
        comptime assert False, "unexpected dtype in _unsigned_integral_type_of"
