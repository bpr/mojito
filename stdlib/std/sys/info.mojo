# Upstream's `std/sys/info.mojo`, in part: `_TargetType`,
# `_current_target`, `_empty_target`, the `CompilationTarget` struct with
# `current`, `_unknown`, `_has_feature`, and its x86 feature queries,
# `simd_bit_width`, `simd_byte_width`, and `bit_width_of`, verbatim, and
# the home of `size_of`, which stays the compiler's layout query
# (upstream's body is a `#kgen.param.expr<get_sizeof>` attribute).
# Omitted: `current_accelerator`, `unsupported_target_error`, the
# string-field predicates and the free target functions over them (R1635),
# the Neon and RISC-V queries, which read `is_apple_silicon` (R1635),
# `align_of` and the `target` parameter of `size_of` and `bit_width_of`,
# with `simd_width_of`, `is_32bit`, and `is_64bit` (R1634), and
# `_first_conforming_plugin`, `stdlib_plugin`, and its free function
# (R1672).
"""Implements methods for querying the host target info.

You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import CompilationTarget

print(CompilationTarget.is_x86())
```
"""

from std.collections.string.string_span import _get_kgen_string
from std.string import StaticString

comptime _TargetType = __mlir_type.`!kgen.target`


@inline(.nodebug)
def _current_target() -> _TargetType:
    return __mlir_attr.`#kgen.param.expr<current_target> : !kgen.target`


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
def bit_width_of[
    type: RegisterPassable,
]() -> Int:
    """Returns the size of (in bits) of the type.

    Parameters:
        type: The type in question.

    Returns:
        The size of the type in bits.
    """
    comptime CHAR_BIT = 8
    return CHAR_BIT * size_of[type]()


@inline(.nodebug)
def bit_width_of[dtype: DType]() -> Int:
    """Returns the size of (in bits) of the dtype.

    Parameters:
        dtype: The type in question.

    Returns:
        The size of the dtype in bits.
    """
    return bit_width_of[Scalar[dtype]._mlir_type]()
