# Upstream's `std/builtin/dtype.mojo`, in part: `struct DType` over
# `!kgen.dtype`, its mask aliases, `_int_type_of_width`,
# `_uint_type_of_width`, `_integral_type_of`, and
# `_unsigned_integral_type_of`.
#
# The members `_uint1`, `_uint2`, `_uint4`, `uint128`, `int128`, `uint256`,
# `int256`, `float4_e2m1fn`, `float6_*`, `float8_*`, and `bfloat16` are
# omitted until R65 admits those dtypes, with their `write_to` and
# `_from_str` arms; `is_float8` is `False`, `is_half_float` tests `float16`
# alone, and `is_numeric` drops the comparisons with omitted members. The
# float queries `mantissa_width`, `max_exponent`, `exponent_width`, and
# `exponent_bias` stay compiler builtins (R1937), and `__mlir_type`, whose
# `!kgen.deferred` result is outside the subset, is omitted. The `int128`,
# `int256`, `_uint1`, `_uint2`, `_uint4`, `uint128`, and `uint256` arms of
# the free functions are omitted too; a second assert rejects their widths
# explicitly.
#
# `struct DType` omits `KeyElement` from its conformances, which the bundled
# library does not declare (R237); `Equatable`, `Hashable`, and
# `ImplicitlyCopyable` answer its parts. Its keyword `__init__` spells its
# parameter `!kgen.dtype` directly where upstream writes
# `Self._mlir_type`, which a compile-time construction of the struct's own
# `comptime` members cannot resolve yet (R2007).
"""Implements the DType class.

These are Mojo built-ins, so you don't need to import them.
"""

from std.collections.optional import Optional
from std.hash.hasher import Hasher
from std.os import abort
from std.string import StringSlice
from std.sys import bit_width_of

comptime _mIsSigned = __mlir_attr.`#kgen.simd<1> : !kgen.scalar<ui8>`
comptime _mIsInteger = __mlir_attr.`#kgen.simd<128> : !kgen.scalar<ui8>`
comptime _mIsNotInteger = __mlir_attr.`#kgen.simd<127> : !kgen.scalar<ui8>`
comptime _mIsFloat = __mlir_attr.`#kgen.simd<64> : !kgen.scalar<ui8>`

struct DType(
    Equatable,
    Hashable,
    ImplicitlyCopyable,
    TrivialRegisterPassable,
    Writable,
):
    """Represents a data type specification and provides methods for working
    with it.

    `DType` defines a set of compile-time constant that specify the precise
    numeric representation of data in order to prevent runtime errors by
    catching type mismatches at compile time. It directly maps to CPU/GPU
    instruction sets, allowing the compiler to generate optimal SIMD and vector
    operations.

    `DType` behaves like an enum rather than a typical object. You don't
    instantiate it, but instead use its compile-time constants (aliases) to
    declare data types for SIMD vectors, tensors, and other data structures.

    Key usage patterns:

    - **Type specification**: Use aliases like `DType.float32` to specify types
      for SIMD vectors, tensors, and other data structures
    - **Type parameters**: Pass `DType` values as compile-time parameters to
      parameterized types like `SIMD[dtype, size]`
    - **Type introspection**: Call methods like `.is_floating_point()` to query
      type properties at compile time
    - **Type conversion**: Use in casting operations to convert between different
      numeric representations

    **Note:** Not all data types are supported on all platforms. For example,
    `DType.bfloat16` is currently not supported on Apple Silicon.

    Example:

    ```mojo
    var data = SIMD[.float16, 4](1.5, 2.5, 3.5, 4.5)
    var dtype = data.dtype

    print("Is float:", dtype.is_floating_point())  # True
    print("Is signed:", dtype.is_signed())         # True
    ```
    """

    # ===-------------------------------------------------------------------===#
    # Fields
    # ===-------------------------------------------------------------------===#

    comptime _mlir_type = __mlir_type.`!kgen.dtype`

    var _mlir_value: Self._mlir_type
    """The underlying storage for the DType value."""

    # ===-------------------------------------------------------------------===#
    # Aliases
    # ===-------------------------------------------------------------------===#

    comptime bool = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<bool> : !kgen.dtype`
    )
    """Represents a boolean data type."""

    comptime int = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<index> : !kgen.dtype`
    )
    """Represents an integral type whose bitwidth is the maximum integral value
    on the system."""

    comptime uint = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<uindex> : !kgen.dtype`
    )
    """Represents an unsigned integral type whose bitwidth is the maximum
    unsigned integral value on the system."""

    comptime uint8 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<ui8> : !kgen.dtype`
    )
    """Represents an unsigned integer type whose bitwidth is 8."""
    comptime int8 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<si8> : !kgen.dtype`
    )
    """Represents a signed integer type whose bitwidth is 8."""
    comptime uint16 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<ui16> : !kgen.dtype`
    )
    """Represents an unsigned integer type whose bitwidth is 16."""
    comptime int16 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<si16> : !kgen.dtype`
    )
    """Represents a signed integer type whose bitwidth is 16."""
    comptime uint32 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<ui32> : !kgen.dtype`
    )
    """Represents an unsigned integer type whose bitwidth is 32."""
    comptime int32 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<si32> : !kgen.dtype`
    )
    """Represents a signed integer type whose bitwidth is 32."""
    comptime uint64 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<ui64> : !kgen.dtype`
    )
    """Represents an unsigned integer type whose bitwidth is 64."""
    comptime int64 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<si64> : !kgen.dtype`
    )
    """Represents a signed integer type whose bitwidth is 64."""

    comptime float16 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<f16> : !kgen.dtype`
    )
    """Represents an IEEE754-2008 `binary16` floating point value."""

    comptime float32 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<f32> : !kgen.dtype`
    )
    """Represents an IEEE754-2008 `binary32` floating point value."""

    comptime float64 = DType(
        mlir_value=__mlir_attr.`#kgen.dtype.constant<f64> : !kgen.dtype`
    )
    """Represents an IEEE754-2008 `binary64` floating point value."""

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    @always_inline("builtin")
    def __init__(out self, *, mlir_value: __mlir_type.`!kgen.dtype`):
        """Construct a DType from MLIR dtype.

        Args:
            mlir_value: The MLIR dtype.
        """
        self._mlir_value = mlir_value

    @staticmethod
    def _from_str(str: StringSlice) -> Optional[DType]:
        """Construct a DType from a string.

        Args:
            str: The name of the DType.

        Returns:
            The parsed DType, or None if the string does not name a DType.
        """
        if str.startswith("DType."):
            return Self._from_str(str.removeprefix("DType."))

        match str:
            case "bool":
                return DType.bool
            case "int":
                return DType.int
            case "uint":
                return DType.uint
            case "uint8":
                return DType.uint8
            case "int8":
                return DType.int8
            case "uint16":
                return DType.uint16
            case "int16":
                return DType.int16
            case "uint32":
                return DType.uint32
            case "int32":
                return DType.int32
            case "uint64":
                return DType.uint64
            case "int64":
                return DType.int64
            case "float16":
                return DType.float16
            case "float32":
                return DType.float32
            case "float64":
                return DType.float64
            case _:
                return None

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        """
        Formats this dtype to the provided Writer.

        Args:
            writer: The object to write to.
        """

        # This per-`dtype` chain must stay in sync with
        # `_scalar_repr_alias` in `simd.mojo`, which maps the same set of
        # `dtype`s to their `Scalar` alias names for scalar `repr()`.
        #
        # `write_string` rather than `write`: `write` promotes each literal to a
        # `String`, so all 27 arms would carry a stack slot, a small-string
        # branch and a refcounted teardown.
        match self:
            case .bool:
                return writer.write_string("bool")
            case .int:
                return writer.write_string("int")
            case .uint:
                return writer.write_string("uint")

            case .uint8:
                return writer.write_string("uint8")
            case .int8:
                return writer.write_string("int8")
            case .uint16:
                return writer.write_string("uint16")
            case .int16:
                return writer.write_string("int16")
            case .uint32:
                return writer.write_string("uint32")
            case .int32:
                return writer.write_string("int32")
            case .uint64:
                return writer.write_string("uint64")
            case .int64:
                return writer.write_string("int64")

            case .float16:
                return writer.write_string("float16")

            case .float32:
                return writer.write_string("float32")

            case .float64:
                return writer.write_string("float64")

        return writer.write_string("<<unknown>>")

    @inline(.nodebug)
    def write_repr_to(self, mut writer: Some[Writer]):
        """Write the string representation of the DType.

        Args:
            writer: The value to write to.
        """
        writer.write_string("DType.")
        self.write_to(writer)

    @inline(.nodebug)
    def get_value(self) -> __mlir_type.`!kgen.dtype`:
        """Gets the associated internal kgen.dtype value.

        Returns:
            The kgen.dtype value.
        """
        return self._mlir_value

    @doc_hidden
    @staticmethod
    @always_inline("builtin")
    def _from_ui8(ui8: UInt8._mlir_type) -> DType:
        return DType(
            mlir_value=__mlir_op.`pop.dtype.from_ui8`(
                __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.ui8](ui8)
            )
        )

    @doc_hidden
    @always_inline("builtin")
    # Raw MLIR type avoids a circular dependency: UInt8._mlir_type would pull
    # in UInt8 = UInt8 → Scalar → SIMD signature → DevicePassable
    # where-clause → _as_ui8 → UInt8 (cycle) during constraint evaluation.
    def _as_ui8(self) -> __mlir_type.`!kgen.scalar<ui8>`:
        return __mlir_op.`pop.cast_from_builtin`[
            _type=__mlir_type.`!kgen.scalar<ui8>`
        ](__mlir_op.`pop.dtype.to_ui8`(self._mlir_value))

    @doc_hidden
    @always_inline("builtin")
    def _match(self, mask: UInt8) -> Bool:
        return self._match(mask._mlir_value)

    @doc_hidden
    @always_inline("builtin")
    def _match(self, mask: __mlir_type.`!kgen.scalar<ui8>`) -> Bool:
        return Bool(
            mlir_value=__mlir_op.`pop.cmp`[
                pred=__mlir_attr.`#kgen.cmp_pred<ne>`
            ](
                __mlir_op.`pop.simd.and`(self._as_ui8(), mask),
                __mlir_attr.`#kgen.simd<0> : !kgen.scalar<ui8>`,
            )
        )

    @always_inline("builtin")
    def __eq__(self, rhs: DType) -> Bool:
        """Compares one DType to another for equality.

        Args:
            rhs: The DType to compare against.

        Returns:
            True if the DTypes are the same and False otherwise.
        """
        return Bool(
            mlir_value=__mlir_op.`pop.cmp`[
                pred=__mlir_attr.`#kgen.cmp_pred<eq>`
            ](self._as_ui8(), rhs._as_ui8())
        )

    @always_inline("builtin")
    def __ne__(self, rhs: DType) -> Bool:
        """Compares one DType to another for inequality.

        Args:
            rhs: The DType to compare against.

        Returns:
            False if the DTypes are the same and True otherwise.
        """
        return Bool(
            mlir_value=__mlir_op.`pop.cmp`[
                pred=__mlir_attr.`#kgen.cmp_pred<ne>`
            ](self._as_ui8(), rhs._as_ui8())
        )

    def __hash__[H: Hasher](self, mut hasher: H):
        """Updates hasher with this `DType` value.

        Parameters:
            H: The hasher type.

        Args:
            hasher: The hasher instance.
        """
        hasher._update_with_simd(UInt8(mlir_value=self._as_ui8()))

    @always_inline("builtin")
    def is_unsigned(self) -> Bool:
        """Returns True if the type parameter is unsigned and False otherwise.

        Returns:
            Returns True if the input type parameter is unsigned.
        """
        return (
            self == DType.uint
            or self._is_non_index_integral()
            and not self._match(_mIsSigned)
        )

    @always_inline("builtin")
    def is_signed(self) -> Bool:
        """Returns True if the type parameter is signed and False otherwise.

        Returns:
            Returns True if the input type parameter is signed.
        """
        return self.is_floating_point() or (
            self.is_integral() and self._match(_mIsSigned)
        )

    @always_inline("builtin")
    def _is_non_index_integral(self) -> Bool:
        """Returns True if the type parameter is a non-index integer value and False otherwise.

        Returns:
            Returns True if the input type parameter is a non-index integer.
        """
        return self._match(_mIsInteger)

    @always_inline("builtin")
    def is_integral(self) -> Bool:
        """Returns True if the type parameter is an integer and False otherwise.

        Returns:
            Returns True if the input type parameter is an integer.
        """
        return (
            self == DType.int
            or self == DType.uint
            or self._is_non_index_integral()
        )

    @always_inline("builtin")
    def is_floating_point(self) -> Bool:
        """Returns True if the type parameter is a floating-point and False
        otherwise.

        Returns:
            Returns True if the input type parameter is a floating-point.
        """
        return self._match(_mIsFloat)

    @always_inline("builtin")
    def is_float8(self) -> Bool:
        """Returns True if the dtype is a 8bit-precision floating point type,
        e.g. float8_e5m2, float8_e5m2fnuz, float8_e4m3fn and float8_e4m3fnuz.

        Returns:
            True if the dtype is a 8bit-precision float, false otherwise.
        """

        return False

    @always_inline("builtin")
    def is_half_float(self) -> Bool:
        """Returns True if the dtype is a half-precision floating point type,
        e.g. either fp16 or bf16.

        Returns:
            True if the dtype is a half-precision float, false otherwise..
        """

        return self == DType.float16

    @always_inline("builtin")
    def is_numeric(self) -> Bool:
        """Returns True if the type parameter is numeric (i.e. you can perform
        arithmetic operations on).

        Returns:
            Returns True if the input type parameter is either integral or
              floating-point.
        """
        return self.is_integral() or self.is_floating_point()


# ===-------------------------------------------------------------------===#
# _int_type_of_width
# ===-------------------------------------------------------------------===#


@inline(.always)
def _int_type_of_width[width: Int]() -> DType:
    comptime assert width in (
        8,
        16,
        32,
        64,
        128,
        256,
    ), "width must be either 8, 16, 32, 64, 128, or 256"
    comptime assert width in (
        8,
        16,
        32,
        64,
    ), "an int DType of width 128 or 256 is not supported yet"

    comptime match width:
        case 8:
            return DType.int8
        case 16:
            return DType.int16
        case 32:
            return DType.int32
        case _:
            return DType.int64


# ===-------------------------------------------------------------------===#
# _uint_type_of_width
# ===-------------------------------------------------------------------===#


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
