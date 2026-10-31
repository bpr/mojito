# Upstream's `std/builtin/int_literal.mojo`, in part: the
# `IntLiteral[value]` struct with its default initializer, `__int__`,
# `__ceil__`, `__floor__`, `__trunc__`, `write_to`, `write_repr_to`, and
# `__mlir_index__`. A literal `4` is its application `IntLiteral[4]`, as
# is an explicit one (`IntLiteral[(4).value]()`), and a bare `IntLiteral`
# parameter is upstream's per-call `IntLiteral[_]`. The operator
# dunders, `__bool__` (which reads `self != 0`), and `__ceildiv__` are not
# ported (R1753). `@__nonmaterializable(Int)` is not read (R1651). The
# struct declares only the traits whose requirements are ported here.
"""Implements the IntLiteral class."""

from std.math import Ceilable, Floorable, Truncable


struct IntLiteral[value: __mlir_type.`!pop.int_literal`](
    Ceilable,
    Defaultable,
    Floorable,
    Indexer,
    Intable,
    TrivialRegisterPassable,
    Truncable,
    Writable,
):
    """This type represents a static integer literal value with
    infinite precision.  This type is a compile-time construct which stores its
    value as a parameter.  It is typically materialized into other types (like
    `Int`) for use at runtime.  This compile-time representation allows for
    arbitrary precision constants that would overflow on Int and other fixed
    precision integer types.

    Parameters:
        value: The underlying integer value.
    """

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    @always_inline("builtin")
    def __init__(out self):
        """Constructor for any value."""
        pass

    # ===-------------------------------------------------------------------===#
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @always_inline("builtin")
    def __int__(self) -> Int:
        """Convert from IntLiteral to Int.

        Returns:
            The value as an integer of platform-specific width.
        """
        return Int(SIMDLength(mlir_value=self.__mlir_index__()))

    @always_inline("builtin")
    def __ceil__(self) -> Self:
        """Return the ceiling of the IntLiteral value, which is itself.

        Returns:
            The IntLiteral value itself.
        """
        return self

    @always_inline("builtin")
    def __floor__(self) -> Self:
        """Return the floor of the IntLiteral value, which is itself.

        Returns:
            The IntLiteral value itself.
        """
        return self

    @always_inline("builtin")
    def __trunc__(self) -> Self:
        """Return the truncated of the IntLiteral value, which is itself.

        Returns:
            The IntLiteral value itself.
        """
        return self

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        """Writes the IntLiteral in string form.

        Args:
            writer: The Writer to write the value to.
        """
        Int(self).write_to(writer)

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        """Writes the IntLiteral in repr form.

        Args:
            writer: The Writer to write the value to.
        """
        Int(self).write_repr_to(writer)

    # ===-------------------------------------------------------------------===#
    # Methods
    # ===-------------------------------------------------------------------===#

    @doc_hidden
    @always_inline("builtin")
    def __mlir_index__(self) -> __mlir_type.index:
        """Convert from IntLiteral to index.

        Returns:
            The corresponding __mlir_type.index value, interpreting as signed.
        """
        return __mlir_attr[
            `#kgen.cast_to_builtin<#pop.int_literal_convert<`,
            self.value,
            `> : !kgen.scalar<index>> : index`,
        ]
