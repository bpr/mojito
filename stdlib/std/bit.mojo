# Upstream's `std/bit/bit.mojo` surface for the proof subset: the
# counting and reversing functions over `llvm_intrinsic`, verbatim, and a
# `rotate_bits_left` over `UInt64`.
# Omitted: `byte_swap` (it needs `std.sys.info.bit_width_of`), and the
# rest of upstream's module.
"""Provides functions for bit manipulation.

You can import these APIs from the `bit` package. For example:

```mojo
from std.bit import count_leading_zeros
```
"""

from std.sys import llvm_intrinsic


@inline(.nodebug)
def count_leading_zeros[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[dtype, width]:
    """Counts the per-element number of leading zeros in a SIMD vector.

    Parameters:
        dtype: `DType` used for the computation.
        width: SIMD width used for the computation.

    Constraints:
        The element type of the input vector must be integral.

    Args:
        val: The input value.

    Returns:
        A SIMD value where the element at position `i` contains the number of
        leading zeros at position `i` of the input value.
    """
    comptime assert dtype.is_integral(), "must be integral"
    return llvm_intrinsic["llvm.ctlz", type_of(val), has_side_effect=False](
        val, False
    )


@inline(.nodebug)
def count_trailing_zeros[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[dtype, width]:
    """Counts the per-element number of trailing zeros in a SIMD vector.

    Parameters:
        dtype: `dtype` used for the computation.
        width: SIMD width used for the computation.

    Constraints:
        The element type of the input vector must be integral.

    Args:
        val: The input value.

    Returns:
        A SIMD value where the element at position `i` contains the number of
        trailing zeros at position `i` of the input value.
    """
    comptime assert dtype.is_integral(), "must be integral"
    return llvm_intrinsic["llvm.cttz", type_of(val), has_side_effect=False](
        val, False.__mlir_i1__()
    )


@inline(.nodebug)
def bit_reverse[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[dtype, width]:
    """Element-wise reverses the bitpattern of a SIMD vector of integer values.

    Parameters:
        dtype: `dtype` used for the computation.
        width: SIMD width used for the computation.

    Args:
        val: The input value.

    Constraints:
        The element type of the input vector must be integral.

    Returns:
        A SIMD value where the element at position `i` has a reversed bitpattern
        of an integer value of the element at position `i` of the input value.
    """
    comptime assert dtype.is_integral(), "must be integral"
    return llvm_intrinsic[
        "llvm.bitreverse", type_of(val), has_side_effect=False
    ](val)


@inline(.nodebug)
def pop_count[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[dtype, width]:
    """Counts the number of bits set in a SIMD vector of integer values.

    Parameters:
        dtype: `dtype` used for the computation.
        width: SIMD width used for the computation.

    Constraints:
        The element type of the input vector must be integral.

    Args:
        val: The input value.

    Returns:
        A SIMD value where the element at position `i` contains the number of
        bits set in the element at position `i` of the input value.
    """
    comptime assert dtype.is_integral(), "must be integral"
    return llvm_intrinsic["llvm.ctpop", type_of(val), has_side_effect=False](
        val
    )


@inline(.nodebug)
def rotate_bits_left[shift: Int](value: UInt64) -> UInt64:
    comptime if shift == 0:
        return value
    return (value << UInt64(shift)) | (value >> UInt64(64 - shift))
