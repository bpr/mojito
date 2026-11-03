"""Core I/O operations: file handling, the standard streams, and `print`
(subset of upstream's `io`; `input` is a compiler builtin here, and
`Writable` and `Writer` are `std.format`'s)."""

from .file import FileHandle, open
from .file_descriptor import RawFileDescriptor
from .io import print
