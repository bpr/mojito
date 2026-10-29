# Upstream's `std/builtin/string_literal.mojo`, in part: the
# `StringLiteral[value]` struct with its default initializer, `__bool__`,
# `byte_length`, `ptr`, `as_string_slice`, `as_bytes`, `write_to`, and
# `write_repr_to`. A literal's own type is still the compiler's
# `StringLiteral`, and a bare `StringLiteral` annotation names it (R1652),
# so this struct is what an explicit application `StringLiteral[v]` names.
# The comparisons and `__add__` (R1653), `__hash__` (R1654), and the
# members whose bodies read `StringSlice(self)` (R1655) are not ported, and
# for that reason `write_to` and `write_repr_to` read `as_string_slice()`.
# `@__nonmaterializable(String)` is not read (R1651). The struct declares
# only the traits whose requirements are ported here. A member reads its
# parameter as `Self.value` where upstream's spell `self.value` (R1447),
# and `ptr` takes `pop.string.address` as the `Pointer` itself, where
# upstream's wraps the `!kgen.pointer` it gives (R1316).
"""Implements the StringLiteral struct.

These are Mojo built-ins, so you don't need to import them.
"""

from std.span import Span
from std.string import StaticString


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
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @inline(.nodebug)
    def __bool__(self) -> Bool:
        """Convert the string to a bool value.

        Returns:
            True if the string is not empty.
        """
        return self.byte_length() != 0

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
            SIMDLength(mlir_value=__mlir_op.`pop.string.size`(Self.value))
        )

    @inline(.nodebug)
    def ptr(
        self,
    ) -> Pointer[Byte, ImmStaticOrigin]:
        """Get a pointer to the underlying data.

        Returns:
            The pointer to the data.
        """
        return __mlir_op.`pop.string.address`(Self.value)

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
