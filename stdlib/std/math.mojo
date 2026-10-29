# Upstream's `std/math/math.mojo`, holding its operation traits, the
# rounding helpers `floor`/`ceil`/`trunc`/`ceildiv`, the `SIMD` overload
# of `iota`, `fma`, `clamp`, and both overloads each of `max` and `min`,
# verbatim (the buffer overload of `iota` is omitted). The prelude exports
# `Absable`, `Powable`, `Roundable`, `max`, and `min`, as upstream's does;
# `abs`/`round`/`divmod` are still compiler builtins (R1458).
#
# Each helper is generic over the trait that supplies its dunder. `SIMD`
# declares `Absable`, `Ceilable`, `Floorable`, and `Truncable` in
# `std.simd`; for the other operation traits the compiler-modelled `Int`,
# `UInt`, and `Float64` conform as upstream's `SIMD` declares, other `SIMD`
# values do not yet (R1623, R1567, R1582), and each answers a requirement
# intrinsically.

from std.sys import llvm_intrinsic


trait Ceilable:
    def __ceil__(self) -> Self:
        ...


trait Floorable:
    def __floor__(self) -> Self:
        ...


trait CeilDivable:
    def __ceildiv__(self, denominator: Self) -> Self:
        ...


trait CeilDivableRaising:
    def __ceildiv__(self, denominator: Self) raises -> Self:
        ...


trait Truncable:
    def __trunc__(self) -> Self:
        ...


trait Absable:
    def __abs__(self) -> Self:
        ...


trait DivModable(ImplicitlyCopyable):
    def __divmod__(self, denominator: Self) -> Tuple[Self, Self]:
        ...


trait Powable:
    def __pow__(self, exp: Self) -> Self:
        ...


trait Roundable:
    def __round__(self) -> Self:
        ...

    def __round__(self, ndigits: Int) -> Self:
        ...


@inline(.always)
def floor[T: Floorable](value: T) -> T:
    return value.__floor__()


@inline(.always)
def ceil[T: Ceilable](value: T) -> T:
    return value.__ceil__()


@inline(.always)
def trunc[T: Truncable](value: T) -> T:
    return value.__trunc__()


@inline(.always)
def ceildiv[T: CeilDivable](numerator: T, denominator: T) -> T:
    return numerator.__ceildiv__(denominator)


def iota[
    dtype: DType, width: Int
](offset: Scalar[dtype] = Scalar[dtype](0)) -> SIMD[dtype, width]:
    """Creates a SIMD vector containing an increasing sequence, starting from
    offset.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        offset: The value to start the sequence at. Default is zero.

    Returns:
        An increasing sequence of values, starting from offset.
    """

    comptime if width == 1:
        return offset

    comptime step_dtype = dtype if dtype.is_integral() else DType.int
    var step: SIMD[step_dtype, width]
    if __is_run_in_comptime_interpreter:
        step = 0

        comptime for i in range(width):
            step[i] = Scalar[step_dtype](i)
    else:
        step = llvm_intrinsic[
            "llvm.stepvector", SIMD[step_dtype, width], has_side_effect=False
        ]()
    return step.cast[dtype]() + offset


# ===----------------------------------------------------------------------=== #
# fma
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def fma[
    dtype: DType, width: SIMDLength, //
](
    a: SIMD[dtype, width],
    b: SIMD[dtype, width],
    c: SIMD[dtype, width],
) -> SIMD[
    dtype, width
]:
    """Performs elementwise `fma` (fused multiply-add) on the inputs.

    Each element in the result SIMD vector is $(A_i * B_i) + C_i$, where $A_i$,
    $B_i$ and $C_i$ are elements at index $i$ in a, b, and c respectively.

    Parameters:
        dtype: The `dtype` of the input SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        a: The first vector of inputs.
        b: The second vector of inputs.
        c: The third vector of inputs.

    Returns:
        Elementwise `fma` of a, b and c.
    """
    return a.fma(b, c)


# ===----------------------------------------------------------------------=== #
# clamp
# ===----------------------------------------------------------------------=== #


def clamp[
    dtype: DType, width: SIMDLength, //
](
    val: SIMD[dtype, width],
    lower_bound: type_of(val),
    upper_bound: type_of(val),
) -> type_of(val):
    """Clamps the values in a SIMD vector to be in a certain range.

    Clamp cuts values in the input SIMD vector off at the upper bound and
    lower bound values. For example,  SIMD vector `[0, 1, 2, 3]` clamped to
    a lower bound of 1 and an upper bound of 2 would return `[1, 1, 2, 2]`.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        val: The value to clamp.
        lower_bound: Minimum of the range to clamp to.
        upper_bound: Maximum of the range to clamp to.

    Returns:
        A SIMD vector containing x clamped to be within lower_bound and
        upper_bound.
    """
    return val.clamp(lower_bound, upper_bound)


# ===----------------------------------------------------------------------=== #
# max
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def max[dtype: DType, //](x: SIMD[dtype, _], y: type_of(x), /) -> type_of(x):
    """Performs elementwise maximum of x and y.

    An element of the result SIMD vector will be the maximum of the
    corresponding elements in x and y.

    Constraints:
        The type of the inputs must be numeric or boolean.

    Parameters:
            dtype: The data type of the SIMD vector.

    Args:
        x: First SIMD vector.
        y: Second SIMD vector.

    Returns:
        A SIMD vector containing the elementwise maximum of x and y.
    """

    comptime assert (
        x.dtype == .bool or x.dtype.is_numeric()
    ), "the SIMD type must be numeric or boolean"

    return {mlir_value = __mlir_op.`pop.max`(x._mlir_value, y._mlir_value)}


@inline(.always)
def max[T: Copyable & Comparable & Deinitable](x: T, *ys: T) -> T:
    """Gets the maximum value from a sequence of values.

    Parameters:
        T: A type that is both copyable and comparable with greater than.

    Args:
        x: The first value to compare.
        ys: Zero or more additional values to compare.

    Returns:
        The maximum value from the input sequence.
    """
    var res = x.copy()
    for y in ys:
        if y > res:
            res = y.copy()
    return res.copy()


# ===----------------------------------------------------------------------=== #
# min
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def min[dtype: DType, //](x: SIMD[dtype, _], y: type_of(x), /) -> type_of(x):
    """Gets the elementwise minimum of x and y.

    An element of the result SIMD vector will be the minimum of the
    corresponding elements in x and y.

    Constraints:
        The type of the inputs must be numeric or boolean.

    Parameters:
         dtype: The data type of the SIMD vector.

    Args:
        x: First SIMD vector.
        y: Second SIMD vector.

    Returns:
        A SIMD vector containing the elementwise minimum of x and y.
    """

    comptime assert (
        x.dtype == .bool or x.dtype.is_numeric()
    ), "the SIMD type must be numeric or boolean"

    return {mlir_value = __mlir_op.`pop.min`(x._mlir_value, y._mlir_value)}


@inline(.always)
def min[T: Copyable & Comparable & Deinitable](x: T, *ys: T) -> T:
    """Gets the minimum value from a sequence of values.

    Parameters:
        T: A type that is both copyable and comparable with less than.

    Args:
        x: The first value to compare.
        ys: Zero or more additional values to compare.

    Returns:
        The minimum value from the input sequence.
    """
    var res = x.copy()
    for y in ys:
        if y < res:
            res = y.copy()
    return res.copy()
