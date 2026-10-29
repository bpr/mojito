# Upstream's `std/sys/info.mojo`, in part: `bit_width_of`, and the home of
# `size_of`, which stays the compiler's layout query (upstream's body is a
# `#kgen.param.expr<get_sizeof>` attribute). Omitted: `CompilationTarget`
# and its feature queries (R1562), its string-field predicates and the
# free target functions (R1635), `align_of` and the `target` parameter of
# `size_of` and `bit_width_of` (R1634), and `stdlib_plugin` (R330).
"""Implements methods for querying the host target info.

You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import CompilationTarget

print(CompilationTarget.is_x86())
```
"""


@inline(.nodebug)
def bit_width_of[
    type: RegisterPassable,
]() -> Int:
    """Returns the size of (in bits) of the type.

    Parameters:
        type: The type in question.

    Returns:
        The size of the type in bits.
    """
    comptime CHAR_BIT = 8
    return CHAR_BIT * size_of[type]()


@inline(.nodebug)
def bit_width_of[dtype: DType]() -> Int:
    """Returns the size of (in bits) of the dtype.

    Parameters:
        dtype: The type in question.

    Returns:
        The size of the dtype in bits.
    """
    return bit_width_of[Scalar[dtype]._mlir_type]()
