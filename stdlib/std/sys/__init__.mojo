"""System facilities: the libc `errno` vocabulary (`std.sys._libc_errno`),
the standard streams (`stdin`, `stdout`, `stderr`), the type queries
`bit_width_of` and `size_of` (`std.sys.info`), `llvm_intrinsic`, and the
`_RegisterPackType` its struct-valued intrinsics return."""

from ._io import stderr, stdin, stdout
from .info import bit_width_of, size_of
from .intrinsics import _RegisterPackType, llvm_intrinsic
