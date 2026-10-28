# Upstream's `std/simd.mojo`, in part: the `SIMD` struct, its storage, and
# its instance `__len__`, `__getitem__`, and `__setitem__`. The other members
# are still served by the compiler (R1550–R1555, R330, R276), and the struct
# declares only the traits whose requirements are ported here. `Scalar`,
# `Int`, `UInt`, the sized aliases, and `Byte` stay compiler names until
# R1557. Omitted: the `@lldb_formatter_wrapping_type` decorator
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

from std.builtin.simd_length import SIMDLength


# ===----------------------------------------------------------------------=== #
# SIMD
# ===----------------------------------------------------------------------=== #


@stable(since="1.1")
struct SIMD[dtype: DType, length: SIMDLength](Sized, TrivialRegisterPassable):
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
