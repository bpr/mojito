# Upstream's `std/builtin/string_literal.mojo`, in part: the
# `StringLiteral[value]` struct with its default initializer, `__bool__`,
# `__add__`, the comparisons, `byte_length`, `ptr`, `unsafe_ptr`,
# `as_string_slice`, `as_bytes`, `_is_identical`, `write_to`, and
# `write_repr_to`. A literal `"ab"` is a `StringLiteral["ab"]`, whose
# members are these bodies. The compiler holds an application as its
# literal type (R1660), and builds one by rule, so the default
# initializer's empty body constructs nothing. A bare run-time
# `StringLiteral` argument is still a value-less compiler type rather than
# upstream's `StringLiteral[_]` (R1658), which is why `_is_identical` and
# `__add__` spell their `rhs` as `StringLiteral[_]`. `__hash__` (R1654)
# and the members whose bodies read `StringSlice(self)` (R1655) are not
# ported, and for that reason the comparisons, `write_to`, and
# `write_repr_to` read `as_string_slice()`.
# `@__nonmaterializable(String)` is not read (R1651). The struct declares
# only the traits whose requirements are ported here. `ptr` takes
# `pop.string.address` as the `Pointer` itself, where upstream's wraps the
# `!kgen.pointer` it gives with `unsafe_bitcast[Byte]()` (R221).
"""Implements the StringLiteral struct.

These are Mojo built-ins, so you don't need to import them.
"""

from std.collections.string.string_span import _get_kgen_string
from std.span import Span
from std.string import StaticString, StringSlice


struct StringLiteral[value: __mlir_type.`!kgen.string`](
    Boolable,
    Defaultable,
    ImplicitlyCopyable,
    TrivialRegisterPassable,
    Writable,
):
    """This type represents a string literal.

    String literals are all null-terminated for compatibility with C APIs, but
    this is subject to change. String literals store their length as an integer,
    and this does not include the null terminator.

    Parameters:
        value: The underlying string value.
    """

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    @always_inline("builtin")
    def __init__(out self):
        """Constructor for any value."""
        pass

    # ===-------------------------------------------------------------------===#
    # Factory methods
    # ===-------------------------------------------------------------------===#

    comptime _from[s: StaticString] = StringLiteral[_get_kgen_string[s]()]
    """Creates a string literal from a static string at compile time."""

    # ===-------------------------------------------------------------------===#
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @inline(.nodebug)
    def __bool__(self) -> Bool:
        """Convert the string to a bool value.

        Returns:
            True if the string is not empty.
        """
        return self.byte_length() != 0

    def __add__(
        self, rhs: StringLiteral[_]
    ) -> StringLiteral[
        __mlir_attr[
            `#pop.string_concat<`,
            self.value,
            `,`,
            rhs.value,
            `> : !kgen.string`,
        ]
    ]:
        """Concatenate two string literals.

        Args:
            rhs: The string to concat.

        Returns:
            The concatenated string.
        """
        return {}

    @inline(.nodebug)
    def __eq__(self, rhs: StringSlice) -> Bool:
        """Compare two string literals for equality.

        Args:
            rhs: The string to compare.

        Returns:
            True if they are equal.
        """
        return not (self != rhs)

    @inline(.nodebug)
    def __ne__(self, rhs: StringSlice) -> Bool:
        """Compare two string literals for inequality.

        Args:
            rhs: The string to compare.

        Returns:
            True if they are not equal.
        """
        return self.as_string_slice() != rhs

    @inline(.nodebug)
    def __lt__(self, rhs: StringSlice) -> Bool:
        """Compare this value to the RHS using lesser than (LT) comparison.

        Args:
            rhs: The other value to compare against.

        Returns:
            True if this is strictly less than the RHS and False otherwise.
        """
        return self.as_string_slice() < rhs

    @inline(.nodebug)
    def __le__(self, rhs: StringSlice) -> Bool:
        """Compare this value to the RHS using lesser than or equal to (LE) comparison.

        Args:
            rhs: The other value to compare against.

        Returns:
            True if this is less than or equal to the RHS and False otherwise.
        """
        return not (rhs < self)

    @inline(.nodebug)
    def __gt__(self, rhs: StringSlice) -> Bool:
        """Compare this value to the RHS using greater than (GT) comparison.

        Args:
            rhs: The other value to compare against.

        Returns:
            True if this is strictly greater than the RHS and False otherwise.
        """
        return rhs < self

    @inline(.nodebug)
    def __ge__(self, rhs: StringSlice) -> Bool:
        """Compare this value to the RHS using greater than or equal to (GE) comparison.

        Args:
            rhs: The other value to compare against.

        Returns:
            True if this is greater than or equal to the RHS and False otherwise.
        """
        return not (self < rhs)

    # ===-------------------------------------------------------------------===#
    # Methods
    # ===-------------------------------------------------------------------===#

    @always_inline("builtin")
    def byte_length(self) -> Int:
        """Get the string length in bytes.

        Returns:
            The length of this string in bytes.

        Notes:
            This does not include the trailing null terminator in the count.
        """
        return Int(
            SIMDLength(mlir_value=__mlir_op.`pop.string.size`(self.value))
        )

    @inline(.nodebug)
    def ptr(
        self,
    ) -> Pointer[Byte, ImmStaticOrigin]:
        """Get a pointer to the underlying data.

        Returns:
            The pointer to the data.
        """
        return __mlir_op.`pop.string.address`(self.value)

    @doc_hidden
    @inline(.nodebug)
    @deprecated(use=ptr)
    def unsafe_ptr(
        self,
    ) -> Pointer[Byte, ImmStaticOrigin]:
        """Get a pointer to the underlying data.

        Returns:
            The pointer to the data.
        """
        return self.ptr()

    @inline(.nodebug)
    def as_string_slice(self) -> StaticString:
        """Returns a string slice of this static string literal.

        Returns:
            A string slice pointing to this static string literal.
        """

        # FIXME(MSTDL-160):
        #   Enforce UTF-8 encoding in StringLiteral so this is actually
        #   guaranteed to be valid.
        return StaticString(
            unsafe_from_utf8=Span(
                unsafe_ptr=self.ptr(),
                length=self.byte_length(),
            )
        )

    @inline(.nodebug)
    def as_bytes(self) -> Span[Byte, ImmStaticOrigin]:
        """
        Returns a contiguous Span of the bytes owned by this string.

        Returns:
            A contiguous slice pointing to the bytes owned by this string.
        """

        return Span[Byte, ImmStaticOrigin](
            unsafe_ptr=self.ptr(), length=self.byte_length()
        )

    @always_inline("builtin")
    def _is_identical(self, rhs: StringLiteral[_]) -> Bool:
        """Returns whether two string literals have identical KGEN parameters.
        """
        return __mlir_attr[
            `#kgen.param.identical<`,
            self.value,
            `, `,
            rhs.value,
            `> : !kgen.scalar<bool>`,
        ]

    def write_to(self, mut writer: Some[Writer]):
        """
        Formats this string literal to the provided Writer.

        Args:
            writer: The object to write to.
        """

        writer.write(self.as_string_slice())

    def write_repr_to(self, mut writer: Some[Writer]):
        """Write the string representation of the string literal.

        Args:
            writer: The value to write to.
        """
        self.as_string_slice().write_repr_to(writer)
