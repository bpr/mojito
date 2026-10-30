"""System facilities: the libc `errno` vocabulary (`std.sys._libc_errno`),
the standard streams (`stdin`, `stdout`, `stderr`), the target and type
queries `CompilationTarget`, `simd_bit_width`, `simd_byte_width`,
`size_of`, `align_of`, `bit_width_of`, and `simd_width_of`
(`std.sys.info`), `llvm_intrinsic`, and the
`_RegisterPackType` its struct-valued intrinsics return."""

from ._io import stderr, stdin, stdout
from .info import (
    CompilationTarget,
    align_of,
    bit_width_of,
    simd_bit_width,
    simd_byte_width,
    simd_width_of,
    size_of,
)
from .intrinsics import _RegisterPackType, llvm_intrinsic
