# Upstream's `std/math/math.mojo`, holding its operation traits, the
# rounding helpers `floor`/`ceil`/`trunc`/`ceildiv`, and the `SIMD` overload
# of `iota`, verbatim (the buffer overload is omitted). The prelude exports
# `Absable`, `Powable`, and `Roundable`, as upstream's does; `abs`/`round`/
# `divmod` are still compiler builtins (R1458).
#
# Each helper is generic over the trait that supplies its dunder. The
# compiler-modelled `Int`, `UInt`, and `Float64` conform as upstream's `SIMD`
# declares, other `SIMD` values only in part (R1457), and each answers a
# requirement intrinsically.

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
