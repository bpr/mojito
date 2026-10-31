# Upstream's `std/sys/info.mojo`, in part: `_TargetType`,
# `_current_target`, `_ANY`, `_empty_target`, the `CompilationTarget`
# struct with `current`, `_unknown`, `unsupported_target_error`,
# `is_triple`, `_has_feature`, the target attribute getters (`_arch`,
# `_os`, `__triple`, `__triple_arch`, `__arch`, `_is_arch`,
# `_is_triple_arch`), `default_compile_options`, its x86, Neon, and RISC-V
# feature queries, the platform, Apple, accelerator, and OS predicates,
# the free `is_triple`, `is_apple_m5`, `is_gpu`, `is_nvidia_gpu`,
# `is_amd_gpu`, `is_apple_gpu`, the AMD architecture families,
# `is_32bit`, `is_64bit`, `simd_bit_width`, `simd_byte_width`,
# `stdlib_plugin`, `size_of`, `align_of`, `bit_width_of`, and
# `simd_width_of`, verbatim.
# A bare `StringLiteral._from[name]()` does not resolve yet (R1658), so
# the bodies spell the alias's definition,
# `StringLiteral[_get_kgen_string[name]()]()`.
# Omitted: `current_accelerator` (R1757), `_eq_triple_and_arch`
# (R1755), the Nvidia architecture methods and the free `_SM_*X_ARCHS`
# and `_is_sm_*` (R1756, R1754), `platform_map` and `platform_type_map`
# (R1758), `Endian`, `is_little_endian`, and `is_big_endian` (R1759),
# `_accelerator_arch`, the core counts, `has_accelerator`, and
# `_macos_version`, and `_first_conforming_plugin`, with the
# `TargetPluginSelector` import (R1694).
# The `stdlib_plugin` member is `DefaultPlugin.HookFor[Self()]`, where
# upstream's is `Self._first_conforming_plugin[StdPlugin,
# default=DefaultPlugin].HookFor[Self()]`. The two agree on every target
# without an `opaque_plugin` payload, which is every target Mojito builds,
# and R1694 replaces it.
"""Implements methods for querying the host target info.

You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import CompilationTarget

print(CompilationTarget.is_x86())
```
"""

from std.collections.list import List
from std.collections.optional import Optional
from std.collections.string.string_span import _get_kgen_string
from std.string import StaticString, String
from std._plugin import StdPlugin, DefaultPlugin, PluginHooks

comptime _TargetType = __mlir_type.`!kgen.target`


@inline(.nodebug)
def _current_target() -> _TargetType:
    return __mlir_attr.`#kgen.param.expr<current_target> : !kgen.target`


comptime _ANY = "<any>"

comptime _empty_target = __mlir_attr[
    `#kgen.target<triple = "", `,
    `arch = "", `,
    `features = "", `,
    `data_layout="",`,
    `index_bit_width = 0,`,
    `simd_bit_width = 0`,
    `> : !kgen.target`,
]
"""Empty target configuration for when no GPU is available."""


struct CompilationTarget[_mlir_value: _TargetType = _current_target(), //](
    TrivialRegisterPassable
):
    """A struct that provides information about a target architecture.

    This struct encapsulates various methods to query target-specific
    information such as architecture features, OS details, endianness, and
    memory characteristics.

    Parameters:
        _mlir_value: The raw target architecture to query. Defaults to the current target.
    """

    # FIXME(MOCO-5023):
    #   The comptime interpreter will incorrectly optimize out side effects from
    #   calls to functions that return empty structs, so use a dummy field to
    #   make this struct non-empty.
    #
    #   This ensures that in a call like `current_accelerator().is_amd_gpu()`,
    #   any assertions inside of `current_accelerator()` can still fire when
    #   that condition is being evaluated by the comptime interpreter.
    var _dummy: Int

    @always_inline("builtin")
    def __init__(out self):
        """Initialize a `CompilationTarget` with the default target."""
        self._dummy = 0

    # ===-------------------------------------------------------------------===#
    # Factory methods
    # ===-------------------------------------------------------------------===#

    comptime current = CompilationTarget[_mlir_value=_current_target()]
    """Get the current compilation target.

    This can vary within a single Mojo compiler invocation, depending on
    the compilation context of a given piece of Mojo code:

    * Typically, this is the host compilation target, configured by the
        [target options](https://mojolang.org/docs/cli/build/#target-options),
        such as `--target-triple`, `--target-cpu`, and `--target-features`.

    * Within an accelerator "offload" compilation, this is the accelerator
        target, typically equivalent to `max.gpu.host.default_accelerator()`.

    Returns:
        A value representing the current compilation target.
    """

    comptime _unknown = CompilationTarget[_mlir_value=_empty_target]
    """Get an empty target configuration representing an unknown target."""

    # ===-------------------------------------------------------------------===#
    # Methods
    # ===-------------------------------------------------------------------===#

    @inline(.nodebug)
    @staticmethod
    def unsupported_target_error[
        *,
        operation: Optional[String] = None,
        note: Optional[String] = None,
    ]() -> Never:
        """Produces a constraint failure when called indicating that some
        operation is not supported by the current compilation target.

        Parameters:
            operation: Optional name of the operation that is not supported.
                Should be a function name or short description.
            note: Optional additional note to print.
        """

        comptime note_text = String(" Note: ", note.or_else(""))
        comptime msg = "Current compilation target does not support"
        comptime op_text = String(
            " operation: ", operation.value(), "."
        ) if operation else " this operation."
        comptime assert False, String(msg, op_text, note_text)

    @inline(.nodebug)
    @staticmethod
    def is_triple[name: StringLiteral]() -> Bool:
        """Returns True if the triple of this target matches the input and
        False otherwise.

        Parameters:
            name: The name of the triple value.

        Returns:
            True if the triple matches and False otherwise.
        """
        return StringLiteral[Self.__triple()]()._is_identical(name)

    @inline(.nodebug)
    @staticmethod
    def _has_feature[name: StaticString]() -> Bool:
        """Checks if the target has a specific feature.

        Parameters:
            name: The name of the feature to check.

        Returns:
            True if the target has the specified feature, False otherwise.
        """
        return __mlir_attr[
            `#kgen.param.expr<target_has_feature,`,
            Self._mlir_value,
            `,`,
            _get_kgen_string[name](),
            `> : i1`,
        ]

    # ===----------------------------------------------------------------------=== #
    # Target attribute getters
    # ===----------------------------------------------------------------------=== #

    @inline(.nodebug)
    @staticmethod
    def _arch() -> StaticString:
        return StaticString(Self.__arch())

    @inline(.nodebug)
    @staticmethod
    def __arch() -> __mlir_type.`!kgen.string`:
        """Get the target processor string from the compilation target.

        Returns:
            The CPU model for a host target (e.g., "znver3", "neoverse-n1") or
            the GPU architecture for an accelerator target (e.g., "sm_100a",
            "gfx942"). For the host architecture, see `__triple_arch()`.
        """
        return __mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            Self._mlir_value,
            `, "arch" : !kgen.string`,
            `> : !kgen.string`,
        ]

    @staticmethod
    def _is_arch[name: StaticString]() -> Bool:
        """Helper function to check if the target processor is the same as given
        by the name.

        NOTE: This function is needed so that we don't compare the strings at
        compile time using `==`, which would lead to a recursions due to SIMD
        (and potentially many other things) depending on architecture checks.

        Parameters:
            name: The CPU model or GPU architecture to check against.

        Returns:
            True if the target processor is the same as the given name, False
            otherwise.
        """
        return StringLiteral[Self.__arch()]()._is_identical(
            StringLiteral[_get_kgen_string[name]()]()
        )

    @inline(.nodebug)
    @staticmethod
    def __triple_arch() -> __mlir_type.`!kgen.string`:
        """Get the architecture of the compilation target's triple.

        Returns:
            The canonical LLVM architecture name (e.g., "x86_64", "aarch64").
            Equivalent spellings are canonicalized, so an `arm64-apple-darwin`
            triple reports "aarch64" and an `i686-*` triple reports "i386".
        """
        return __mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            Self._mlir_value,
            `, "triple_arch" : !kgen.string`,
            `> : !kgen.string`,
        ]

    @inline(.nodebug)
    @staticmethod
    def __triple() -> __mlir_type.`!kgen.string`:
        return __mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            Self._mlir_value,
            `, "triple" : !kgen.string`,
            `> : !kgen.string`,
        ]

    @staticmethod
    def _is_triple_arch[name: StaticString]() -> Bool:
        """Helper function to check if the target's architecture is the same as
        given by the name.

        NOTE: As with `_is_arch`, the comparison happens in the parameter
        expression rather than via `==` on strings, which would recurse through
        SIMD.

        Parameters:
            name: The canonical LLVM architecture name to check against.

        Returns:
            True if the target's architecture is the same as the given name,
            False otherwise.
        """
        return StringLiteral[Self.__triple_arch()]()._is_identical(
            StringLiteral[_get_kgen_string[name]()]()
        )

    @staticmethod
    @staticmethod
    def _os() -> StaticString:
        var res = __mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            Self._mlir_value,
            `, "os" : !kgen.string`,
            `> : !kgen.string`,
        ]
        return StaticString(res)

    # ===----------------------------------------------------------------------=== #
    # Plugin attribute access
    # ===----------------------------------------------------------------------=== #

    comptime stdlib_plugin: PluginHooks = DefaultPlugin.HookFor[Self()]
    """The stdlib plugin to use for this target."""

    # ===----------------------------------------------------------------------=== #
    # Predicates
    # ===----------------------------------------------------------------------=== #

    @inline(.nodebug)
    @staticmethod
    def default_compile_options() -> StaticString:
        """Returns the default compile options for the compilation target.

        Returns:
            The string of default compile options for the compilation target.
        """

        comptime if Self.is_triple["nvptx64-nvidia-cuda"]():
            # TODO: use `is_nvidia_gpu` when moved to into this struct.
            return "target-abi=shortptr"
        else:
            return ""

    # Features

    @staticmethod
    def has_sse4() -> Bool:
        """Checks if the target supports SSE4 instructions.

        Returns:
            True if the target supports SSE4, False otherwise.
        """
        return Self._has_feature["sse4.1"]()

    @staticmethod
    def has_avx() -> Bool:
        """Returns True if the host system has AVX, otherwise returns False.

        Returns:
            True if the host system has AVX, otherwise returns False.
        """
        return Self._has_feature["avx"]()

    @staticmethod
    def has_avx2() -> Bool:
        """Returns True if the host system has AVX2, otherwise returns False.

        Returns:
            True if the host system has AVX2, otherwise returns False.
        """
        return Self._has_feature["avx2"]()

    @staticmethod
    def has_avx512f() -> Bool:
        """Returns True if the host system has AVX512, otherwise returns False.

        Returns:
            True if the host system has AVX512, otherwise returns False.
        """
        return Self._has_feature["avx512f"]()

    @staticmethod
    def has_intel_amx() -> Bool:
        """Returns True if the host system has Intel AMX support, otherwise returns
        False.

        Returns:
            True if the host system has Intel AMX and False otherwise.
        """
        return Self._has_feature["amx-tile"]()

    @staticmethod
    def has_fma() -> Bool:
        """Returns True if the target has FMA (Fused Multiply-Add) support,
        otherwise returns False.

        Returns:
            True if the target has FMA support, otherwise returns False.
        """
        return Self._has_feature["fma"]()

    @staticmethod
    def has_vnni() -> Bool:
        """Returns True if the target has avx512_vnni, otherwise returns False.

        Returns:
            True if the target has avx512_vnni, otherwise returns False.
        """
        return (
            Self._has_feature["avx512vnni"]() or Self._has_feature["avxvnni"]()
        )

    @staticmethod
    def has_neon() -> Bool:
        """Returns True if the target has Neon support, otherwise returns
        False.

        Returns:
            True if the target support the Neon instruction set.
        """
        return Self._has_feature["neon"]() or Self.is_apple_silicon()

    @staticmethod
    def has_neon_int8_dotprod() -> Bool:
        """Returns True if the target has the Neon int8 dot product extension,
        otherwise returns False.

        Returns:
            True if the target support the Neon int8 dot product extension and
            False otherwise.
        """
        return Self.has_neon() and Self._has_feature["dotprod"]()

    @staticmethod
    def has_neon_int8_matmul() -> Bool:
        """Returns True if the target has the Neon int8 matrix multiplication
        extension (I8MM), otherwise returns False.

        Returns:
            True if the target support the Neon int8 matrix multiplication
            extension (I8MM) and False otherwise.
        """
        return Self.has_neon() and Self._has_feature["i8mm"]()

    @staticmethod
    def has_riscv_extension[name: StaticString]() -> Bool:
        """Returns True if the target implements a RISC-V ISA extension.

        RISC-V has too many extensions to give each one a predicate, so this
        takes the extension name directly. Use the lowercase name LLVM gives the
        extension, which is the ISA-string spelling without the `rv32`/`rv64`
        prefix and without any version: `"m"`, `"a"`, `"f"`, `"d"`, `"c"`,
        `"v"`, `"zba"`, `"zicsr"`, and so on. `"g"` is not an extension; query
        its members individually.

        An extension implied by another one counts as present, so a target built
        with `d` also reports `f`, and one built with `a` also reports `zaamo`
        and `zalrsc`.

        Parameters:
            name: The extension to check for.

        Returns:
            True if the target is RISC-V and implements the extension, False
            otherwise. Always False on a non-RISC-V target.

        Example:

        ```mojo
        from std.sys import CompilationTarget

        def triple(x: Int32) -> Int32:
            comptime if CompilationTarget.has_riscv_extension["m"]():
                return x * 3
            return (x << 1) + x
        ```
        """
        comptime assert name.islower(), String(
            "RISC-V extension names are lowercase, got: ", name
        )
        return Self.is_riscv() and Self._has_feature[name]()

    # Platforms

    @staticmethod
    def is_x86() -> Bool:
        """Checks if the target is an x86 architecture.

        This is an architecture check, not an instruction-set check: use
        `has_sse4()`, `has_avx2()`, and friends to gate code on a specific
        extension.

        Returns:
            True if the target is 32- or 64-bit x86, False otherwise.
        """
        return (
            Self._is_triple_arch["x86_64"]() or Self._is_triple_arch["i386"]()
        )

    @staticmethod
    def is_arm() -> Bool:
        """Checks if the target is an ARM architecture.

        This is an architecture check, not an instruction-set check: use
        `has_neon()` and friends to gate code on a specific extension.

        Returns:
            True if the target is 32- or 64-bit ARM, False otherwise.
        """
        return (
            Self._is_triple_arch["aarch64"]()
            or Self._is_triple_arch["aarch64_be"]()
            or Self._is_triple_arch["aarch64_32"]()
            or Self._is_triple_arch["arm"]()
            or Self._is_triple_arch["armeb"]()
            or Self._is_triple_arch["thumb"]()
            or Self._is_triple_arch["thumbeb"]()
        )

    @staticmethod
    def is_riscv() -> Bool:
        """Checks if the target is a RISC-V architecture.

        This is an architecture check, not an instruction-set check: use
        `has_riscv_extension()` to gate code on a specific extension.

        Returns:
            True if the target is 32- or 64-bit RISC-V, False otherwise.
        """
        return Self.is_rv32() or Self.is_rv64()

    @staticmethod
    def is_rv32() -> Bool:
        """Checks if the target is a 32-bit RISC-V architecture.

        Returns:
            True if the target is 32-bit RISC-V, False otherwise.
        """
        return (
            Self._is_triple_arch["riscv32"]()
            or Self._is_triple_arch["riscv32be"]()
        )

    @staticmethod
    def is_rv64() -> Bool:
        """Checks if the target is a 64-bit RISC-V architecture.

        Returns:
            True if the target is 64-bit RISC-V, False otherwise.
        """
        return (
            Self._is_triple_arch["riscv64"]()
            or Self._is_triple_arch["riscv64be"]()
        )

    @staticmethod
    def is_apple_m1() -> Bool:
        """Check if the target is an Apple M1 system.

        Returns:
            True if the host system is an Apple M1, False otherwise.
        """
        return Self._is_arch["apple-m1"]()

    @staticmethod
    def is_apple_m2() -> Bool:
        """Check if the target is an Apple M2 system.

        Returns:
            True if the host system is an Apple M2, False otherwise.
        """
        return Self._is_arch["apple-m2"]()

    @staticmethod
    def is_apple_m3() -> Bool:
        """Check if the target is an Apple M3 system.

        Returns:
            True if the host system is an Apple M3, False otherwise.
        """
        return Self._is_arch["apple-m3"]()

    @staticmethod
    def is_apple_m4() -> Bool:
        """Check if the target is an Apple M4 system.

        Returns:
            True if the host system is an Apple M4, False otherwise.
        """
        return Self._is_arch["apple-m4"]()

    @staticmethod
    def is_apple_m5() -> Bool:
        """Check if the target is an Apple M5 system.

        Returns:
            True if the host system is an Apple M5, False otherwise.
        """
        return Self._is_arch["apple-m5"]()

    @staticmethod
    def is_apple_silicon() -> Bool:
        """Check if the host system is an Apple Silicon with AMX support.

        Returns:
            True if the host system is an Apple Silicon with AMX support, and
            False otherwise.
        """
        return (
            Self.is_apple_m1()
            or Self.is_apple_m2()
            or Self.is_apple_m3()
            or Self.is_apple_m4()
            or Self.is_apple_m5()
        )

    @staticmethod
    def is_neoverse_n1() -> Bool:
        """Returns True if the host system is a Neoverse N1 system, otherwise
        returns False.

        Returns:
            True if the host system is a Neoverse N1 system and False otherwise.
        """
        return Self._is_arch["neoverse-n1"]()

    # ===----------------------------------------------------------------------=== #
    # Accelerators
    # ===----------------------------------------------------------------------=== #

    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_cpu(self) -> Bool:
        """Returns True if the target is a recognized general-purpose CPU
        architecture (x86, ARM, or RISC-V), and False otherwise.

        This is a closed list rather than `not self.is_accelerator()` to avoid
        accidentally classifying unknown non-accelerator targets as CPUs.
        Targets with an unlisted architecture are neither a CPU nor an
        accelerator.

        Returns:
            True if the target is a known CPU target, and False otherwise.
        """
        # FIXME(MSTDL-3279): Add other host architectures here if they become
        # supported platforms.
        return self.is_x86() or self.is_arm() or self.is_riscv()

    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_accelerator(self) -> Bool:
        """Returns True if the target triple is an accelerator, and False
        otherwise.

        Returns:
            True if the triple target is an accelerator and False otherwise.
        """
        # FIXME(MSTDL-3256):
        #   What about non-GPU accelerators? Non-builtin targets? Support
        #   extending this via plugin hooks.
        return self.is_gpu()

    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_gpu(self) -> Bool:
        """Returns True if the target triple is a GPU and False otherwise.

        Returns:
            True if the triple target is a GPU and False otherwise.
        """
        return self.is_nvidia_gpu() or self.is_amd_gpu() or self.is_apple_gpu()

    # NOTE: Uses a sentinel instead of Optional to avoid recursive elaboration.
    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_nvidia_gpu[subarch: StaticString = _ANY](self) -> Bool:
        """Returns True if the target triple is `nvptx64-nvidia-cuda`,
        and optionally if the target is also the specified sub-architecture, and False otherwise.

        Parameters:
            subarch: The subarchitecture (e.g. sm_80) (defaults to any).

        Returns:
            True if the triple target is cuda and False otherwise.
        """
        comptime is_nvidia = Self.is_triple["nvptx64-nvidia-cuda"]()

        comptime if not StringLiteral[
            _get_kgen_string[subarch]()
        ]()._is_identical(_ANY):
            return is_nvidia and Self._is_arch[subarch]()
        else:
            return is_nvidia

    # NOTE: Uses a sentinel instead of Optional to avoid recursive elaboration.
    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_amd_gpu[subarch: StaticString = _ANY](self) -> Bool:
        """Returns True if the target triple is an AMD GPU triple, and
        optionally if the target is also the specified sub-architecture,
        and False otherwise.

        Parameters:
            subarch: The AMD GPU sub-architecture to check for (e.g., "gfx90a")
                (defaults to any).

        Returns:
            True if the triple target is amdgpu and False otherwise.
        """
        # Matches on the triple's canonical architecture rather than the whole
        # triple: AMD names the subarch in the arch field, so there is one triple
        # spelling per GPU, and LLVM canonicalises all of them -- including the
        # legacy `amdgcn` -- to `amdgpu`.
        comptime is_amd = Self._is_triple_arch["amdgpu"]()

        comptime if not StringLiteral[
            _get_kgen_string[subarch]()
        ]()._is_identical(_ANY):
            return is_amd and Self._is_arch[subarch]()
        else:
            return is_amd

    # NOTE: Uses a sentinel instead of Optional to avoid recursive elaboration.
    # TODO(MOCO-5022): Make these @staticmethod's once side effect bug is fixed
    @inline(.nodebug)
    def is_apple_gpu[subarch: StaticString = _ANY](self) -> Bool:
        """Returns True if the target triple is for Apple GPU (Metal, `air64-apple-macosx`),
        and optionally if the target is alos the specified sub-architecture, and False otherwise.

        Parameters:
            subarch: The subarchitecture (e.g. sm_80) (defaults to any).

        Returns:
            True if the triple target is Apple GPU and False otherwise.
        """
        comptime is_apple = Self.is_triple["air64-apple-macosx"]()

        comptime if not StringLiteral[
            _get_kgen_string[subarch]()
        ]()._is_identical(_ANY):
            return is_apple and Self._is_arch[subarch]()
        else:
            return is_apple

    # ===----------------------------------------------------------------------=== #
    # OS
    # ===----------------------------------------------------------------------=== #

    @staticmethod
    def is_linux() -> Bool:
        """Returns True if the host operating system is Linux.

        Returns:
            True if the host operating system is Linux and False otherwise.
        """
        return Self._os() == "linux"

    @staticmethod
    def is_macos() -> Bool:
        """Returns True if the host operating system is macOS.

        Returns:
            True if the host operating system is macOS and False otherwise.
        """
        return Self._os() in ["darwin", "macosx"]

@inline(.nodebug)
@deprecated("use CompilationTarget.is_triple instead")
def is_triple[
    name: StringLiteral, target: CompilationTarget = CompilationTarget.current()
]() -> Bool:
    """Returns True if the target triple of the compiler matches the input and
    False otherwise.

    Parameters:
      name: The name of the triple value.
      target: The triple value to be checked against.

    Returns:
        True if the triple matches and False otherwise.
    """
    return target.is_triple[name]()


@inline(.nodebug)
def is_apple_m5() -> Bool:
    """Returns True if the target is an Apple M5 GPU and False otherwise.

    Returns:
        True if the target is Apple M5 and False otherwise.
    """
    return is_apple_gpu() and CompilationTarget.is_apple_m5()


@inline(.nodebug)
def is_gpu() -> Bool:
    """Returns True if the target triple is GPU and False otherwise.

    Equivalent to `CompilationTarget.current().is_gpu()`.

    Returns:
        True if the current compilation target is a GPU.
    """
    return CompilationTarget.current().is_gpu()


@inline(.nodebug)
def is_nvidia_gpu[subarch: StaticString = _ANY]() -> Bool:
    """Checks whether the current compilation target is an NVIDIA GPU.

    Equivalent to `CompilationTarget.current().is_nvidia_gpu[subarch]()`.

    Parameters:
        subarch: The GPU sub-architecture to check for.

    Returns:
        True if the current compilation target is an NVIDIA GPU.
    """
    return CompilationTarget.current().is_nvidia_gpu[subarch]()


@inline(.nodebug)
def is_amd_gpu[subarch: StaticString = _ANY]() -> Bool:
    """Checks whether the current compilation target is an AMD GPU.

    Equivalent to `CompilationTarget.current().is_amd_gpu[subarch]()`.

    Parameters:
        subarch: The GPU sub-architecture to check for.

    Returns:
        True if the current compilation target is an AMD GPU.
    """
    return CompilationTarget.current().is_amd_gpu[subarch]()


@inline(.nodebug)
def is_apple_gpu[subarch: StaticString = _ANY]() -> Bool:
    """Checks whether the current compilation target is an Apple GPU.

    Equivalent to `CompilationTarget.current().is_apple_gpu[subarch]()`.

    Parameters:
        subarch: The GPU sub-architecture to check for.

    Returns:
        True if the current compilation target is an Apple GPU.
    """
    return CompilationTarget.current().is_apple_gpu[subarch]()


comptime _AMD_GCN_ARCHS: List[StaticString] = [
    "gfx600",
    "gfx601",
    "gfx602",
    "gfx700",
    "gfx701",
    "gfx702",
    "gfx703",
    "gfx704",
    "gfx705",
    "gfx801",
    "gfx802",
    "gfx803",
    "gfx805",
    "gfx810",
    "gfx900",
    "gfx902",
    "gfx904",
    "gfx906",
    "gfx909",
]


@inline(.nodebug)
def _is_amd_gcn() -> Bool:
    """Returns True if the target triple of the compiler is an AMD GPU triple
    and we are compiling for any of the GCN (Graphics Core Next) architectures.

    Returns:
        True if GCN and False otherwise.
    """
    comptime for arch in _AMD_GCN_ARCHS:
        comptime if is_amd_gpu[arch]():
            return True
    return False


comptime _AMD_RDNA1_ARCHS: List[StaticString] = [
    "gfx1010",  # Navi 10 (RX 5700 XT/5700)
    "gfx1011",  # Navi 12
    "gfx1012",  # Navi 14 (RX 5500 XT/5500)
    "gfx1013",  # Navi 14
]


@inline(.nodebug)
def _is_amd_rdna1() -> Bool:
    """Returns True if the target triple of the compiler is an AMD GPU triple
    and we are compiling for the any of the Radeon RX 5000 series
    sub-architectures:

        gfx1010: Navi 10 (RX 5700 XT/5700)
        gfx1011: Navi 12
        gfx1012: Navi 14 (RX 5500 XT/5500)
        gfx1013: Navi 14

    Returns:
        True if the RDNA1 and False otherwise.
    """
    comptime for arch in _AMD_RDNA1_ARCHS:
        comptime if is_amd_gpu[arch]():
            return True
    return False


comptime _AMD_RDNA2_ARCHS: List[StaticString] = [
    "gfx1030",  # Navi 21 (RX 6900/6800)
    "gfx1031",  # Navi 22 (RX 6700)
    "gfx1032",  # Navi 23 (RX 6600)
    "gfx1033",  # Navi 24 (Van Gogh)
    "gfx1034",  # Navi 24
    "gfx1035",  # Rembrandt APU
    "gfx1036",  # Raphael APU
]


@inline(.nodebug)
def _is_amd_rdna2() -> Bool:
    """Returns True if the target triple of the compiler is an AMD GPU triple
    and we are compiling for the any of the Radeon RX 6000 series
    sub-architectures:

        gfx1030: Navi 21 (RX 6900/6800)
        gfx1031: Navi 22 (RX 6700)
        gfx1032: Navi 23 (RX 6600)
        gfx1033: Navi 24 (Van Gogh)
        gfx1034: Navi 24
        gfx1035: Rembrandt APU
        gfx1036: Raphael APU

    Returns:
        True if the RDNA2 and False otherwise.
    """
    comptime for arch in _AMD_RDNA2_ARCHS:
        comptime if is_amd_gpu[arch]():
            return True
    return False


comptime _AMD_RDNA3_ARCHS: List[StaticString] = [
    "gfx1100",  # Navi 31
    "gfx1101",  # Navi 32
    "gfx1102",  # Navi 33
    "gfx1103",  # Navi 34
    "gfx1150",  # Navi 41
    "gfx1151",  # Navi 42
    "gfx1152",  # Navi 43
    "gfx1153",  # Navi 44
]


@inline(.nodebug)
def _is_amd_rdna3() -> Bool:
    """Returns True if the target triple of the compiler is an AMD GPU triple
    and we are compiling for the any of the Radeon RX 7000 series
    sub-architectures:

        gfx1100: Navi 31
        gfx1101: Navi 32
        gfx1102: Navi 33
        gfx1103: Navi 34
        gfx1150: Navi 41
        gfx1151: Navi 42
        gfx1152: Navi 43
        gfx1153: Navi 44

    Returns:
        True if the RDNA3 and False otherwise.
    """
    comptime for arch in _AMD_RDNA3_ARCHS:
        comptime if is_amd_gpu[arch]():
            return True
    return False


@inline(.nodebug)
def _is_amd_rdna4() -> Bool:
    return is_amd_gpu["gfx1200"]() or is_amd_gpu["gfx1201"]()


@inline(.nodebug)
def _is_amd_rdna() -> Bool:
    return (
        _is_amd_rdna1() or _is_amd_rdna2() or _is_amd_rdna3() or _is_amd_rdna4()
    )


@inline(.nodebug)
def _is_amd_rdna2_or_earlier() -> Bool:
    """Returns True if the target is GCN, RDNA1, or RDNA2.

    Returns:
        True if the GPU is GCN, RDNA1, or RDNA2, False otherwise.
    """
    return _is_amd_gcn() or _is_amd_rdna1() or _is_amd_rdna2()


@inline(.nodebug)
def _is_amd_mi250x() -> Bool:
    return is_amd_gpu["gfx90a"]()


@inline(.nodebug)
def _is_amd_mi300x() -> Bool:
    return is_amd_gpu["gfx942"]()


@inline(.nodebug)
def _is_amd_mi355x() -> Bool:
    return is_amd_gpu["gfx950"]()


@inline(.nodebug)
def _is_amd_mi455x() -> Bool:
    return is_amd_gpu["gfx1250"]()


@inline(.nodebug)
def _cdna_version() -> Int:
    comptime assert (
        _is_amd_cdna()
    ), "querying the cdna version is only supported on AMD hardware"

    comptime if _is_amd_mi250x():
        return 2
    elif _is_amd_mi300x():
        return 3
    elif _is_amd_mi355x():
        return 4
    else:
        return 5


@inline(.nodebug)
def _cdna_3_or_newer() -> Bool:
    comptime if _is_amd_cdna():
        return _cdna_version() >= 3
    return False


@inline(.nodebug)
def _cdna_4_or_newer() -> Bool:
    comptime if _is_amd_cdna():
        return _cdna_version() >= 4
    return False


@inline(.nodebug)
def _cdna_5_or_newer() -> Bool:
    comptime if _is_amd_cdna():
        return _cdna_version() >= 5
    return False


@inline(.nodebug)
def _is_amd_cdna() -> Bool:
    return (
        _is_amd_mi250x()
        or _is_amd_mi300x()
        or _is_amd_mi355x()
        or _is_amd_mi455x()
    )



@inline(.nodebug)
def is_32bit[target: CompilationTarget = CompilationTarget.current()]() -> Bool:
    """Returns True if the maximum integral value is 32 bit.

    Parameters:
        target: The target architecture.

    Returns:
        True if the maximum integral value is 32 bit, False otherwise.
    """
    return size_of[DType.int, target]() == size_of[DType.int32, target]()


@inline(.nodebug)
def is_64bit[target: CompilationTarget = CompilationTarget.current()]() -> Bool:
    """Returns True if the maximum integral value is 64 bit.

    Parameters:
        target: The target architecture.

    Returns:
        True if the maximum integral value is 64 bit, False otherwise.
    """
    return size_of[DType.int, target]() == size_of[DType.int64, target]()


@inline(.nodebug)
def simd_bit_width[
    target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the vector size (in bits) of the specified target.

    Parameters:
        target: The target architecture.

    Returns:
        The vector size (in bits) of the specified target.
    """
    return Int(
        mlir_value=__mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            target._mlir_value,
            `, "simd_bit_width" : !kgen.string`,
            `> : index`,
        ]
    )


@inline(.nodebug)
def simd_byte_width[
    target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the vector size (in bytes) of the specified target.

    Parameters:
        target: The target architecture.

    Returns:
        The vector size (in bytes) of the host system.
    """
    comptime CHAR_BIT = 8
    return simd_bit_width[target]() // CHAR_BIT


@inline(.nodebug)
def stdlib_plugin[target: _TargetType = _current_target()]() -> StaticString:
    """Returns the stdlib plugin name for the specified target.

    Parameters:
        target: The target architecture.

    Returns:
        The stdlib plugin name for the specified target.
    """
    return StaticString(
        __mlir_attr[
            `#kgen.param.expr<target_get_field,`,
            target,
            `, "stdlib_plugin" : !kgen.string`,
            `> : !kgen.string`,
        ]
    )


@inline(.nodebug)
def size_of[
    type: AnyType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the size of (in bytes) of the type.

    The size includes any padding required by the type's alignment, so it is
    always a multiple of `align_of[type]()` and always matches the stride
    between adjacent elements of an array of the type.

    Parameters:
        type: The type in question.
        target: The target architecture.

    Returns:
        The size of the type in bytes.

    Example:
    ```mojo
    from std.sys.info import size_of
    def main() raises:
        print(
            size_of[UInt8]() == 1,
            size_of[UInt16]() == 2,
            size_of[Int32]() == 4,
            size_of[Float64]() == 8,
            size_of[
                SIMD[.uint8, 4]
            ]() == 4,
        )
    ```
    Note: `align_of` is in same module.
    """
    comptime mlir_type = __mlir_attr[
        `#kgen.param.expr<rebind, #kgen.type<!kgen.param<`,
        type,
        `>> : `,
        AnyType,
        `> : !kgen.type`,
    ]
    return Int(
        SIMDLength(
            mlir_value=__mlir_attr[
                `#kgen.param.expr<get_sizeof, #kgen.type<`,
                mlir_type,
                `> : !kgen.type,`,
                target._mlir_value,
                `> : index`,
            ]
        )
    )


@inline(.nodebug)
def size_of[
    dtype: DType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the size of (in bytes) of the dtype.

    Parameters:
        dtype: The DType in question.
        target: The target architecture.

    Returns:
        The size of the dtype in bytes.
    """
    return Int(
        mlir_value=__mlir_attr[
            `#kgen.param.expr<get_sizeof, #kgen.type<`,
            Scalar[dtype]._mlir_type,
            `> : !kgen.type,`,
            target._mlir_value,
            `> : index`,
        ]
    )


@always_inline("builtin")
def align_of[
    type: AnyType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the align of (in bytes) of the type.

    Parameters:
        type: The type in question.
        target: The target architecture.

    Returns:
        The alignment of the type in bytes.
    """
    comptime mlir_type = __mlir_attr[
        `#kgen.param.expr<rebind, #kgen.type<!kgen.param<`,
        type,
        `>> : `,
        AnyType,
        `> : !kgen.type`,
    ]
    return Int(
        SIMDLength(
            mlir_value=__mlir_attr[
                `#kgen.param.expr<get_alignof, #kgen.type<`,
                +mlir_type,
                `> : !kgen.type,`,
                target._mlir_value,
                `> : index`,
            ]
        )
    )


@inline(.nodebug)
def align_of[
    dtype: DType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the align of (in bytes) of the dtype.

    Parameters:
        dtype: The DType in question.
        target: The target architecture.

    Returns:
        The alignment of the dtype in bytes.
    """
    return Int(
        SIMDLength(
            mlir_value=__mlir_attr[
                `#kgen.param.expr<get_alignof, #kgen.type<`,
                Scalar[dtype]._mlir_type,
                `> : !kgen.type,`,
                target._mlir_value,
                `> : index`,
            ]
        )
    )


@inline(.nodebug)
def bit_width_of[
    type: RegisterPassable,
    target: CompilationTarget = CompilationTarget.current(),
]() -> Int:
    """Returns the size of (in bits) of the type.

    Parameters:
        type: The type in question.
        target: The target architecture.

    Returns:
        The size of the type in bits.
    """
    comptime CHAR_BIT = 8
    return CHAR_BIT * size_of[type, target=target]()


@inline(.nodebug)
def bit_width_of[
    dtype: DType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the size of (in bits) of the dtype.

    Parameters:
        dtype: The type in question.
        target: The target architecture.

    Returns:
        The size of the dtype in bits.
    """
    return bit_width_of[Scalar[dtype]._mlir_type, target=target]()


@inline(.nodebug)
def simd_width_of[
    type: RegisterPassable,
    target: CompilationTarget = CompilationTarget.current(),
]() -> Int:
    """Returns the vector size of the type on the host system.

    Parameters:
        type: The type in question.
        target: The target architecture.

    Returns:
        The vector size of the type on the host system.
    """
    return simd_bit_width[target]() // bit_width_of[type, target]()


@inline(.nodebug)
def simd_width_of[
    dtype: DType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    """Returns the vector size of the type on the host system.

    Parameters:
        dtype: The DType in question.
        target: The target architecture.

    Returns:
        The vector size of the dtype on the host system.
    """
    return simd_width_of[Scalar[dtype]._mlir_type, target]()
