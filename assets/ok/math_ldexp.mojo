# `std.math`'s `ldexp` over `_ldexp_impl`: a scalar, a four-lane `float32`
# vector, the 16- and 32-lane `float32` widths an AVX-512 host computes
# through `scalef`, `float16`, and a compile-time call.
from std.math import ldexp


def main():
    print(ldexp(Float64(1.5), Int32(3)))
    print(
        ldexp(
            SIMD[DType.float32, 4](1, 2, 3, 0.5),
            SIMD[DType.int32, 4](1, -1, 4, 10),
        ).cast[DType.float64]()
    )
    print(
        ldexp(SIMD[DType.float32, 16](1), SIMD[DType.int32, 16](2)).cast[
            DType.float64
        ]()
    )
    print(
        ldexp(SIMD[DType.float32, 32](0.75), SIMD[DType.int32, 32](-2))
        .reduce_add()
        .cast[DType.float64]()
    )
    print(ldexp(SIMD[DType.float16, 2](1, 3), SIMD[DType.int32, 2](2, -1)))
    comptime c = ldexp(Float64(3), Int32(-2))
    print(c)
