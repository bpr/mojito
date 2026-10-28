"""System facilities: the libc `errno` vocabulary (`std.sys._libc_errno`),
the standard streams (`stdin`, `stdout`, `stderr`), `llvm_intrinsic`, and
the `_RegisterPackType` its struct-valued intrinsics return."""

from ._io import stderr, stdin, stdout
from .intrinsics import _RegisterPackType, llvm_intrinsic
