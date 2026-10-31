# `std.math`'s `log`: `_log_accurate` for `float64` scalars and vectors
# (zero, negative, infinite, subnormal, and large inputs), `_log_base[27]`
# for a four-lane `float32` vector, `float16` through `float32`, a
# compile-time call, and `frexp` of a two-lane `float32` vector.
from std.math import frexp, log


def main():
    print(log(Float64(2)), log(Float64(0)), log(Float64(-1)))
    print(log(Float64(1) / 0), log(Float64(5e-324)), log(Float64(1e300)))
    print(log(SIMD[DType.float64, 2](0.75, 1.5)))
    print(log(SIMD[DType.float32, 4](1, 2, 0.5, 10)).cast[DType.float64]())
    print(log(SIMD[DType.float16, 2](1, 2)).cast[DType.float64]())
    comptime c = log(Float64(2))
    print(c)
    var fr = frexp(SIMD[DType.float32, 2](8, 0.3))
    print(fr[0].cast[DType.float64](), fr[1].cast[DType.float64]())
