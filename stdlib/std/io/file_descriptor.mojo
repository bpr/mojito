"""Higher level abstraction for file streams: `FileDescriptor`, a `Writer`
over a raw descriptor (`print(..., file=fd)` writes through it)."""

from std.ffi import c_int, c_ssize_t, external_call
from std.os import abort
from std.span import Span
from std.string import String, StringSpan
from .file import FileHandle


struct FileDescriptor(Copyable, ImplicitlyCopyable, Movable, TrivialRegisterPassable, Writer):
    """File descriptor of a file."""

    var value: Int

    def __init__(out self, value: Int = 1):
        self.value = value

    def __init__(out self, f: FileHandle):
        self.value = f._get_raw_fd()

    def write_bytes(mut self, bytes: Span[Byte, _]):
        var written = external_call["write", c_ssize_t](
            self.value, bytes.unsafe_ptr(), len(bytes)
        )
        if written != len(bytes):
            abort("expected amount of bytes not written")

    def write_string(mut self, string: String):
        self.write_bytes(string.as_bytes())

    def read_bytes[origin: Origin[mut=True]](mut self, buffer: Span[Byte, origin]) raises -> Int:
        var read = external_call["read", c_ssize_t](
            self.value, buffer.unsafe_ptr(), len(buffer)
        )
        if read < 0:
            raise Error("Failed to read bytes.")
        return read
