# Upstream's `std/builtin/_format_float.mojo`, in part: `_write_float`, the
# writer `SIMD.write_to` calls for a floating-point lane. Upstream's body is
# Dragonbox over `UInt128` tables; until R159 ports it, this body is a
# staged leaf that writes the shortest round-trip text of the value widened
# to `Float64`, which the compiler's `_mojito_float_text` puts in a byte
# buffer. The leaf and its builtin go with R159. The `where
# dtype.is_floating_point()` clause is a `comptime assert` until a `where`
# clause takes a `DType` predicate (R1661).
"""Implements the float writer `SIMD.write_to` calls."""

from std.collections import Array
from std.span import Span
from std.string import StringSlice


def _write_float[
    W: Writer, dtype: DType, //
](mut writer: W, value: Scalar[dtype]):
    """Write a SIMD float type into a Writer.

    Parameters:
        W: The type of the Writer.
        dtype: The type of the float being passed in.

    Args:
        writer: The Writer to write the float to.
        value: The float to write into the Writer.
    """
    comptime assert dtype.is_floating_point(), "Expected floating point"
    var digits = Array[Byte, 32](uninitialized=True)
    var length = _mojito_float_text(
        value.cast[DType.float64](), digits.unsafe_ptr()
    )
    writer.write_string(
        StringSlice(
            unsafe_from_utf8=Span(unsafe_ptr=digits.unsafe_ptr(), length=length)
        )
    )
