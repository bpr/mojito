"""Upstream's `std/io/io.mojo` subset: `print`. `input` is a compiler
builtin here."""

from std.format._utils import _FlushingWriteBuffer
from std.range import range
from std.string import StringSlice
from std.sys import stdout
from .file_descriptor import RawFileDescriptor


@inline(.never)
def print[
    *Ts: Writable
](
    *values: *Ts,
    sep: StringSlice = " ",
    end: StringSlice = "\n",
    flush: Bool = False,
    var file: RawFileDescriptor = stdout,
):
    """Prints elements to the text stream. Each element is separated by `sep`
    and followed by `end`.

    Parameters:
        Ts: The elements types.

    Args:
        values: The elements to print.
        sep: The separator used between elements.
        end: The String to write after printing the elements.
        flush: If set to true, then the stream is forcibly flushed.
        file: The output stream.
    """

    comptime assert Ts.all_conforms_to[
        Writable
    ]()  # satisfy _write_to where clause.

    # Upstream's plugin `print_emit_fn` and GPU branches are not ported, as
    # `std._plugin` omits the hook (R1884); the CPU branch is the comptime
    # interpreter's too.
    var buffer = _FlushingWriteBuffer(file)
    # Upstream's `values._write_to(buffer, sep=sep, end=end)`, whose bundled
    # delimiters are `StaticString`s, not `print`'s `StringSlice`s (R1926).
    comptime for i in range(values.__len__()):
        comptime if i > 0:
            buffer.write(sep)
        values[i].write_to(buffer)
    buffer.write(end)

    buffer.flush()
    # Upstream's `if flush: _flush(file=file)` waits on the libc stream
    # callees `_flush` needs (R1883); nothing here writes through C stdio.
