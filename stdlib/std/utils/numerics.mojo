# Upstream's `std/utils/numerics.mojo`, in part: `FPUtils`, `nan`, `isnan`, `inf`,
# `neg_inf`, `max_finite`, `min_finite`, `max_or_inf`, `min_or_neg_inf`,
# `isinf`, and `isfinite`; `inf` through `min_or_neg_inf` are the
# functions behind `SIMD`'s `MAX`, `MIN`, `MAX_FINITE`, and `MIN_FINITE`.
# The `comptime match` cases for the `float8_*` and `bfloat16` dtypes,
# `max_finite`'s `float4_e2m1fn`, `float8_*`, and `bfloat16` arms, and the
# classifiers' `float8_*`, `float4_e2m1fn`, and `bfloat16` arms and
# `dtype in (…)` tests are omitted: Mojito has none of those dtypes (R65).
# The two `bool` arms spell `Scalar[DType.bool](...)` where upstream spells
# `Scalar(...)`, since a `_` the selected initializer's `out self` binds is
# not yet solved (R1544). `get_accum_type`, `nextafter`, and
# `FlushDenormals` are not ported.
"""Defines utilities to work with numeric types.

You can import these APIs from the `utils` package. For example:

```mojo
from std.utils.numerics import FPUtils
```
"""

from std.sys import llvm_intrinsic

from std.builtin.dtype import _integral_type_of, _unsigned_integral_type_of
from std.memory import bitcast

# ===----------------------------------------------------------------------=== #
# FPUtils
# ===----------------------------------------------------------------------=== #


def _constrain_fp_type[dtype: DType]():
    comptime assert (
        dtype.is_floating_point()
    ), "dtype must be a floating point type"


struct FPUtils[
    dtype: DType, *, _constraint: NoneType = _constrain_fp_type[dtype]()
]:
    """Collection of utility functions for working with FP values.

    Constraints:
        The dtype is floating point.

    Parameters:
        dtype: The concrete FP dtype (FP32/FP64/etc).
        _constraint: Implements the constraint. Do not pass explicitly.
    """

    comptime integral_type = _integral_type_of[Self.dtype]()
    """The equivalent integer dtype of the float type."""

    comptime uint_type = _unsigned_integral_type_of[Self.dtype]()
    """The equivalent uint dtype of the float type."""

    @staticmethod
    @inline(.nodebug)
    def mantissa_width() -> Int:
        """Returns the mantissa width of a floating point type.

        Returns:
            The mantissa width.
        """
        return DType.mantissa_width[Self.dtype]()

    @staticmethod
    @inline(.nodebug)
    def max_exponent() -> Int:
        """Returns the max exponent of a floating point dtype without accounting
        for inf representations. This is not
        the maximum representable exponent, which is generally equal to
        the exponent_bias.

        Returns:
            The max exponent.
        """
        return DType.max_exponent[Self.dtype]()

    @staticmethod
    @inline(.nodebug)
    def exponent_width() -> Int:
        """Returns the exponent width of a floating point type.

        Returns:
            The exponent width.
        """
        return DType.exponent_width[Self.dtype]()

    @staticmethod
    @inline(.always)
    def exponent_bias() -> Int:
        """Returns the exponent bias of a floating point type.

        Returns:
            The exponent bias.
        """
        return DType.exponent_bias[Self.dtype]()

    @staticmethod
    @inline(.always)
    def mantissa_mask() -> Int:
        """Returns the mantissa mask of a floating point type.

        Returns:
            The mantissa mask.
        """
        return (1 << Self.mantissa_width()) - 1

    @staticmethod
    @inline(.always)
    def sign_mask() -> Int:
        """Returns the sign mask of a floating point type.

        It is computed by `1 << (exponent_width + mantissa_width)`.

        Returns:
            The sign mask.
        """
        return 1 << (Self.exponent_width() + Self.mantissa_width())

    @staticmethod
    @inline(.always)
    def exponent_mask() -> Int:
        """Returns the exponent mask of a floating point type.

        It is computed by `~(sign_mask | mantissa_mask)`.

        Returns:
            The exponent mask.
        """
        return ~(Self.sign_mask() | Self.mantissa_mask())

    @staticmethod
    @inline(.always)
    def exponent_mantissa_mask() -> Int:
        """Returns the exponent and mantissa mask of a floating point type.

        It is computed by `exponent_mask | mantissa_mask`.

        Returns:
            The exponent and mantissa mask.
        """
        return Self.exponent_mask() | Self.mantissa_mask()

    @staticmethod
    @inline(.always)
    def quiet_nan_mask() -> Int:
        """Returns the quiet NaN mask for a floating point type.

        The mask is defined by evaluating:

        ```
        (1<<exponent_width-1)<<mantissa_width + 1<<(mantissa_width-1)
        ```

        Returns:
            The quiet NaN mask.
        """
        comptime mantissa_width_val = Self.mantissa_width()
        return (1 << Self.exponent_width() - 1) << mantissa_width_val + (
            1 << (mantissa_width_val - 1)
        )

    @staticmethod
    @inline(.always)
    def bitcast_to_integer(value: Scalar[Self.dtype]) -> Int:
        """Bitcasts the floating-point value to an integer.

        Args:
            value: The floating-point type.

        Returns:
            An integer representation of the floating-point value.
        """
        return Int(bitcast[Self.integral_type, 1](value))

    @staticmethod
    @inline(.always)
    def bitcast_to_uint(value: Scalar[Self.dtype]) -> Scalar[Self.uint_type]:
        """Bitcasts the floating-point value to an integer.

        Args:
            value: The floating-point type.

        Returns:
            An integer representation of the floating-point value.
        """
        return bitcast[Self.uint_type, 1](value)

    @staticmethod
    @inline(.always)
    def bitcast_from_integer(value: Int) -> Scalar[Self.dtype]:
        """Bitcasts the floating-point value from an integer.

        Args:
            value: The int value.

        Returns:
            An floating-point representation of the Int.
        """
        return bitcast[Self.dtype, 1](Scalar[Self.integral_type](value))

    @staticmethod
    @inline(.always)
    def get_sign(value: Scalar[Self.dtype]) -> Bool:
        """Returns the sign of the floating point value.

        Args:
            value: The floating-point type.

        Returns:
            Returns True if the sign is set and False otherwise.
        """
        return (Self.bitcast_to_integer(value) & Self.sign_mask()) != 0

    @staticmethod
    @inline(.always)
    def set_sign(value: Scalar[Self.dtype], sign: Bool) -> Scalar[Self.dtype]:
        """Sets the sign of the floating point value.

        Args:
            value: The floating-point value.
            sign: True to set the sign and false otherwise.

        Returns:
            Returns the floating point value with the sign set.
        """
        var bits = Self.bitcast_to_integer(value)
        var sign_bits = Self.sign_mask()
        bits &= ~sign_bits
        if sign:
            bits |= sign_bits
        return Self.bitcast_from_integer(bits)

    @staticmethod
    @inline(.always)
    def get_exponent(value: Scalar[Self.dtype]) -> Int:
        """Returns the exponent bits of the floating-point value.

        Args:
            value: The floating-point value.

        Returns:
            Returns the exponent bits.
        """
        return (
            Self.bitcast_to_integer(value) & Self.exponent_mask()
        ) >> Self.mantissa_width()

    @staticmethod
    @inline(.always)
    def get_exponent_biased(value: Scalar[Self.dtype]) -> Int:
        """Returns the biased exponent of the floating-point value as an Int,
        this is how the value is stored before subtracting the exponent bias.

        Args:
            value: The floating-point value.

        Returns:
            The biased exponent as an Int.
        """
        return Int(
            Self.bitcast_to_uint(value)
            >> Scalar[Self.uint_type](Self.mantissa_width())
            & Scalar[Self.uint_type]((1 << Self.exponent_width()) - 1)
        )

    @staticmethod
    @inline(.always)
    def set_exponent(
        value: Scalar[Self.dtype], exponent: Int
    ) -> Scalar[Self.dtype]:
        """Sets the exponent bits of the floating-point value.

        Args:
            value: The floating-point value.
            exponent: The exponent bits.

        Returns:
            Returns the floating-point value with the exponent bits set.
        """
        var bits = Self.bitcast_to_integer(value)
        bits &= ~Self.exponent_mask()
        bits |= (exponent << Self.mantissa_width()) & Self.exponent_mask()
        return Self.bitcast_from_integer(bits)

    @staticmethod
    @inline(.always)
    def get_mantissa(value: Scalar[Self.dtype]) -> Int:
        """Gets the mantissa bits of the floating-point value.

        Args:
            value: The floating-point value.

        Returns:
            The mantissa bits.
        """
        return Self.bitcast_to_integer(value) & Self.mantissa_mask()

    @staticmethod
    @inline(.always)
    def get_mantissa_uint(value: Scalar[Self.dtype]) -> Scalar[Self.uint_type]:
        """Gets the mantissa bits of the floating-point value.

        Args:
            value: The floating-point value.

        Returns:
            The mantissa bits.
        """
        return Self.bitcast_to_uint(value) & Scalar[Self.uint_type](
            Self.mantissa_mask()
        )

    @staticmethod
    @inline(.always)
    def set_mantissa(
        value: Scalar[Self.dtype], mantissa: Int
    ) -> Scalar[Self.dtype]:
        """Sets the mantissa bits of the floating-point value.

        Args:
            value: The floating-point value.
            mantissa: The mantissa bits.

        Returns:
            Returns the floating-point value with the mantissa bits set.
        """
        var bits = Self.bitcast_to_integer(value)
        bits &= ~Self.mantissa_mask()
        bits |= mantissa & Self.mantissa_mask()
        return Self.bitcast_from_integer(bits)

    @staticmethod
    @inline(.always)
    def pack(sign: Bool, exponent: Int, mantissa: Int) -> Scalar[Self.dtype]:
        """Construct a floating-point value from its constituent sign, exponent,
        and mantissa.

        Args:
            sign: The sign of the floating-point value.
            exponent: The exponent of the floating-point value.
            mantissa: The mantissa of the floating-point value.

        Returns:
            Returns the floating-point value.
        """
        var res: Scalar[Self.dtype] = 0
        res = Self.set_sign(res, sign)
        res = Self.set_exponent(res, exponent)
        res = Self.set_mantissa(res, mantissa)
        return res


# ===----------------------------------------------------------------------=== #
# nan
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def nan[dtype: DType]() -> Scalar[dtype]:
    """Gets a NaN value for the given dtype.

    Constraints:
        Can only be used for FP dtypes.

    Parameters:
        dtype: The value dtype.

    Returns:
        The NaN value of the given dtype.
    """
    comptime assert (
        dtype.is_floating_point()
    ), "Only floating point dtypes support NaN."

    comptime match dtype:
        case .float16:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"nan"> : !kgen.scalar<f16>`,
            )
        case .float32:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"nan"> : !kgen.scalar<f32>`,
            )
        case .float64:
            return rebind[Scalar[dtype]](
                __mlir_attr.`#kgen.simd<"nan"> : !kgen.scalar<f64>`,
            )
        case _:
            comptime assert False, "unsupported float type"


# ===----------------------------------------------------------------------=== #
# isnan
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def isnan[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[.bool, width]:
    """Checks if the value is Not a Number (NaN).

    Parameters:
        dtype: The value dtype.
        width: The width of the SIMD vector.

    Args:
        val: The value to check.

    Returns:
        True if val is NaN and False otherwise.
    """

    comptime if not dtype.is_floating_point():
        return SIMD[.bool, width](fill=False)
    elif dtype == .float16:
        var bits = val.to_bits()
        return (bits & 0x7C00).eq(0x7C00) & (bits & 0x03FF).ne(0)

    comptime signaling_nan_test: UInt32 = 0x0001
    comptime quiet_nan_test: UInt32 = 0x0002
    return llvm_intrinsic[
        "llvm.is.fpclass", SIMD[.bool, width], has_side_effect=False
    ](val._mlir_value, signaling_nan_test | quiet_nan_test)


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


# ===----------------------------------------------------------------------=== #
# isinf
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def isinf[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[.bool, width]:
    """Checks if the value is infinite.

    This is always False for non-FP data types.

    Parameters:
        dtype: The value dtype.
        width: The width of the SIMD vector.

    Args:
        val: The value to check.

    Returns:
        True if val is infinite and False otherwise.
    """

    comptime if not dtype.is_floating_point():
        return SIMD[.bool, width](fill=False)

    comptime negative_infinity_test: UInt32 = 0x0004
    comptime positive_infinity_test: UInt32 = 0x0200
    return llvm_intrinsic[
        "llvm.is.fpclass", SIMD[.bool, width], has_side_effect=False
    ](val._mlir_value, negative_infinity_test | positive_infinity_test)


# ===----------------------------------------------------------------------=== #
# isfinite
# ===----------------------------------------------------------------------=== #


@inline(.nodebug)
def isfinite[
    dtype: DType, width: SIMDLength, //
](val: SIMD[dtype, width]) -> SIMD[.bool, width]:
    """Checks if the value is not infinite.

    This is always True for non-FP data types.

    Parameters:
        dtype: The value dtype.
        width: The width of the SIMD vector.

    Args:
        val: The value to check.

    Returns:
        True if val is finite and False otherwise.
    """

    comptime if not dtype.is_floating_point():
        return SIMD[.bool, width](fill=True)

    return llvm_intrinsic[
        "llvm.is.fpclass", SIMD[.bool, width], has_side_effect=False
    ](val._mlir_value, UInt32(0x1F8))
