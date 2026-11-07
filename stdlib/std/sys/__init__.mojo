"""System facilities: the libc `errno` vocabulary (`std.sys._libc_errno`),
the standard streams (`stdin`, `stdout`, `stderr`), the target and type
queries `CompilationTarget`, `is_gpu`, `is_nvidia_gpu`, `is_amd_gpu`,
`is_apple_gpu`, `simd_bit_width`, `simd_byte_width`, `size_of`,
`align_of`, `bit_width_of`, and `simd_width_of` (`std.sys.info`), `llvm_intrinsic`, and the
`_RegisterPackType` its struct-valued intrinsics return, and the
compile-time define reads `is_defined`, `get_defined_bool`,
`get_defined_int`, and `get_defined_string` (`std.sys.defines`)."""

from ._io import stderr, stdin, stdout
from .info import (
    CompilationTarget,
    align_of,
    bit_width_of,
    is_amd_gpu,
    is_apple_gpu,
    is_gpu,
    is_nvidia_gpu,
    simd_bit_width,
    simd_byte_width,
    simd_width_of,
    size_of,
)
from .intrinsics import _RegisterPackType, llvm_intrinsic
from .defines import (
    get_defined_bool,
    get_defined_int,
    get_defined_string,
    is_defined,
)
