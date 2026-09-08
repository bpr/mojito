"""Core I/O operations: file handling and the standard streams (subset of
upstream's `io`; `print`, `input`, `Writer`, and `Writable` are compiler
builtins here)."""

from .file import FileHandle, open
from .file_descriptor import FileDescriptor
