# `std.math`'s `exp` over `_exp_impl`: `float64` and `float32` scalars and
# vectors, `float16` through the `float32` arm, the clamped extremes, a
# 16-lane `float32` vector an AVX-512 host scales through `scalef`, the
# `exp[T: _Expable]` overload over a user `__exp__`, and a compile-time
# call.
from std.math import exp, ldexp
from std.math.math import _Expable


@fieldwise_init
struct Growth(_Expable, ImplicitlyCopyable):
    var rate: Float64

    def __exp__(self) -> Self:
        return Growth(exp(self.rate))


def main():
    print(exp(Float64(1)), exp(Float64(0)), exp(Float64(-1)))
    print(exp(SIMD[DType.float32, 4](0, 1, 2, -1)).cast[DType.float64]())
    print(exp(SIMD[DType.float64, 2](10, -5)))
    print(exp(SIMD[DType.float16, 2](1, 2)).cast[DType.float64]())
    print(
        exp(Float64(-1000)),
        exp(Float64(1000)),
        exp(Float32(100)).cast[DType.float64](),
    )
    print(exp(SIMD[DType.float32, 16](0.5)).cast[DType.float64]())
    print(exp(Growth(0.5)).rate)
    print(
        ldexp(Float64(1.5), Int32(3)),
        ldexp(
            SIMD[DType.float32, 2](3, 4), SIMD[DType.int32, 2](-1, 2)
        ).cast[DType.float64](),
    )
    comptime folded = exp(Float64(2))
    print(folded)
