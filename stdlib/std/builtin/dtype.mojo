# Upstream's `std/builtin/dtype.mojo`, in part: `_uint_type_of_width`.
# `DType` itself stays a compiler type (R160). The chain's `_uint1`,
# `_uint2`, `_uint4`, `uint128`, and `uint256` arms are omitted until R65
# admits those dtypes; the second assert rejects their widths explicitly.
"""Implements the DType class.

These are Mojo built-ins, so you don't need to import them.
"""


@always_inline("builtin")
def _uint_type_of_width[width: Int]() -> DType:
    # fmt: off
    comptime assert width in (
        1, 2, 4, 8, 16, 32, 64, 128, 256,
    ), "width must be 1, 2, 4, 8, 16, 32, 64, 128, or 256"
    comptime assert width in (
        8, 16, 32, 64,
    ), "a uint DType of width 1, 2, 4, 128, or 256 is not supported yet"
    return (
        DType.uint8 if width == 8 else
        DType.uint16 if width == 16 else
        DType.uint32 if width == 32 else
        DType.uint64
    )
    # fmt: on
