# Upstream's `std/simd.mojo`, in part: the `SIMD` struct, its storage, the
# `_simd_dtype_checks` and `_simd_construction_checks` helpers, the
# initializers `__init__(out self)`, the cast from `SIMD[other_dtype,
# Self.length]`, the `SIMDLength` splat, `from_int=`, `Bool`, `fill=`, `Int`
# from an `index`, and the `Scalar[Self.dtype]` splat, and its instance
# `__len__`, `__getitem__`, `__setitem__`, `__contains__`, `__add__`,
# `__sub__`, `__mul__`, `__floordiv__`, `__pos__`, `__neg__`, `__and__`,
# `__xor__`, `__or__`, `__lshift__`, `__rshift__`, `__invert__`, the
# comparisons `__eq__`, `__ne__`, `__gt__`, `__ge__`, `__lt__`, and
# `__le__`, the elementwise `eq`, `ne`, `gt`, `ge`, `lt`, and `le` with
# their `_Mask` alias, the in-place `__iadd__`, `__isub__`, `__imul__`,
# `__ifloordiv__`, `__iand__`, `__ixor__`, `__ior__`, `__ilshift__`, and
# `__irshift__`, the reflected `__radd__`, `__rsub__`, `__rmul__`,
# `__rfloordiv__`, `__rand__`, `__rxor__`, `__ror__`, `__rlshift__`, and
# `__rrshift__`, `__bool__`, `__int__`,
# `__mlir_index__`, `__float__`, `__floor__`, `__ceil__`, `__trunc__`,
# `__abs__`, `__hash__`, `_refine`, `cast`, `is_power_of_two`, `to_bits`,
# `_to_bits_signed`, `fma`, and `select`, with the `FastMathFlag` enum `fma`
# takes. The other
# members are still served by the compiler (R184, R1623, R1567, R1581,
# R1582, R1585, R1586, R1596, R330, R276), and so are the other initializers:
# `mlir_value=` (R1591), the element list (R1592), `from_bits=` (R1593),
# `IntLiteral` and `FloatLiteral` (R1507), and the `Intable`, `IntableRaising`, `Floatable`,
# and `FloatableRaising` conversions (R1399); `py=` is not ported. Until
# then a construction takes a declared overload only where its arguments
# select one at the pin. `_simd_dtype_checks` is `True`: Mojito has no
# sub-byte float dtype (R65), and `__hash__` omits its `float8_e8m0fnu`
# conjunct for the same reason. The struct declares
# only the traits whose requirements are ported here; as upstream's, its
# trait list has no `Floatable`. `Scalar`, `Int`, `UInt`, the sized aliases,
# and `Byte` stay compiler names until R1557.
# `cast` omits upstream's `is_nvidia_gpu()` and `bfloat16`-on-AMD branches
# (R1635), and its float8, `float8_e8m0fnu`, and `_uint1`/`_uint2`/`_uint4`
# branches (R65): Mojito has none of those targets or dtypes.
# Omitted: the `@lldb_formatter_wrapping_type` decorator
# (`std.utils._visualizers`) and `CoordLike`'s static `__len__`
# (`std.utils.coord`).
"""Implements SIMD primitives and abstractions.

Provides high-performance SIMD primitives and abstractions for
vectorized computation in Mojo. It enables efficient data-parallel operations
by leveraging hardware vector processing units across different architectures.

Key Features:
1. Architecture-agnostic SIMD abstractions with automatic hardware detection
2. Optimized vector operations for common numerical computations
3. Explicit control over vectorization strategies and memory layouts
4. Zero-cost abstractions that compile to efficient machine code
5. Support for different vector widths and element types

Primary Components:
- Vector types: Strongly-typed vector containers with element-wise operations
- SIMD intrinsics: Low-level access to hardware SIMD instructions
- Vectorized algorithms: Common algorithms optimized for SIMD execution
- Memory utilities: Aligned memory allocation and vector load/store operations

Performance Considerations:
- Vector width selection should match target hardware capabilities
- Memory alignment affects load/store performance
- Data layout transformations may be necessary for optimal vectorization

Integration:
This module is designed to work seamlessly with other Mojo numerical computing
components, including tensor operations, linear algebra routines, and
domain-specific libraries for machine learning and scientific computing.
"""

# `max`/`min` are upstream's prelude names, which `std.simd` cannot see
# while the prelude bootstraps (R894).
from std.math import Ceilable, Floorable, Truncable, max, min
from std.sys.info import bit_width_of
from std.builtin.simd_length import SIMDLength
from std.builtin.enum_like import EnumLike
from std.memory import bitcast
from std.builtin.dtype import (
    _integral_type_of,
    _uint_type_of_width,
    _unsigned_integral_type_of,
)
from std.builtin.int import _FromInt
from std.hash.hasher import Hasher


# ===----------------------------------------------------------------------=== #
# Utilities
# ===----------------------------------------------------------------------=== #


@always_inline("builtin")
def _simd_dtype_checks[dtype: DType]() -> Bool:
    """Checks that the SIMD element dtype can be held in a value.

    The sub-byte float formats are storage-only: no target registers a
    `pop.cast` for them, so such a value can neither be produced from nor read
    back into another float type.

    Parameters:
      dtype: The data type of SIMD vector elements.

    Returns:
        Whether a SIMD value may be constructed with this element dtype.
    """
    return True


@always_inline("builtin")
def _simd_construction_checks[dtype: DType, size: SIMDLength]():
    """Checks if the SIMD dtype and size are valid.

    The SIMD size is valid if it is a power of two and is positive.

    Parameters:
      dtype: The data type of SIMD vector elements.
      size: The number of elements in the SIMD vector. The size must not be greater than 2**15.
    """
    comptime assert _simd_dtype_checks[dtype](), (
        "cannot construct a SIMD value with a sub-byte float dtype; these are"
        " storage-only formats with no conversion support on any target."
    )
    comptime assert size.is_power_of_two(), "simd width must be power of 2"
    # MOCO-1388: Until LLVM's issue #122571 is fixed, LLVM's SelectionDAG has
    # a limit of 2^15 for the number of operands of the instruction.
    # NOTE: Even after the limit increases in LLVM, compile time might be 3x
    # slower than with GCC, therefore until we have a real use case for large
    # SIMD, we better to keep limit at 2^15.
    # NOTE: Might need to revisit the limit for targets that use GlobalISel
    # as it does have smaller limit now.
    # comptime assert (
    #     size <= 2**15
    # ), "simd size is too large and must be less than 2^15"


# ===----------------------------------------------------------------------=== #
# FastMathFlag
# ===----------------------------------------------------------------------=== #


@fieldwise_init
struct FastMathFlag(EnumLike, Equatable, ImplicitlyCopyable, RegisterPassable):
    """Flags for controlling fast-math optimizations in floating-point operations.

    FastMathFlag provides compile-time controls for various floating-point math
    optimization modes that trade strict IEEE 754 compliance for performance.

    Available flags:
    - `NONE`: No fast-math optimizations.
    - `NNAN`: Assume operands and results are not NaN.
    - `NINF`: Assume operands and results are not +/- infinity.
    - `NSZ`: Treat the sign of a zero as insignificant.
    - `ARCP`: Allow reciprocal of values.
    - `CONTRACT`: Allow floating-point contraction (e.g., fused multiply-add).
    - `AFN`: Allow algebraic function approximations.
    - `REASSOC`: Allow reassociation of floating-point operations.
    - `FAST`: Enable all fast-math optimizations.

    Examples:
        ```mojo
        from std.simd import FastMathFlag
        var value = Float32(2.0)
        var multiplier = Float32(3.0)
        var accumulator = Float32(1.0)

        # Use contract flag for fused multiply-add
        var result = value.fma[FastMathFlag.CONTRACT](multiplier, accumulator)

        # Use fast flag for maximum optimization
        var fast_result = value.fma[FastMathFlag.FAST](multiplier, accumulator)
        ```
    """

    var _value: UInt8

    comptime NONE = Self(0)
    """No fast-math optimizations enabled."""

    comptime NNAN = Self(1)
    """Assume no NaN values."""

    comptime NINF = Self(2)
    """Assume no infinite values."""

    comptime NSZ = Self(3)
    """Treat the sign of zero as insignificant."""

    comptime ARCP = Self(4)
    """Allow reciprocal approximations."""

    comptime CONTRACT = Self(5)
    """Allow floating-point contraction."""

    comptime AFN = Self(6)
    """Allow approximate function implementations."""

    comptime REASSOC = Self(7)
    """Allow reassociation of operations."""

    comptime FAST = Self(8)
    """Enable all fast-math optimizations."""

    comptime _enum_case_names = ParameterList.of[
        "NONE".value,
        "NNAN".value,
        "NINF".value,
        "NSZ".value,
        "ARCP".value,
        "CONTRACT".value,
        "AFN".value,
        "REASSOC".value,
        "FAST".value,
    ].values

    # Upstream spells this `TypeList.splat[ParameterList[
    # Self._enum_case_names].size, NoneType].values` (R1181).
    comptime _enum_case_types = TypeList.of[
        Trait=AnyType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
        NoneType,
    ].values

    @inline(.always)
    def _get_enum_discriminant(self) -> Int:
        return Int(self._value)

    @inline(.always)
    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        comptime assert False, "FastMathFlag has no payload"

    def __eq__(self, other: Self) -> Bool:
        """Compares two FastMathFlag values for identity.

        Args:
            other: The FastMathFlag to compare against.

        Returns:
            True if both flags have the same value, False otherwise.
        """
        return self._value == other._value

    def _mlir_attr(self) -> __mlir_type.`!kgen.deferred`:
        if self == FastMathFlag.NONE:
            return __mlir_attr.`#pop.fmf<none>`
        if self == FastMathFlag.NNAN:
            return __mlir_attr.`#pop.fmf<nnan>`
        if self == FastMathFlag.NINF:
            return __mlir_attr.`#pop.fmf<ninf>`
        if self == FastMathFlag.NSZ:
            return __mlir_attr.`#pop.fmf<nsz>`
        if self == FastMathFlag.ARCP:
            return __mlir_attr.`#pop.fmf<arcp>`
        if self == FastMathFlag.CONTRACT:
            return __mlir_attr.`#pop.fmf<contract>`
        if self == FastMathFlag.AFN:
            return __mlir_attr.`#pop.fmf<afn>`
        if self == FastMathFlag.REASSOC:
            return __mlir_attr.`#pop.fmf<reassoc>`
        if self == FastMathFlag.FAST:
            return __mlir_attr.`#pop.fmf<fast>`

        return __mlir_attr.`#pop.fmf<none>`


# ===----------------------------------------------------------------------=== #
# SIMD
# ===----------------------------------------------------------------------=== #


@stable(since="1.1")
struct SIMD[dtype: DType, length: SIMDLength](
    Absable,
    Boolable,
    Ceilable,
    Comparable,
    Defaultable,
    Equatable,
    Floorable,
    Hashable,
    Indexer,
    Intable,
    Sized,
    TrivialRegisterPassable,
    Truncable,
    _FromInt,
):
    """Represents a vector type that leverages hardware acceleration to process
    multiple data elements with a single operation.

    SIMD (Single Instruction, Multiple Data) is a fundamental parallel
    computing paradigm where a single CPU instruction operates on multiple data
    elements at once. Modern CPUs can perform 4, 8, 16, or even 32 operations
    in parallel using SIMD, delivering substantial performance improvements
    over scalar operations. Instead of processing one value at a time, SIMD
    processes entire vectors of values with each instruction.

    For example, when adding two vectors of four values, a scalar operation
    adds each value in the vector one by one, while a SIMD operation adds all
    four values at once using vector registers:

    ```text
    Scalar operation:                SIMD operation:
    ┌─────────────────────────┐      ┌───────────────────────────┐
    │ 4 instructions          │      │ 1 instruction             │
    │ 4 clock cycles          │      │ 1 clock cycle             │
    │                         │      │                           │
    │ ADD  a[0], b[0] → c[0]  │      │ Vector register A         │
    │ ADD  a[1], b[1] → c[1]  │      │ ┌─────┬─────┬─────┬─────┐ │
    │ ADD  a[2], b[2] → c[2]  │      │ │a[0] │a[1] │a[2] │a[3] │ │
    │ ADD  a[3], b[3] → c[3]  │      │ └─────┴─────┴─────┴─────┘ │
    └─────────────────────────┘      │           +               │
                                     │ Vector register B         │
                                     │ ┌─────┬─────┬─────┬─────┐ │
                                     │ │b[0] │b[1] │b[2] │b[3] │ │
                                     │ └─────┴─────┴─────┴─────┘ │
                                     │           ↓               │
                                     │        SIMD_ADD           │
                                     │           ↓               │
                                     │ Vector register C         │
                                     │ ┌─────┬─────┬─────┬─────┐ │
                                     │ │c[0] │c[1] │c[2] │c[3] │ │
                                     │ └─────┴─────┴─────┴─────┘ │
                                     └───────────────────────────┘
    ```

    The `SIMD` type maps directly to hardware vector registers and
    instructions. Mojo automatically generates optimal SIMD code that leverages
    CPU-specific instruction sets (such as AVX and NEON) without requiring
    manual intrinsics or assembly programming.

    This type is the foundation of high-performance CPU computing in Mojo,
    enabling you to write code that automatically leverages modern CPU vector
    capabilities while maintaining code clarity and portability.

    **Caution:** If you declare a SIMD vector size larger than the vector
    registers of the target hardware, the compiler will break up the SIMD into
    multiple vector registers for compatibility. However, you should avoid
    using a vector that's more than 2x the hardware's vector register size
    because the resulting code will perform poorly.

    Key properties:

    - **Hardware-mapped**: Directly maps to CPU vector registers
    - **Type-safe**: Data types and vector sizes are checked at compile time
    - **Zero-cost**: No runtime overhead compared to hand-optimized intrinsics
    - **Portable**: Same code works across different CPU architectures
      (x86, ARM, etc.)
    - **Composable**: Seamlessly integrates with Mojo's parallelization features

    Key APIs:

    - Construction:
      - Broadcast single value to all elements: `SIMD[dtype, length](value)`
      - Initialize with specific values: `SIMD[dtype, length](v1, v2, ...)`
      - Zero-initialized vector: `SIMD[dtype, length]()`

    - Element operations:
      - Arithmetic: `+`, `-`, `*`, `/`, `%`, `//`
      - Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`
      - Math functions: `sqrt()`, `sin()`, `cos()`, `fma()`, etc.
      - Bit operations: `&`, `|`, `^`, `~`, `<<`, `>>`

    - Vector operations:
      - Horizontal reductions: `reduce_add()`, `reduce_mul()`, `reduce_min()`, `reduce_max()`
      - Element-wise conditional selection: `select(true_case, false_case)`
      - Vector manipulation: `shuffle()`, `slice()`, `join()`, `split()`
      - Type conversion: `cast[target_dtype]()`

    Examples:

    Vectorized math operations:

    ```mojo
    # Process 8 floating-point numbers simultaneously
    var a = SIMD[.float32, 8](1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0)
    var b = SIMD[.float32, 8](2.0)  # Broadcast 2.0 to all elements
    var result = a * b + 1.0
    print(result)  # => [3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0, 17.0]
    ```

    Conditional operations with masking:

    ```mojo
    # Double the positive values and negate the negative values
    var values = SIMD[.int32, 4](1, -2, 3, -4)
    var is_positive = values.gt(0)  # greater-than: gets SIMD of booleans
    var result = is_positive.select(values * 2, values * -1)
    print(result)  # => [2, 2, 6, 4]
    ```

    Horizontal reductions:

    ```mojo
    # Sum all elements in a vector
    var data = SIMD[.float64, 4](10.5, 20.3, 30.1, 40.7)
    var total = data.reduce_add()
    var maximum = data.reduce_max()
    print(total, maximum)  # => 101.6 40.7
    ```

    Constraints:
        The length of the SIMD vector must be positive and a power of 2.

    Parameters:
        dtype: The data type of SIMD vector elements.
        length: The length of the SIMD vector (number of elements).
    """

    # ===-------------------------------------------------------------------===#
    # Fields
    # ===-------------------------------------------------------------------===#

    comptime _mlir_type = __mlir_type[
        `!kgen.simd<`,
        Self.length._mlir_value,
        `, `,
        Self.dtype._mlir_value,
        `>`,
    ]

    var _mlir_value: Self._mlir_type
    """The underlying storage for the vector."""

    # ===-------------------------------------------------------------------===#
    # Aliases
    # ===-------------------------------------------------------------------===#

    comptime _Mask = SIMD[.bool, Self.length]

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    @stable(since="1.1")
    @always_inline("builtin")
    def __init__(out self):
        """Default initializer of the SIMD vector.

        By default the SIMD vectors are initialized to all zeros.
        """
        # make sure this constructor is called at compile time
        comptime res = SIMD[Self.dtype, Self.length](0)
        self = res

    @inline(.nodebug)
    def __init__[
        other_dtype: DType, //
    ](out self, value: SIMD[other_dtype, Self.length], /):
        """Initialize from another SIMD of the same size. If the value
        passed is a scalar, you can initialize a SIMD vector with more elements.

        Parameters:
            other_dtype: The type of the value that is being cast from.

        Args:
            value: The value to cast from.

        Example:

        ```mojo
        print(UInt64(UInt8(42))) # 42
        print(SIMD[.uint64, 4](UInt8(42))) # [42, 42, 42, 42]
        ```

        Casting behavior:

        ```mojo
        # Basic casting preserves value within range
        Int8(UInt8(127)) == Int8(127)

        # Numbers above signed max wrap to negative using two's complement
        Int8(UInt8(128)) == Int8(-128)
        Int8(UInt8(129)) == Int8(-127)
        Int8(UInt8(256)) == Int8(0)

        # Negative signed cast to unsigned using two's complement
        UInt8(Int8(-128)) == UInt8(128)
        UInt8(Int8(-127)) == UInt8(129)
        UInt8(Int8(-1)) == UInt8(255)

        # Truncate precision after downcast and upcast
        Float64(Float32(Float64(123456789.123456789))) == Float64(123456792.0)

        # Rightmost bits of significand become 0's on upcast
        Float64(Float32(0.3)) == Float64(0.30000001192092896)

        # Numbers equal after truncation of float literal and cast truncation
        Float32(Float64(123456789.123456789)) == Float32(123456789.123456789)

        # Float to int/uint floors
        Int64(Float64(42.2)) == Int64(42)
        ```
        """
        self = value.cast[Self.dtype]()

    @always_inline("builtin")
    @implicit
    def __init__(out self, value: SIMDLength, /):
        """Initializes the SIMD vector with a signed integer.

        The signed integer value is splatted across all the elements of the SIMD
        vector.

        Args:
            value: The input value.
        """
        _simd_construction_checks[Self.dtype, Self.length]()

        self._mlir_value = __mlir_op.`pop.simd.splat`[_type=Self._mlir_type](
            __mlir_op.`pop.cast`[_type=Scalar[Self.dtype]._mlir_type](
                __mlir_op.`pop.cast_from_builtin`[
                    _type=__mlir_type.`!kgen.scalar<index>`
                ](value._mlir_value)
            )
        )

    @doc_hidden
    @always_inline("builtin")
    def __init__(out self, *, from_int: Int):
        _simd_construction_checks[Self.dtype, Self.length]()
        self = Self(from_int)

    @inline(.nodebug)
    @implicit
    def __init__(out self: SIMD[.bool, Self.length], value: Bool, /):
        """Initializes a Scalar with a bool value.

        Since this constructor does not splat, it can be implicit.

        Args:
            value: The bool value to initialize the Scalar with.
        """

        # NOTE: due to some issues with the out Self parameter not always being
        # respected (i.e. through implicit conversion paths), it's better to do
        # this check instead of constraining the signature, because otherwise
        # the error would point to a type mismatch. All this should be fixed by
        # using a requires clause when it becomes available.
        comptime assert Self.length == 1, (
            "must be a scalar; use the `fill` keyword instead for explicit"
            " splatting"
        )

        _simd_construction_checks[Self.dtype, Self.length]()
        self._mlir_value = rebind[Self._Mask._mlir_type](value._mlir_value)

    @inline(.nodebug)
    def __init__(out self: SIMD[.bool, Self.length], *, fill: Bool):
        """Initializes the SIMD vector with a bool value.

        The bool value is splatted across all elements of the SIMD vector.

        Args:
            fill: The bool value to fill each element of the SIMD vector with.
        """
        _simd_construction_checks[Self.dtype, Self.length]()
        self._mlir_value = __mlir_op.`pop.simd.splat`[
            _type=Self._Mask._mlir_type
        ](fill._mlir_value)

    @doc_hidden
    @always_inline("builtin")
    def __init__(out self: Int, *, mlir_value: __mlir_type.index):
        self._mlir_value = __mlir_op.`pop.cast_from_builtin`[
            _type=__mlir_type.`!kgen.scalar<index>`
        ](mlir_value)

    @always_inline("builtin")
    @implicit
    def __init__(out self, value: Scalar[Self.dtype], /):
        """Constructs a SIMD vector by splatting a scalar value.

        The input value is splatted across all elements of the SIMD vector.

        Args:
            value: The value to splat to the elements of the vector.
        """
        _simd_construction_checks[Self.dtype, Self.length]()
        self._mlir_value = __mlir_op.`pop.simd.splat`[_type=Self._mlir_type](
            value._mlir_value
        )

    # ===-------------------------------------------------------------------===#
    # Operator dunders
    # ===-------------------------------------------------------------------===#

    @inline(.nodebug)
    def __getitem__(self, idx: Int) -> Scalar[Self.dtype]:
        """Gets an element from the vector.

        Args:
            idx: The element index.

        Returns:
            The value at position `idx`.
        """
        return Scalar[Self.dtype](
            mlir_value=__mlir_op.`pop.simd.extractelement`(
                self._mlir_value, idx.__mlir_index__()
            )
        )

    @inline(.nodebug)
    def __setitem__(mut self, idx: Int, val: Scalar[Self.dtype]):
        """Sets an element in the vector.

        Args:
            idx: The index to set.
            val: The value to set.
        """
        self._mlir_value = __mlir_op.`pop.simd.insertelement`(
            self._mlir_value, val._mlir_value, idx.__mlir_index__()
        )

    def __contains__(self, value: Scalar[Self.dtype]) -> Bool:
        """Whether the vector contains the value.

        Args:
            value: The value.

        Returns:
            Whether the vector contains the value.
        """
        return self.eq(value).reduce_or()

    @always_inline("builtin")
    def __add__(self, rhs: Self) -> Self:
        """Computes `self + rhs`.

        Args:
            rhs: The rhs value.

        Returns:
            A new vector whose element at position `i` is computed as
            `self[i] + rhs[i]`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return Self(
            mlir_value=__mlir_op.`pop.add`(self._mlir_value, rhs._mlir_value)
        )

    @always_inline("builtin")
    def __sub__(self, rhs: Self) -> Self:
        """Computes `self - rhs`.

        Args:
            rhs: The rhs value.

        Returns:
            A new vector whose element at position `i` is computed as
            `self[i] - rhs[i]`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return Self(
            mlir_value=__mlir_op.`pop.sub`(self._mlir_value, rhs._mlir_value)
        )

    @always_inline("builtin")
    def __mul__(self, rhs: Self) -> Self:
        """Computes `self * rhs`.

        Args:
            rhs: The rhs value.

        Returns:
            A new vector whose element at position `i` is computed as
            `self[i] * rhs[i]`.
        """

        return Self(
            mlir_value=__mlir_op.`pop.mul`(self._mlir_value, rhs._mlir_value)
        )

    @inline(.nodebug)
    def __floordiv__(self, rhs: Self) -> Self:
        """Returns the division of self and rhs rounded down to the nearest
        integer.

        Constraints:
            The element type of the SIMD vector must be numeric.

        Args:
            rhs: The value to divide with.

        Returns:
            `floor(self / rhs)` value.
        """
        comptime assert Self.dtype.is_numeric(), "the type must be numeric"

        var is_zero_mask = Self._Mask(fill=Self.dtype.is_integral()) and rhs.eq(
            0
        )
        var safe_divisor = is_zero_mask.select(Self(1), rhs)

        var floordiv = Self(
            mlir_value=__mlir_op.`pop.floordiv`(
                self._mlir_value, safe_divisor._mlir_value
            )
        )
        return is_zero_mask.select(Self(), floordiv)

    @always_inline("builtin")
    def __pos__(self) -> Self:
        """Defines the unary `+` operation.

        Returns:
            This SIMD vector.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return self

    @always_inline("builtin")
    def __neg__(self) -> Self:
        """Defines the unary `-` operation.

        Returns:
            The negation of this SIMD vector.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return Self(mlir_value=__mlir_op.`pop.neg`(self._mlir_value))

    @always_inline("builtin")
    def __and__(self, rhs: Self) -> Self:
        """Returns `self & rhs`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.

        Returns:
            `self & rhs`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return Self(
            mlir_value=__mlir_op.`pop.simd.and`(
                self._mlir_value, rhs._mlir_value
            )
        )

    @always_inline("builtin")
    def __xor__(self, rhs: Self) -> Self:
        """Returns `self ^ rhs`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.

        Returns:
            `self ^ rhs`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return Self(
            mlir_value=__mlir_op.`pop.simd.xor`(
                self._mlir_value, rhs._mlir_value
            )
        )

    @always_inline("builtin")
    def __or__(self, rhs: Self) -> Self:
        """Returns `self | rhs`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.

        Returns:
            `self | rhs`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return Self(
            mlir_value=__mlir_op.`pop.simd.or`(
                self._mlir_value, rhs._mlir_value
            )
        )

    @always_inline("builtin")
    def __lshift__(self, rhs: Self) -> Self:
        """Returns `self << rhs`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            rhs: The RHS value.

        Returns:
            `self << rhs`.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        return Self(
            mlir_value=__mlir_op.`pop.shl`(self._mlir_value, rhs._mlir_value)
        )

    @always_inline("builtin")
    def __rshift__(self, rhs: Self) -> Self:
        """Returns `self >> rhs`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            rhs: The RHS value.

        Returns:
            `self >> rhs`.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        return Self(
            mlir_value=__mlir_op.`pop.shr`(self._mlir_value, rhs._mlir_value)
        )

    @always_inline("builtin")
    def __invert__(self) -> Self:
        """Returns `~self`.

        Constraints:
            The element type of the SIMD vector must be boolean or integral.

        Returns:
            The `~self` value.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"

        return self ^ -1

    # ===------------------------------------------------------------------=== #
    # Boolean comparison operations.
    # ===------------------------------------------------------------------=== #

    @always_inline("builtin")
    @stable(since="1.1")
    def __eq__(self, rhs: Self) -> Bool:
        """Compares two SIMD vectors for equality.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            True if all elements of the SIMD vectors are equal, False otherwise.
        """
        return Bool(
            mlir_value=__mlir_op.`pop.simd.reduce_and`(self.eq(rhs)._mlir_value)
        )

    @stable(since="1.1")
    @always_inline("builtin")
    def __ne__(self, rhs: Self) -> Bool:
        """Compares two SIMD vectors for inequality.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            True if any elements of the SIMD vectors are not equal, False
            otherwise.
        """
        return not self == rhs

    @always_inline("builtin")
    def __gt__(self, rhs: Self) -> Bool:
        """Compares two Scalars using greater-than comparison.

        Args:
            rhs: The Scalar to compare with.

        Returns:
            True if `self` is greater than `rhs`, False otherwise.
        """
        comptime assert Self.length == 1, (
            "Strict inequality is only defined for `Scalar`s; "
            "did you mean to use `SIMD.gt(...)`?"
        )
        return self.gt(rhs).__bool__()

    @always_inline("builtin")
    def __ge__(self, rhs: Self) -> Bool:
        """Compares two Scalars using greater-than-or-equal comparison.

        Args:
            rhs: The Scalar to compare with.

        Returns:
            True if `self` is greater than or equal to `rhs`, False otherwise.
        """
        comptime assert Self.length == 1, (
            "Greater than or equal is only defined for `Scalar`s; "
            "did you mean to use `SIMD.ge(...)`?"
        )
        return self.ge(rhs).__bool__()

    @always_inline("builtin")
    def __lt__(self, rhs: Self) -> Bool:
        """Compares two Scalars using less-than comparison.

        Args:
            rhs: The Scalar to compare with.

        Returns:
            True if `self` is less than `rhs`, False otherwise.
        """
        comptime assert Self.length == 1, (
            "Strict inequality is only defined for `Scalar`s; "
            "did you mean to use `SIMD.lt(...)`?"
        )
        return self.lt(rhs).__bool__()

    @always_inline("builtin")
    def __le__(self, rhs: Self) -> Bool:
        """Compares two Scalars using less-than-or-equal comparison.

        Args:
            rhs: The Scalar to compare with.

        Returns:
            True if `self` is less than or equal to `rhs`, False otherwise.
        """
        comptime assert Self.length == 1, (
            "Less than or equal is only defined for `Scalar`s; "
            "did you mean to use `SIMD.le(...)`?"
        )
        return self.le(rhs).__bool__()

    # ===------------------------------------------------------------------=== #
    # Elementwise comparison operations.
    # ===------------------------------------------------------------------=== #

    @always_inline("builtin")
    def eq(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise equality.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] == rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<eq>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    @always_inline("builtin")
    def ne(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise inequality.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] != rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<ne>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    @always_inline("builtin")
    def gt(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise greater-than comparison.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] > rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<gt>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    @always_inline("builtin")
    def ge(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise greater-than-or-equal
        comparison.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] >= rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<ge>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    @always_inline("builtin")
    def lt(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise less-than comparison.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] < rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<lt>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    @always_inline("builtin")
    def le(self, rhs: Self) -> Self._Mask:
        """Compares two SIMD vectors using elementwise less-than-or-equal
        comparison.

        Args:
            rhs: The SIMD vector to compare with.

        Returns:
            A new bool SIMD vector of the same size whose element at position
            `i` is the value of `self[i] <= rhs[i]`.
        """

        var res = __mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<le>`](
            self._mlir_value, rhs._mlir_value
        )
        return Self._Mask(mlir_value=res)

    # ===------------------------------------------------------------------=== #
    # In place operations.
    # ===------------------------------------------------------------------=== #

    @inline(.nodebug)
    def __iadd__(mut self, rhs: Self):
        """Performs in-place addition.

        The vector is mutated where each element at position `i` is computed as
        `self[i] + rhs[i]`.

        Args:
            rhs: The rhs of the addition operation.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        self = self + rhs

    @inline(.nodebug)
    def __isub__(mut self, rhs: Self):
        """Performs in-place subtraction.

        The vector is mutated where each element at position `i` is computed as
        `self[i] - rhs[i]`.

        Args:
            rhs: The rhs of the operation.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        self = self - rhs

    @inline(.nodebug)
    def __imul__(mut self, rhs: Self):
        """Performs in-place multiplication.

        The vector is mutated where each element at position `i` is computed as
        `self[i] * rhs[i]`.

        Args:
            rhs: The rhs of the operation.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        self = self * rhs

    @inline(.nodebug)
    def __ifloordiv__(mut self, rhs: Self):
        """In-place flood div operator.

        The vector is mutated where each element at position `i` is computed as
        `self[i] // rhs[i]`.

        Args:
            rhs: The rhs of the operation.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        self = self // rhs

    @inline(.nodebug)
    def __iand__(mut self, rhs: Self):
        """Computes `self & rhs` and save the result in `self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        self = self & rhs

    @inline(.nodebug)
    def __ixor__(mut self, rhs: Self):
        """Computes `self ^ rhs` and save the result in `self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        self = self ^ rhs

    @inline(.nodebug)
    def __ior__(mut self, rhs: Self):
        """Computes `self | rhs` and save the result in `self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            rhs: The RHS value.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        self = self | rhs

    @inline(.nodebug)
    def __ilshift__(mut self, rhs: Self):
        """Computes `self << rhs` and save the result in `self`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            rhs: The RHS value.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        self = self << rhs

    @inline(.nodebug)
    def __irshift__(mut self, rhs: Self):
        """Computes `self >> rhs` and save the result in `self`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            rhs: The RHS value.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        self = self >> rhs

    # ===------------------------------------------------------------------=== #
    # Reversed operations
    # ===------------------------------------------------------------------=== #

    @always_inline("builtin")
    def __radd__(self, value: Self) -> Self:
        """Returns `value + self`.

        Args:
            value: The other value.

        Returns:
            `value + self`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return value + self

    @always_inline("builtin")
    def __rsub__(self, value: Self) -> Self:
        """Returns `value - self`.

        Args:
            value: The other value.

        Returns:
            `value - self`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return value - self

    @always_inline("builtin")
    def __rmul__(self, value: Self) -> Self:
        """Returns `value * self`.

        Args:
            value: The other value.

        Returns:
            `value * self`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"
        return value * self

    @inline(.nodebug)
    def __rfloordiv__(self, rhs: Self) -> Self:
        """Returns the division of rhs and self rounded down to the nearest
        integer.

        Constraints:
            The element type of the SIMD vector must be numeric.

        Args:
            rhs: The value to divide by self.

        Returns:
            `floor(rhs / self)` value.
        """
        comptime assert Self.dtype.is_numeric(), "the type must be numeric"
        return rhs // self

    @always_inline("builtin")
    def __rand__(self, value: Self) -> Self:
        """Returns `value & self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            value: The other value.

        Returns:
            `value & self`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return value & self

    @always_inline("builtin")
    def __rxor__(self, value: Self) -> Self:
        """Returns `value ^ self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            value: The other value.

        Returns:
            `value ^ self`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return value ^ self

    @always_inline("builtin")
    def __ror__(self, value: Self) -> Self:
        """Returns `value | self`.

        Constraints:
            The element type of the SIMD vector must be bool or integral.

        Args:
            value: The other value.

        Returns:
            `value | self`.
        """
        comptime assert (
            Self.dtype.is_integral() or Self.dtype == DType.bool
        ), "must be an integral or bool type"
        return value | self

    @always_inline("builtin")
    def __rlshift__(self, value: Self) -> Self:
        """Returns `value << self`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            value: The other value.

        Returns:
            `value << self`.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        return value << self

    @always_inline("builtin")
    def __rrshift__(self, value: Self) -> Self:
        """Returns `value >> self`.

        Constraints:
            The element type of the SIMD vector must be integral.

        Args:
            value: The other value.

        Returns:
            `value >> self`.
        """
        comptime assert Self.dtype.is_integral(), "must be an integral type"
        return value >> self

    # ===------------------------------------------------------------------=== #
    # Trait implementations
    # ===------------------------------------------------------------------=== #

    @stable(since="1.1")
    @inline(.nodebug)
    def __len__(self) -> Int:
        """Gets the length of the SIMD vector.

        Returns:
            The length of the SIMD vector.
        """

        return self.length

    @always_inline("builtin")
    def __bool__(self) -> Bool:
        """Converts the SIMD scalar into a boolean value.

        Returns:
            True if the SIMD scalar is non-zero and False otherwise.
        """

        var ne_zero = __mlir_op.`pop.cmp`[
            pred=__mlir_attr.`#kgen.cmp_pred<ne>`
        ](self._mlir_value, Self(0)._mlir_value)
        return Bool(mlir_value=__mlir_op.`pop.simd.reduce_or`(ne_zero))

    @inline(.nodebug)
    def __int__(self) -> Int:
        """Casts to the value to an Int. If there is a fractional component,
        then the fractional part is truncated.

        Constraints:
            The size of the SIMD vector must be 1.

        Returns:
            The value as an integer.
        """
        comptime assert Int(Self.length) == 1, "expected a scalar type"

        comptime int_width = bit_width_of[Int]()
        comptime type_width = bit_width_of[Self.dtype]()

        comptime if Self.dtype.is_unsigned() and int_width > type_width:
            # If we are casting up, prevent sign extension by first casting to
            # a large unsigned
            return self.cast[_uint_type_of_width[int_width]()]().__int__()
        else:
            return self._refine[new_size=1]().cast[DType.int]()

    @doc_hidden
    @always_inline("builtin")
    def __mlir_index__(self) -> __mlir_type.index:
        """Convert to index.

        Returns:
            The corresponding __mlir_type.index value.
        """
        comptime assert (
            Self.dtype.is_integral()
        ), "cannot index using a floating point type"
        comptime assert Self.length == SIMDLength(
            1
        ), "cannot index using a non-scalar SIMD"

        return __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.index](
            __mlir_op.`pop.cast`[
                _type=SIMD[.int, 1]._mlir_type,
                fastmathFlags=__mlir_attr.`#pop.fmf<fast>`,
            ](rebind[SIMD[Self.dtype, SIMDLength(1)]](self)._mlir_value)
        )

    @inline(.nodebug)
    def __float__(self) -> Float64:
        """Casts the value to a float.

        Constraints:
            The size of the SIMD vector must be 1.

        Returns:
            The value as a float.
        """
        comptime assert Self.length == 1, "expected a scalar type"
        return self._refine[new_size=1]().cast[DType.float64]()


    @always_inline("builtin")
    def __floor__(self) -> Self:
        """Performs elementwise floor on the elements of a SIMD vector.

        Returns:
            The elementwise floor of this SIMD vector.
        """
        return Self(mlir_value=__mlir_op.`pop.floor`(self._mlir_value))

    @always_inline("builtin")
    def __ceil__(self) -> Self:
        """Performs elementwise ceiling on the elements of a SIMD vector.

        Returns:
            The elementwise ceiling of this SIMD vector.
        """
        return Self(mlir_value=__mlir_op.`pop.ceil`(self._mlir_value))

    @always_inline("builtin")
    def __trunc__(self) -> Self:
        """Performs elementwise truncation on the elements of a SIMD vector.

        Returns:
            The elementwise truncated values of this SIMD vector.
        """
        return Self(mlir_value=__mlir_op.`pop.trunc`(self._mlir_value))

    @always_inline("builtin")
    def __abs__(self) -> Self:
        """Defines the absolute value operation.

        For signed integral element types, the absolute value of the minimum
        representable value is the minimum value itself.

        Returns:
            The absolute value of this SIMD vector.
        """
        return Self(mlir_value=__mlir_op.`pop.abs`(self._mlir_value))

    def __hash__[H: Hasher](self, mut hasher: H):
        """Updates hasher with this SIMD value.

        For a floating-point vector, `-0.0` hashes as `0.0`, since the two
        compare equal.

        Parameters:
            H: The hasher type.

        Args:
            hasher: The hasher instance.
        """

        comptime if Self.dtype.is_floating_point():
            comptime neg_zero = Scalar[Self.dtype](-0.0).to_bits()

            # The `fnuz` encodings have no negative zero, so nothing to fold.
            comptime if neg_zero != 0:
                # Match on the bit pattern rather than the value: a float
                # compare crashes the compiler for every sub-16-bit encoding
                # (MOCO-4680). The hashers start from `to_bits()` either way,
                # so this leaves every other value's hash unchanged.
                var bits = self.to_bits()
                hasher._update_with_simd(
                    bits.eq(type_of(bits)(neg_zero)).select(
                        type_of(bits)(0), bits
                    )
                )
                return

        hasher._update_with_simd(self)

    # ===------------------------------------------------------------------=== #
    # Methods
    # ===------------------------------------------------------------------=== #

    @inline(.nodebug)
    def _refine[
        new_dtype: DType = Self.dtype, new_size: SIMDLength = Self.length
    ](self) -> SIMD[new_dtype, new_size]:
        """Manually refines the SIMD vector to a specific element type and size.

        Parameters:
            new_dtype: The target DType.
            new_size: The target size of the SIMD vector.

        Returns:
            The same SIMD vector with the specified element type and size.
        """
        return rebind[SIMD[new_dtype, new_size]](self)

    @inline(.nodebug)
    def cast[target: DType](self) -> SIMD[target, Self.length]:
        """Casts the elements of the SIMD vector to the target element type.

        Parameters:
            target: The target DType.

        Returns:
            A new SIMD vector whose elements have been cast to the target
            element type.

        Casting behavior:

        ```mojo
        # Basic casting preserves value within range
        Int8(UInt8(127)) == Int8(127)

        # Numbers above signed max wrap to negative using two's complement
        Int8(UInt8(128)) == Int8(-128)
        Int8(UInt8(129)) == Int8(-127)
        Int8(UInt8(256)) == Int8(0)

        # Negative signed cast to unsigned using two's complement
        UInt8(Int8(-128)) == UInt8(128)
        UInt8(Int8(-127)) == UInt8(129)
        UInt8(Int8(-1)) == UInt8(255)

        # Truncate precision after downcast and upcast
        Float64(Float32(Float64(123456789.123456789))) == Float64(123456792.0)

        # Rightmost bits of significand become 0's on upcast
        Float64(Float32(0.3)) == Float64(0.30000001192092896)

        # Numbers equal after truncation of float literal and cast truncation
        Float32(Float64(123456789.123456789)) == Float32(123456789.123456789)

        # Float to int/uint floors
        Int64(Float64(42.2)) == Int64(42)
        ```
        """

        comptime if Self.dtype == target:
            return self._refine[target]()

        comptime if Self.dtype == DType.bool:
            return self.select[target](1, 0)
        elif target == DType.bool:
            return self.ne(0)._refine[target]()

        var res = __mlir_op.`pop.cast`[
            _type=SIMD[target, Self.length]._mlir_type,
            fastmathFlags=__mlir_attr.`#pop.fmf<fast>`,
        ](self._mlir_value)
        return SIMD(mlir_value=res)

    @always_inline("builtin")
    def is_power_of_two(self) -> SIMD[.bool, Self.length]:
        """Checks if the input value is a power of 2 for each element of a SIMD vector.

        Constraints:
            The element type of the input vector must be integral.

        Returns:
            A SIMD value where the element at position `i` is True if the integer at
            position `i` of the input value is a power of 2, False otherwise.
        """
        comptime assert Self.dtype.is_integral(), "must be integral"

        return self.gt(0) & (self & (self - 1)).eq(0)

    @inline(.always)
    def to_bits[
        _dtype: DType = _uint_type_of_width[bit_width_of[Self.dtype]()]()
    ](self) -> SIMD[_dtype, Self.length]:
        """Bitcasts the SIMD vector to an integer SIMD vector.

        Parameters:
            _dtype: The integer type to cast to.

        Returns:
            An integer representation of the floating-point value.
        """
        comptime assert (
            _dtype.is_unsigned()
        ), "the target type must be unsigned integral"
        comptime assert (
            bit_width_of[_dtype]() >= bit_width_of[Self.dtype]()
        ), "the target type must be at least as wide as the source type"

        comptime if Self.dtype == DType.bool:
            return self.cast[DType.uint8]().to_bits[_dtype]()
        else:
            comptime uint = _unsigned_integral_type_of[Self.dtype]()
            return bitcast[uint, Self.length](self).cast[_dtype]()

    @inline(.always)
    def _to_bits_signed(
        self,
    ) -> SIMD[_integral_type_of[Self.dtype](), Self.length]:
        return bitcast[_integral_type_of[Self.dtype](), Self.length](self)


    def clamp(self, lower_bound: Self, upper_bound: Self) -> Self:
        """Clamps the values in a SIMD vector to be in a certain range.

        Clamp cuts values in the input SIMD vector off at the upper bound and
        lower bound values. For example,  SIMD vector `[0, 1, 2, 3]` clamped to
        a lower bound of 1 and an upper bound of 2 would return `[1, 1, 2, 2]`.

        Args:
            lower_bound: Minimum of the range to clamp to.
            upper_bound: Maximum of the range to clamp to.

        Returns:
            A new SIMD vector containing x clamped to be within lower_bound and
            upper_bound.
        """
        return max(min(self, upper_bound), lower_bound)

    # TODO: Move to global function.
    @inline(.nodebug)
    def fma[
        flag: FastMathFlag = FastMathFlag.CONTRACT
    ](self, multiplier: Self, accumulator: Self) -> Self:
        """Performs a fused multiply-add operation, i.e.
        `self*multiplier + accumulator`.

        Parameters:
            flag: Fast-math optimization flags to apply (default: CONTRACT).

        Args:
            multiplier: The value to multiply.
            accumulator: The value to accumulate.

        Returns:
            A new vector whose element at position `i` is computed as
            `self[i]*multiplier[i] + accumulator[i]`.
        """
        comptime assert Self.dtype.is_numeric(), "the SIMD type must be numeric"

        return Self(
            mlir_value=__mlir_op.`pop.fma`[fastmathFlags=flag._mlir_attr()](
                self._mlir_value,
                multiplier._mlir_value,
                accumulator._mlir_value,
            )
        )

    # ===------------------------------------------------------------------=== #
    # select
    # ===------------------------------------------------------------------=== #

    # TODO (7748): always_inline required to WAR LLVM codegen bug
    @inline(.nodebug)
    def select[
        _dtype: DType
    ](
        self,
        true_case: SIMD[_dtype, Self.length],
        false_case: SIMD[_dtype, Self.length],
    ) -> SIMD[_dtype, Self.length]:
        """Selects the values of the `true_case` or the `false_case` based on
        the current boolean values of the SIMD vector.

        Parameters:
            _dtype: The element type of the input and output SIMD vectors.

        Args:
            true_case: The values selected if the positional value is True.
            false_case: The values selected if the positional value is False.

        Constraints:
            The element type of the vector must be boolean.

        Returns:
            A new vector of the form
            `[true_case[i] if elem else false_case[i] for i, elem in enumerate(self)]`.
        """
        comptime assert Self.dtype == DType.bool, "the simd type must be bool"
        var res = __mlir_op.`pop.simd.select`(
            self._refine[DType.bool]()._mlir_value,
            true_case._mlir_value,
            false_case._mlir_value,
        )
        return SIMD(mlir_value=res)
