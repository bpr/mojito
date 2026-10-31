# Upstream's `std/math/math.mojo`, the `std.math` package's module, holding
# its operation traits, the rounding helpers `floor`/`ceil`/`trunc`/
# `ceildiv`, the plugin-hook signatures `_UnaryMathPluginHookFnType` and
# `_BinaryMathPluginHookFnType`, `_ldexp_impl` and `ldexp` (with its
# AVX-512 `scalef` arm), `_Expable`, `_exp_taylor`, both overloads of
# `exp`, `_exp_impl`, `copysign`, the `SIMD` overload of `iota`,
# `fma`, `modf` over `std.simd`'s `_modf`, `clamp`, both overloads each of
# `max` and `min`, `abs`, and the libm constants `_SQRT2`, `_LN2`, `_LN2_HI_F64`, and `_LN2_LO_F64`,
# `_frexp_mask1`, `_frexp_mask2`, `frexp`, `_log_base`, `log`, and their
# kernels `_assert_float64_supported`, `_log_kernel`, `_is_zero_bits`,
# `_is_subnormal_bits`, `_frexp_bits`, `_log_reduce`, `_log_combine`, and
# `_log_accurate`, verbatim (the buffer overload of `iota` is omitted).
# The traits precede the functions, where upstream interleaves them, save
# `_Expable`, which keeps upstream's position. The
# masks' `bfloat16` cases are omitted (R65), and so
# are `log`'s `is_nvidia_gpu()` arm through `log2` and `exp`'s `is_gpu()`
# arm through `exp2` (R1635). The
# `std.string` import is explicit, as bundled modules' are. Upstream's
# `from . import` line omits `nextafter`, which the package does not yet
# re-export. The prelude exports `Absable`, `Powable`, `Roundable`, `abs`, `max`, and
# `min`, as upstream's does; `round`/`divmod` are still compiler builtins
# (R1458).
#
# Each helper is generic over the trait that supplies its dunder. `SIMD`
# declares `Absable`, `Ceilable`, `Floorable`, and `Truncable` in
# `std.simd`; for the other operation traits the compiler-modelled `Int`,
# `UInt`, and `Float64` conform as upstream's `SIMD` declares, other `SIMD`
# values do not yet (R1623, R1567, R1582), and each answers a requirement
# intrinsically.

from std.sys import (
    CompilationTarget,
    bit_width_of,
    llvm_intrinsic,
    simd_width_of,
    size_of,
)

from std.builtin.dtype import _integral_type_of
from std.string import StaticString

from std.utils.numerics import FPUtils
from std.simd import _modf
from std.memory import bitcast
from std.utils.static_tuple import StaticTuple

from std._plugin import CurrentPlugin

from . import pi, inf, isfinite, isinf, isnan, nan
from .constants import log2e
from .polynomial import polynomial_evaluate


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


comptime _UnaryMathPluginHookFnType = def[dtype: DType, width: SIMDLength, //](
    SIMD[dtype, width]
) thin -> SIMD[dtype, width]
"""Plugin-hook signature for `PluginHooks.unary_math_fn`."""

comptime _BinaryMathPluginHookFnType = def[dtype: DType, width: SIMDLength, //](
    SIMD[dtype, width], SIMD[dtype, width]
) thin -> SIMD[dtype, width]
"""Plugin-hook signature for `PluginHooks.binary_math_fn`."""


# ===----------------------------------------------------------------------=== #
# ldexp
# ===----------------------------------------------------------------------=== #


@inline(.always)
def _ldexp_impl[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width], exp: SIMD[dtype, width]) -> SIMD[dtype, width]:
    """Computes elementwise ldexp function.

    The ldexp function multiplies a floating point value x by the number 2
    raised to the exp power. I.e. $ldexp(x,exp)$ calculate the value of $x *
    2^{exp}$ and is used within the $erf$ function.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: SIMD vector of floating point values.
        exp: SIMD vector containing the exponents.

    Returns:
        Vector containing elementwise result of ldexp on x and exp.
    """

    comptime hardware_width = simd_width_of[dtype]()

    comptime if (
        CompilationTarget.has_avx512f()
        and dtype == .float32
        and width >= hardware_width
    ):
        var res: SIMD[dtype, width] = 0
        var zero: SIMD[dtype, hardware_width] = 0

        comptime for idx in range(width // hardware_width):
            comptime i = idx * hardware_width
            # On AVX512, we can use the scalef intrinsic to compute the ldexp
            # function.
            var part = llvm_intrinsic[
                "llvm.x86.avx512.mask.scalef.ps.512",
                SIMD[dtype, hardware_width],
                has_side_effect=False,
            ](
                x.slice[hardware_width, offset=i](),
                exp.slice[hardware_width, offset=i](),
                zero,
                Int16(-1),
                Int32(4),
            )
            res = res.insert[offset=i](part)

        return res

    comptime integral_type = FPUtils[dtype].integral_type
    var m = exp.cast[integral_type]() + SIMD[integral_type, width](
        FPUtils[dtype].exponent_bias()
    )

    return x * type_of(x)(
        from_bits=m
        << SIMD[integral_type, width](FPUtils[dtype].mantissa_width())
    )


@inline(.always)
def ldexp[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width], exp: SIMD[.int32, width]) -> SIMD[
    dtype, width
] where dtype.is_floating_point():
    """Computes elementwise ldexp function.

    The ldexp function multiplies a floating point value x by the number 2
    raised to the exp power. I.e. $ldexp(x,exp)$ calculate the value of $x *
    2^{exp}$ and is used within the $erf$ function.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: SIMD vector of floating point values.
        exp: SIMD vector containing the exponents.

    Returns:
        Vector containing elementwise result of ldexp on x and exp.
    """
    return _ldexp_impl(x, exp.cast[dtype]())


# ===----------------------------------------------------------------------=== #
# exp
# ===----------------------------------------------------------------------=== #


trait _Expable:
    """Trait for types that support the exp function."""

    def __exp__(self) -> Self:
        """Computes the exponential of the input value.

        Returns:
            The exponential of the input value.
        """
        ...


@inline(.always)
def _exp_taylor[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    comptime coefficients = List(
        [
            Scalar[dtype](1.0),
            1.0,
            0.5,
            0.16666666666666666667,
            0.041666666666666666667,
            0.0083333333333333333333,
            0.0013888888888888888889,
            0.00019841269841269841270,
            0.000024801587301587301587,
            2.7557319223985890653e-6,
            2.7557319223985890653e-7,
            2.5052108385441718775e-8,
            2.0876756987868098979e-9,
        ]
    )
    return polynomial_evaluate[
        coefficients[:] if dtype == .float64 else coefficients[:8],
    ](x)


@inline(.always)
def exp[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[dtype, width] where dtype.is_floating_point():
    """Calculates elementwise exponential of the input vector.

    Given an input vector $X$ and an output vector $Y$, sets $Y_i = e^{X_i}$ for
    each position $i$ in the input vector (where $e$ is the mathematical constant
    $e$).

    Constraints:
        The input must be a floating-point type.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: The input SIMD vector.

    Returns:
        A SIMD vector containing $e$ raised to the power $X_i$ where $X_i$ is an
        element in the input SIMD vector.
    """

    if __is_run_in_comptime_interpreter:
        return _exp_impl(x)

    comptime hook = CurrentPlugin.unary_math_fn["exp", dtype, width]
    comptime if hook:
        return comptime (hook.value())(x)

    return _exp_impl(x)


@inline(.always)
def _exp_impl[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[dtype, width] where dtype.is_floating_point():
    comptime neg_ln2 = -0.69314718055966295651160180568695068359375

    comptime if dtype not in (DType.float32, DType.float64):
        return exp(x.cast[.float32]()).cast[dtype]()

    var min_val: SIMD[dtype, width]
    var max_val: SIMD[dtype, width]

    comptime if dtype == .float64:
        min_val = -709.436139303
        max_val = 709.437
    else:
        min_val = -88.3762626647949
        max_val = 88.3762626647950

    var xc = x.clamp(min_val, max_val)
    var k = floor(xc.fma(log2e, 0.5))
    var r = k.fma(neg_ln2, xc)
    return max(_ldexp_impl(_exp_taylor(r), k), xc)


@inline(.always)
def exp[T: _Expable](x: T) -> T:
    """Computes the exponential of the input value.

    Parameters:
        T: The type of the input value.

    Args:
        x: The input value.

    Returns:
        The exponential of the input value.
    """
    return x.__exp__()


# ===----------------------------------------------------------------------=== #
# frexp
# ===----------------------------------------------------------------------=== #


@inline(.always)
def _frexp_mask1[
    dtype: DType, width: Int
]() -> SIMD[_integral_type_of[dtype](), width]:
    comptime match dtype:
        case .float16:
            return 0x7C00
        case .float32:
            return 0x7F800000
        case _:
            comptime assert dtype == .float64, "unhandled fp type"
            return 0x7FF0000000000000


@inline(.always)
def _frexp_mask2[
    dtype: DType, width: Int
]() -> SIMD[_integral_type_of[dtype](), width]:
    comptime match dtype:
        case .float16:
            return 0x3800
        case .float32:
            return 0x3F000000
        case _:
            comptime assert dtype == .float64, "unhandled fp type"
            return 0x3FE0000000000000


def frexp[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> StaticTuple[
    SIMD[dtype, width], 2
] where dtype.is_floating_point():
    """Breaks floating point values into a fractional part and an exponent part.
    This follows C and Python in increasing the exponent by 1 and normalizing the
    fraction from 0.5 to 1.0 instead of 1.0 to 2.0.

    Constraints:
        The input must be a floating-point type.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: The input values.

    Returns:
        A tuple of two SIMD vectors containing the fractional and exponent parts
        of the input floating point values.
    """

    # Based on the implementation in boost/simd/arch/common/simd/function/ifrexp.hpp

    comptime T = SIMD[dtype, width]
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime zero = T(0)
    # Add one to the resulting exponent up by subtracting 1 from the bias
    comptime exponent_bias = FPUtils[dtype].exponent_bias() - 1
    comptime mantissa_width = FPUtils[dtype].mantissa_width()
    comptime subnorm_scale = T(
        from_bits=TInt(
            (mantissa_width + FPUtils[dtype].exponent_bias()) << mantissa_width
        )
    )

    var mask1 = _frexp_mask1[dtype, width]()
    var mask2 = _frexp_mask2[dtype, width]()

    var subnorm_test = (x._to_bits_signed() & mask1).eq(0) & x.ne(0)
    var t = subnorm_test.select(TInt(mantissa_width), TInt(0))
    var x0 = subnorm_test.select(subnorm_scale * x, x)

    var x_int = x0._to_bits_signed()
    var selector = x0.ne(zero)
    var r1 = mask1 & x_int
    var mb = x_int & ~mask1
    r1 = (r1 >> TInt(mantissa_width)) - TInt(exponent_bias)

    var r0 = mb | mask2

    var test1 = r1.gt(TInt(FPUtils[dtype].max_exponent()))
    r1 = (~test1 & selector).select(r1, TInt(0))

    r1 -= t
    return StaticTuple[size=2](
        selector.select(
            bitcast[dtype, width](r0) + test1.select(x0, zero), zero
        ),
        T(r1),
    )


# ===----------------------------------------------------------------------=== #
# log
# ===----------------------------------------------------------------------=== #


@inline(.always)
def _log_base[
    dtype: DType, width: SIMDLength, //, base: Int
](x: SIMD[dtype, width]) -> SIMD[dtype, width] where dtype.is_floating_point():
    """Performs elementwise log of a SIMD vector with a specific base.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.
        base: The logarithm base.

    Args:
        x: Vector to perform logarithm operation on.

    Returns:
        Vector containing result of performing logarithm on x.
    """
    # Based on the Cephes approximation.
    comptime sqrt2_div_2 = 0.70710678118654752440

    comptime assert base == 2 or base == 27, "input base must be either 2 or 27"

    var frexp_result = frexp(x)
    var x1 = frexp_result[0]
    var exp = frexp_result[1]
    exp = x1.lt(sqrt2_div_2).select(exp - 1, exp)
    x1 = x1.lt(sqrt2_div_2).select(x1 + x1, x1) - 1

    var x2 = x1 * x1
    var x3 = x2 * x1

    var y = (
        polynomial_evaluate[
            [
                Scalar[dtype](3.3333331174e-1),
                -2.4999993993e-1,
                2.0000714765e-1,
                -1.6668057665e-1,
                1.4249322787e-1,
                -1.2420140846e-1,
                1.1676998740e-1,
                -1.1514610310e-1,
                7.0376836292e-2,
            ],
        ](x1)
        * x3
    )
    y = x1 + x2.fma(-0.5, y)

    # TODO: fix this hack
    comptime if base == 27:  # Natural log
        comptime ln2 = 0.69314718055994530942
        y = exp.fma(ln2, y)
    else:
        y = y.fma(log2e, exp)
    y = x.eq(inf[dtype]()).select(x, y)
    return x.eq(0).select(Scalar[dtype].MIN, x.gt(0).select(y, nan[dtype]()))


@inline(.always)
def log[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> type_of(x) where dtype.is_floating_point():
    """Performs elementwise natural log (base E) of a SIMD vector.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: Vector to perform logarithm operation on.

    Returns:
        Vector containing result of performing natural log base E on x.
    """

    _assert_float64_supported[dtype, "log"]()

    comptime if dtype == .float64:
        var r = _log_accurate(x)
        r = x.eq(inf[dtype]()).select(x, r)
        return x.eq(0).select(-inf[dtype](), x.gt(0).select(r, nan[dtype]()))
    elif size_of[dtype]() < size_of[DType.float32]():
        return log(x.cast[.float32]()).cast[dtype]()

    if __is_run_in_comptime_interpreter:
        return _log_base[27](x)

    comptime if CurrentPlugin.flushes_subnormals[dtype]:
        # Subnormals read as zero in comparisons and `frexp`, so reduce from
        # the bits instead.
        var r = _log_accurate(x)
        r = x.eq(inf[dtype]()).select(x, r)
        var positive = x.gt(0) | (
            _is_subnormal_bits(x) & x._to_bits_signed().gt(0)
        )
        return _is_zero_bits(x).select(
            -inf[dtype](), positive.select(r, nan[dtype]())
        )

    return _log_base[27](x)

# ===----------------------------------------------------------------------=== #
# copysign
# ===----------------------------------------------------------------------=== #


@inline(.always)
def copysign[
    dtype: DType, width: SIMDLength, //
](magnitude: SIMD[dtype, width], sign: SIMD[dtype, width]) -> SIMD[
    dtype, width
]:
    """Returns a value with the magnitude of the first operand and the sign of
    the second operand.

    Constraints:
        The type of the input must be numeric.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        magnitude: The magnitude to use.
        sign: The sign to copy.

    Returns:
        Copies the sign from sign to magnitude.
    """
    comptime assert dtype.is_numeric(), "operands must be a numeric type"

    comptime if dtype.is_unsigned():
        return magnitude
    elif dtype.is_integral():
        var mag_abs = abs(magnitude)
        return sign.lt(0).select(-mag_abs, mag_abs)
    return llvm_intrinsic[
        "llvm.copysign", SIMD[dtype, width], has_side_effect=False
    ](magnitude, sign)


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
# modf
# ===----------------------------------------------------------------------=== #


def modf[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> Tuple[type_of(x), type_of(x)]:
    """Computes the integral and fractional part of the value.

    Parameters:
        dtype: The `dtype` of the input and output SIMD vector.
        width: The width of the input and output SIMD vector.

    Args:
        x: The input value.

    Returns:
        A tuple containing the integral and fractional part of the value.
    """
    return _modf(x)



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
# abs
# ===----------------------------------------------------------------------=== #


@inline(.always)
def abs[T: Absable](value: T) -> T:
    """Get the absolute value of the given object.

    Parameters:
        T: The type conforming to Absable.

    Args:
        value: The object to get the absolute value of.

    Returns:
        The absolute value of the object.
    """
    return value.__abs__()


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


# ===----------------------------------------------------------------------=== #
# Pure-Mojo libm implementations
# ===----------------------------------------------------------------------=== #

comptime _SQRT2 = 1.41421356237309504880168872420969807856967187537694

comptime _LN2 = 0.69314718055994530941723212145817656807550013436026

# ln2 = _LN2_HI_F64 + _LN2_LO_F64, where _LN2_HI_F64 has 32 significant bits
# (21 trailing zero bits), so k * _LN2_HI_F64 is exact for |k| < 2^21.
comptime _LN2_HI_F64 = 6.93147180369123816490e-01
comptime _LN2_LO_F64 = 1.90821492927058770002e-10


# ===----------------------------------------------------------------------=== #
# Shared kernels
# ===----------------------------------------------------------------------=== #


@inline(.always)
def _assert_float64_supported[dtype: DType, name: StaticString]():
    comptime assert dtype != .float64 or CurrentPlugin.supports_float64, (
        "`" + name + "` does not support float64 on this target"
    )


@inline(.always)
def _log_kernel[
    dtype: DType, width: SIMDLength, //
](f: SIMD[dtype, width]) -> type_of(f):
    """Returns `log(1 + f) - f + f*f/2` for `1 + f` in `[sqrt(2)/2, sqrt(2))`.

    FreeBSD msun k_log.h (float64) and k_logf.h / e_logf.c (float32):
    `s = f / (2 + f)` and a minimax polynomial in `s^2`.

    Uses Sun fdlibm k_log.h (float64) and FreeBSD msun e_logf.c (float32),
    also carried by historical musl logf.c: `s = f / (2 + f)` and a minimax
    polynomial in `s^2`.
    """
    var s = f / (2.0 + f)
    var z = s * s
    var w = z * z
    var r: type_of(f)
    comptime if dtype == .float64:
        r = w * polynomial_evaluate[
            [
                Scalar[dtype](3.999999999940941908e-01),
                2.222219843214978396e-01,
                1.531383769920937332e-01,
            ]
        ](w) + z * polynomial_evaluate[
            [
                Scalar[dtype](6.666666666666735130e-01),
                2.857142874366239149e-01,
                1.818357216161805012e-01,
                1.479819860511658591e-01,
            ]
        ](
            w
        )
    else:
        comptime assert dtype == .float32, "expected float32 or float64"
        r = w * polynomial_evaluate[
            [Scalar[dtype](0.40000972152), 0.24279078841]
        ](w) + z * polynomial_evaluate[
            [Scalar[dtype](0.66666662693), 0.28498786688]
        ](
            w
        )
    return s * (0.5 * f * f + r)


@inline(.always)
def _is_zero_bits[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[
    DType.bool, width
] where dtype.is_floating_point():
    """Returns whether `x` is +-0, from its bits: Apple GPUs compare float32
    subnormals as zero."""
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime magnitude = TInt((1 << (bit_width_of[dtype]() - 1)) - 1)
    return (x._to_bits_signed() & magnitude).eq(0)


@inline(.always)
def _is_subnormal_bits[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[
    DType.bool, width
] where dtype.is_floating_point():
    """Returns whether `x` is a nonzero subnormal, from its bits."""
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime mantissa_width = FPUtils[dtype].mantissa_width()
    comptime magnitude = TInt((1 << (bit_width_of[dtype]() - 1)) - 1)
    var m = x._to_bits_signed() & magnitude
    return m.ne(0) & m.lt(TInt(1 << mantissa_width))


@inline(.always)
def _frexp_bits[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> Tuple[
    type_of(x), type_of(x)
] where dtype.is_floating_point():
    """Returns `frexp(x)` for finite nonzero `x`, reading subnormal exponents
    from the bit pattern so flush-to-zero targets keep them."""
    comptime T = type_of(x)
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime mantissa_width = FPUtils[dtype].mantissa_width()
    comptime exponent_bias = FPUtils[dtype].exponent_bias()
    comptime min_normal = T(from_bits=TInt(1 << mantissa_width))

    var exponent_mask = _frexp_mask1[dtype, width]()
    var half_exponent = _frexp_mask2[dtype, width]()
    var bits = x._to_bits_signed()
    var is_subnormal = (bits & exponent_mask).eq(0)
    # A subnormal is `m * 2^(emin - mantissa_width)` for an integer significand
    # `m`. Converting `m` is exact and `m * 2^emin` is normal, so the value
    # survives targets that flush subnormal operands to zero.
    var m = (bits & TInt((1 << mantissa_width) - 1)).cast[dtype]()
    var x0 = is_subnormal.select(copysign(m * min_normal, x), x)
    var bits0 = x0._to_bits_signed()
    var e = ((bits0 & exponent_mask) >> TInt(mantissa_width)) - TInt(
        exponent_bias - 1
    )
    e -= is_subnormal.select(TInt(mantissa_width), TInt(0))
    var fraction = T(from_bits=(bits0 & ~exponent_mask) | half_exponent)
    return (fraction, e.cast[dtype]())


@inline(.always)
def _log_reduce[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> Tuple[
    type_of(x), type_of(x)
] where dtype.is_floating_point():
    """Returns `(k, f)` with `x = 2^k (1 + f)`, `1 + f` in `[sqrt(2)/2, sqrt(2))`,
    for positive finite `x` (subnormals included)."""
    var fr = _frexp_bits(x)
    var below = fr[0].lt(_SQRT2 / 2)
    return (
        below.select(fr[1] - 1.0, fr[1]),
        below.select(fr[0] + fr[0], fr[0]) - 1.0,
    )


@inline(.always)
def _log_combine[
    dtype: DType, width: SIMDLength, //
](k: SIMD[dtype, width], f: type_of(k), c: type_of(k)) -> type_of(k):
    """Returns `k*ln2 + log(1 + f) + c` for small `c`; ln2 is split so that
    `k * ln2_hi` is exact."""
    comptime is_f64 = dtype == .float64
    comptime ln2_hi = Scalar[dtype](_LN2_HI_F64 if is_f64 else 6.9313812256e-01)
    comptime ln2_lo = Scalar[dtype](_LN2_LO_F64 if is_f64 else 9.0580006145e-06)
    var hfsq = 0.5 * f * f
    return k.fma(ln2_hi, -((hfsq - (_log_kernel(f) + k.fma(ln2_lo, c))) - f))


@inline(.always)
def _log_accurate[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> type_of(x) where dtype.is_floating_point():
    """Computes the natural log of positive finite `x` to within about 1 ulp."""
    var exponent, fraction = _log_reduce(x)
    return _log_combine(exponent, fraction, 0)
