# Upstream's `std/builtin/format_int.mojo`, in part: `_write_int`, the
# digit writer `SIMD.write_to` calls for an integral lane, and its digit
# table. `bin`, `hex`, `oct`, and `_format_int` are not ported. Three lines
# of `_write_int` are respelled, each by the same arithmetic:
# - `floor_div` divides with `//`, where upstream's `/` (R184): the two
#   agree on the non-negative operand it takes;
# - `ceil_div` inlines upstream's `SIMD.__ceildiv__` body, where upstream
#   calls it, with its `divmod` as `//` and `%` (R1623);
# - `neg_digit_value` calls `__abs__`, the member upstream's `abs` calls,
#   where upstream calls `abs` (R1458).
"""Provides the `hex` and `bin` functions.

These are Mojo built-ins, so you don't need to import them.
"""

from std.collections import Array
from std.span import Span
from std.string import StaticString, StringSlice
from std.sys import bit_width_of

comptime _DEFAULT_DIGIT_CHARS = "0123456789abcdefghijklmnopqrstuvwxyz"


def _write_int[
    dtype: DType,
    W: Writer,
    //,
    *,
    radix: Int = 10,
    digit_chars: StaticString = _DEFAULT_DIGIT_CHARS,
](mut writer: W, value: Scalar[dtype], *, prefix: StaticString = "",):
    """Writes a formatted string representation of the given integer using the
    specified radix.

    The maximum supported radix is 36 unless a custom `digit_chars` mapping is
    provided.
    """
    comptime assert dtype.is_integral(), "Expected integral"
    comptime assert (
        radix >= 2
    ), "Unable to format integer to string with radix < 2"
    comptime assert radix <= digit_chars.byte_length(), (
        "Unable to format integer to string when provided radix is larger than"
        " length of available digit value characters"
    )
    comptime assert digit_chars.byte_length() >= 2, (
        "Unable to format integer to string when provided digit_chars mapping"
        " len is not >= 2"
    )

    # Process the integer value into its corresponding digits

    # TODO(#26444, Unicode support): Get an array of Character, not bytes.
    var digit_chars_array = digit_chars.as_bytes()

    # Prefix a '-' if the original int was negative and make positive.
    if value < 0:
        writer.write_string("-")

    # Add the custom number prefix, e.g. "0x" commonly used for hex numbers.
    # This comes *after* the minus sign, if present.
    writer.write(prefix)

    if value == 0:
        # TODO: Replace with safe digit_chars[:1] syntax.
        # SAFETY:
        #   This static origin is valid as long as we're using a
        #   `StaticString` for `digit_chars`.
        var zero_char = digit_chars_array.unsafe_ptr()[]

        # Construct a null-terminated buffer of single-byte char.
        var zero_buf: Array[UInt8, 2] = [zero_char, 0]

        # TODO(MSTDL-720):
        #   Support printing non-null-terminated strings on GPU and switch
        #   back to this code without a workaround.
        # ptr=digit_chars_array,
        writer.write(
            StringSlice(
                unsafe_from_utf8=Span(
                    unsafe_ptr=zero_buf.unsafe_ptr(), length=1
                )
            )
        )

        return

    # Create a buffer to store the formatted value

    # Stack allocate enough bytes to store any formatted integer up to 256 bits
    # TODO: use a dynamic size when #2194 is resolved
    # +1 for storing NUL terminator.
    comptime CAPACITY: Int = max(64, bit_width_of[dtype]()) + 1

    var buf = Array[UInt8, CAPACITY](uninitialized=True)

    # Start the buf pointer at the end. We will write the least-significant
    # digits later in the buffer, and then decrement the pointer to move
    # earlier in the buffer as we write the more-significant digits.
    var offset = CAPACITY - 1

    buf.unsafe_ptr().unsafe_offset(offset).write(
        0
    )  # Write NUL terminator at the end

    # Position the offset to write the least-significant digit just before the
    # NUL terminator.
    offset -= 1

    # Write the digits of the number
    var remaining_int = value

    def process_digits[
        get_digit_value: def(Scalar[dtype]) thin -> Scalar[dtype],
        div_fn: def(Scalar[dtype]) thin -> Scalar[dtype],
    ]() {mut}:
        while remaining_int:
            var digit_value = get_digit_value(remaining_int)

            # Write the char representing the value of the least significant
            # digit.
            buf.unsafe_ptr().unsafe_offset(offset).write(
                digit_chars_array.unsafe_ptr()[unsafe_offset=Int(digit_value)]
            )

            # Position the offset to write the next digit.
            offset -= 1

            # TODO: (MOCO-3028)
            # We should be able to simply do `remaining_int /= radix` here,
            # however, there is a compiler bug when using `/` in parameter
            # vs argument for signed SIMD values.
            #
            # Drop the least significant digit
            remaining_int = div_fn(remaining_int)

    if remaining_int >= 0:

        def pos_digit_value(value: Scalar[dtype]) -> Scalar[dtype]:
            return value % Scalar[dtype](radix)

        def floor_div(value: Scalar[dtype]) -> Scalar[dtype]:
            return value // Scalar[dtype](radix)

        process_digits[pos_digit_value, floor_div]()
    else:

        def neg_digit_value(value: Scalar[dtype]) -> Scalar[dtype]:
            return (value % Scalar[dtype](-radix)).__abs__()

        def ceil_div(value: Scalar[dtype]) -> Scalar[dtype]:
            comptime if dtype.is_signed():
                return -(value // -Scalar[dtype](radix))
            var quotient = value // Scalar[dtype](radix)
            var remainder = value % Scalar[dtype](radix)
            return quotient + remainder.ne(Scalar[dtype](0)).cast[dtype]()

        process_digits[neg_digit_value, ceil_div]()

    # Re-add +1 byte since the loop ended so we didn't write another char.
    offset += 1

    # Create a span to only those bytes in `buf` that have been initialized.
    # -1 because NUL terminator
    writer.write_string(
        StringSlice(
            unsafe_from_utf8=Span(
                unsafe_ptr=buf.unsafe_ptr().unsafe_offset(offset),
                length=len(buf) - offset - 1,
            )
        )
    )
