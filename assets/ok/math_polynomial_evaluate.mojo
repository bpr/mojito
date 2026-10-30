# `std.math.polynomial`'s `polynomial_evaluate` over compile-time
# coefficient displays: a display symbolic in a generic's dtype, the
# one-coefficient degenerate case, and a four-lane vector.
from std.math.polynomial import polynomial_evaluate


def p[dtype: DType, width: SIMDLength](
    x: SIMD[dtype, width]
) -> SIMD[dtype, width]:
    return polynomial_evaluate[[Scalar[dtype](1.0), 2.0, 3.0]](x)


def main():
    print(p(Float64(2)), p(SIMD[DType.float32, 2](1, 2)))
    print(polynomial_evaluate[[Float32(5.0)]](Float32(3)))
    print(
        polynomial_evaluate[[Float64(1.0), -1.0, 0.5, 0.25]](
            SIMD[DType.float64, 4](0, 1, 2, -1)
        )
    )
