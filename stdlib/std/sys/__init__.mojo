"""System facilities: the libc `errno` vocabulary (`std.sys._libc_errno`),
the standard streams (`stdin`, `stdout`, `stderr`), and `llvm_intrinsic`."""

from ._io import stderr, stdin, stdout
from .intrinsics import llvm_intrinsic
