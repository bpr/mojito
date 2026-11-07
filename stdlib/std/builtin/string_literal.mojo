# Upstream's `std/builtin/string_literal.mojo`. A literal `"ab"` is a
# `StringLiteral["ab"]`, whose members are these bodies. An application is
# this struct, and like upstream's it holds nothing at run time, but the
# compiler still builds it by rule (R1973), so the default initializer's
# empty body constructs nothing. `ptr` takes `pop.string.address` as the
# `Pointer` itself, where upstream's wraps the `!kgen.pointer` it gives
# with `unsafe_bitcast[Byte]()` (R221). The iterator members name the
# bundled private iterator structs (R1866), and `split` returns the
# `List[String]` `StringSpan.split` does (R189). `format` binds its
# template to a local `comptime` (R1646, R1973), and the bundled
# `format_to_comptime` parses it at run time (R425).
#
# Omitted: `as_c_string_span` and `as_c_string_slice` (R222), and the
# `ConvertibleToPython` conformance with `to_python_object` (R1455).
"""Implements the StringLiteral struct.

These are Mojo built-ins, so you don't need to import them.
"""

from std.collections.string.string_span import _get_kgen_string
from std.builtin.floatable import FloatableRaising
from std.builtin.int import IntableRaising
from std.span import Span
from std.string import (
    PathLike,
    StaticString,
    String,
    StringSlice,
    _CodepointIter,
    _CodepointSliceIter,
    _CodepointSliceReversedIter,
    _FormatUtils,
    _GraphemeIter,
    _GraphemeReversedIter,
)


@__nonmaterializable(String)
struct StringLiteral[value: __mlir_type.`!kgen.string`](
    Boolable,
    Defaultable,
    FloatableRaising,
    ImplicitlyCopyable,
    IntableRaising,
    PathLike,
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

    @always_inline("builtin")
    def __add__(
        self, rhs: StringLiteral
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

    def __mul__(self, n: Int) -> String:
        """Concatenates the string `n` times.

        Args:
            n: The number of times to concatenate the string.

        Returns:
            The string concatenated `n` times.
        """
        return StringSlice(self) * n

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
        return StringSlice(self) != rhs

    @inline(.nodebug)
    def __lt__(self, rhs: StringSlice) -> Bool:
        """Compare this value to the RHS using lesser than (LT) comparison.

        Args:
            rhs: The other value to compare against.

        Returns:
            True if this is strictly less than the RHS and False otherwise.
        """
        return StringSlice(self) < rhs

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
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @inline(.nodebug)
    def __bool__(self) -> Bool:
        """Convert the string to a bool value.

        Returns:
            True if the string is not empty.
        """
        return self.byte_length() != 0

    @inline(.always)
    def __int__(self) raises -> Int:
        """Parses the given string as a base-10 integer and returns that value.
        If the string cannot be parsed as an int, an error is raised.

        Returns:
            An integer value that represents the string, or otherwise raises.

        Raises:
            If the operation fails.
        """
        return Int(StringSlice(self))

    @inline(.always)
    def __float__(self) raises -> Float64:
        """Parses the string as a float point number and returns that value. If
        the string cannot be parsed as a float, an error is raised.

        Returns:
            A float value that represents the string, or otherwise raises.

        Raises:
            If the operation fails.
        """
        return Float64(StringSlice(self))

    def __fspath__(self) -> String:
        """Return the file system path representation of the object.

        Returns:
            The file system path representation as a string.
        """
        return String(self)

    def __iter__(self) -> _GraphemeIter[ImmStaticOrigin]:
        """Iterate over the string literal, returning grapheme clusters.

        Returns:
            An iterator over the grapheme clusters of the string literal.
        """
        return self.graphemes()

    def __reversed__(self) -> _GraphemeReversedIter[ImmStaticOrigin]:
        """Iterate backwards over the string literal, returning grapheme
        clusters.

        Returns:
            A reversed iterator over the grapheme clusters of the string
            literal.
        """
        return self.graphemes_reversed()

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

    @inline(.always)
    def count_codepoints(self) -> Int:
        """Calculates the length in Unicode codepoints encoded in the
        UTF-8 representation of this string.

        Returns:
            The length in Unicode codepoints.
        """
        return StringSlice(self).count_codepoints()

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

    def find(self, substr: StaticString, start: Int = 0) -> Int:
        """Finds the offset of the first occurrence of `substr` starting at
        `start`. If not found, returns -1.

        Args:
          substr: The substring to find.
          start: The offset from which to find.

        Returns:
          The offset of `substr` relative to the beginning of the string.
        """
        return StringSlice(self).find(substr, start=start)

    def rfind(self, substr: StaticString, start: Int = 0) -> Int:
        """Finds the offset of the last occurrence of `substr` starting at
        `start`. If not found, returns -1.

        Args:
          substr: The substring to find.
          start: The offset from which to find.

        Returns:
          The offset of `substr` relative to the beginning of the string.
        """
        return StringSlice(self).rfind(substr, start=start)

    def count(self, substr: StringSlice) -> Int:
        """Return the number of non-overlapping occurrences of substring
        `substr` in the string literal.

        Args:
          substr: The substring to count.

        Returns:
          The number of occurrences of `substr`.
        """
        return StringSlice(self).count(substr)

    def lower(self) -> String:
        """Returns a copy of the string literal with all cased characters
        converted to lowercase.

        Returns:
            A new string where cased letters have been converted to lowercase.
        """
        return StringSlice(self).lower()

    def upper(self) -> String:
        """Returns a copy of the string literal with all cased characters
        converted to uppercase.

        Returns:
            A new string where cased letters have been converted to uppercase.
        """
        return StringSlice(self).upper()

    def ascii_rjust(self, width: Int, fillchar: StaticString = " ") -> String:
        """Pads the string literal on the left with the specified fill
        character so that the total byte length of the resulting string
        equals `width`.

        Args:
            width: The total width (in bytes) of the resulting string.
            fillchar: The padding character to use (defaults to space).

        Returns:
            A right-justified string of byte length `width`.
        """
        return StringSlice(self).ascii_rjust(width, fillchar)

    def ascii_ljust(self, width: Int, fillchar: StaticString = " ") -> String:
        """Pads the string literal on the right with the specified fill
        character so that the total byte length of the resulting string
        equals `width`.

        Args:
            width: The total width (in bytes) of the resulting string.
            fillchar: The padding character to use (defaults to space).

        Returns:
            A left-justified string of byte length `width`.
        """
        return StringSlice(self).ascii_ljust(width, fillchar)

    def ascii_center(self, width: Int, fillchar: StaticString = " ") -> String:
        """Pads the string literal on both sides with the specified fill
        character so that the total length of the resulting string equals
        `width`.

        Args:
            width: The total width (in bytes) of the resulting string.
            fillchar: The padding character to use (defaults to space).

        Returns:
            A center-justified string of length `width`.
        """
        return StringSlice(self).ascii_center(width, fillchar)

    def startswith(
        self, prefix: StringSlice, start: Int = 0, end: Int = -1
    ) -> Bool:
        """Checks if the string literal starts with the specified prefix
        between start and end positions.

        Args:
            prefix: The prefix to check.
            start: The start byte offset from which to check.
            end: The end byte offset up to which to check.

        Returns:
            True if the `self[byte=start:end]` is prefixed by the input prefix.
        """
        return StringSlice(self).startswith(prefix, start, end)

    def endswith(
        self, suffix: StringSlice, start: Int = 0, end: Int = -1
    ) -> Bool:
        """Checks if the string literal end with the specified suffix between
        start and end positions.

        Args:
            suffix: The suffix to check.
            start: The start byte offset from which to check.
            end: The end byte offset up to which to check.

        Returns:
            True if the `self[byte=start:end]` is suffixed by the input suffix.
        """
        return StringSlice(self).endswith(suffix, start, end)

    def is_ascii_digit(self) -> Bool:
        """A string is a digit string if all characters in the string are
        digits and there is at least one character in the string.

        Returns:
            True if all characters are digits else False.
        """
        return StringSlice(self).is_ascii_digit()

    def isupper(self) -> Bool:
        """Returns True if all cased characters in the string literal are
        uppercase and there is at least one cased character.

        Returns:
            True if all cased characters are uppercase, False otherwise.
        """
        return StringSlice(self).isupper()

    def islower(self) -> Bool:
        """Returns True if all cased characters in the string literal are
        lowercase and there is at least one cased character.

        Returns:
            True if all cased characters are lowercase, False otherwise.
        """
        return StringSlice(self).islower()

    def strip(self) -> StaticString:
        """Return a copy of the string literal with leading and trailing
        whitespaces removed.

        Returns:
            A `StaticString` into the literal with no leading or trailing
            whitespaces.
        """
        return self.lstrip().rstrip()

    def strip(self, chars: StringSlice) -> StaticString:
        """Return a copy of the string literal with leading and trailing
        characters removed.

        Args:
            chars: A set of characters to be removed.

        Returns:
            A `StaticString` into the literal with no leading or trailing
            characters.
        """
        return self.lstrip(chars).rstrip(chars)

    def rstrip(self, chars: StringSlice) -> StaticString:
        """Return a copy of the string literal with trailing characters
        removed.

        Args:
            chars: A set of characters to be removed.

        Returns:
            A `StaticString` into the literal with no trailing characters.
        """
        return StringSlice(self).rstrip(chars)

    def rstrip(self) -> StaticString:
        """Return a copy of the string with trailing whitespaces removed.

        Returns:
            A `StaticString` into the literal with no trailing whitespaces.
        """
        return StringSlice(self).rstrip()

    def lstrip(self, chars: StringSlice) -> StaticString:
        """Return a copy of the string with leading characters removed.

        Args:
            chars: A set of characters to be removed.

        Returns:
            A `StaticString` into the literal with no leading characters.
        """
        return StringSlice(self).lstrip(chars)

    def lstrip(self) -> StaticString:
        """Return a copy of the string with leading whitespaces removed.

        Returns:
            A `StaticString` into the literal with no leading whitespaces.
        """
        return StringSlice(self).lstrip()

    def codepoint_slices(self) -> _CodepointSliceIter[ImmStaticOrigin]:
        """Iterate over the string literal, returning immutable references.

        Returns:
            An iterator of codepoint slices.
        """
        return StringSlice(self).codepoint_slices()

    def codepoint_slices_reversed(
        self,
    ) -> _CodepointSliceReversedIter[ImmStaticOrigin]:
        """Iterates backwards over the codepoint slices of this string
        literal.

        Returns:
            A reversed iterator of references to the string literal elements.
        """
        return StringSlice(self).codepoint_slices_reversed()

    def codepoints(self) -> _CodepointIter[ImmStaticOrigin]:
        """Returns an iterator over the `Codepoint`s encoded in this string
        literal.

        Returns:
            An iterator over successive `Codepoint` values.
        """
        return StringSlice(self).codepoints()

    def graphemes(self) -> _GraphemeIter[ImmStaticOrigin]:
        """Returns an iterator over the extended grapheme clusters of this
        string literal.

        Returns:
            An iterator yielding each grapheme cluster as a `StringSlice`.
        """
        return StringSlice(self).graphemes()

    def graphemes_reversed(
        self,
    ) -> _GraphemeReversedIter[ImmStaticOrigin]:
        """Returns an iterator over the extended grapheme clusters of this
        string literal, yielding them in reverse order.

        Returns:
            A reverse iterator yielding each grapheme cluster as a
            `StringSlice`.
        """
        return StringSlice(self).graphemes_reversed()

    def format[*Ts: Writable](self, *args: *Ts) -> String:
        """Produce a formatted string using the current string as a template.

        Parameters:
            Ts: The types of substitution values that implement `Writable`.

        Args:
            args: The substitution values.

        Returns:
            The template with the given values substituted.
        """
        var buffer = String()
        comptime template = StaticString(StringLiteral[Self.value]())
        _FormatUtils.format_to_comptime[template](buffer, *args)
        return buffer^

    def join[T: Copyable & Writable, //](self, elems: Span[T, _]) -> String:
        """Joins string elements using the current string as a delimiter.

        Parameters:
            T: The type of the elements.

        Args:
            elems: The input values.

        Returns:
            The joined string.
        """
        var result = self
        return result.join(elems)

    @inline(.always)
    def split(self, sep: StringSlice) -> List[String]:
        """Split the string by a separator.

        Args:
            sep: The string to split on.

        Returns:
            A List of Strings containing the input split by the separator.
        """
        return StringSlice(self).split(sep)

    @inline(.always)
    def split(self, sep: StringSlice, maxsplit: Int) -> List[String]:
        """Split the string by a separator.

        Args:
            sep: The string to split on.
            maxsplit: The maximum amount of items to split from String.

        Returns:
            A List of Strings containing the input split by the separator.
        """
        return StringSlice(self).split(sep, maxsplit=maxsplit)

    @inline(.always)
    def split(self, sep: NoneType = None) -> List[String]:
        """Split the string by every Whitespace separator.

        Args:
            sep: None.

        Returns:
            A List of Strings containing the input split by the separator.
        """
        return StringSlice(self).split(sep)

    @inline(.always)
    def split(self, sep: NoneType = None, *, maxsplit: Int) -> List[String]:
        """Split the string by every Whitespace separator.

        Args:
            sep: None.
            maxsplit: The maximum amount of items to split from String.

        Returns:
            A List of Strings containing the input split by the separator.
        """
        return StringSlice(self).split(sep, maxsplit=maxsplit)

    @always_inline("builtin")
    def _is_identical(self, rhs: StringLiteral) -> Bool:
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

        writer.write(StringSlice(self))

    def write_repr_to(self, mut writer: Some[Writer]):
        """Write the string representation of the string literal.

        Args:
            writer: The value to write to.
        """
        StringSlice(self).write_repr_to(writer)
