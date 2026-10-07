"""Core I/O operations: file handling and the standard streams (subset of
upstream's `io`; `print`, `input`, and `Writable` are compiler builtins
here, and `Writer` is `std.format`'s)."""

from .file import FileHandle, open
from .file_descriptor import FileDescriptor
