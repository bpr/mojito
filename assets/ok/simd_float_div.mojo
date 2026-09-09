# Float lanes divide in IEEE arithmetic: zero divisors flow through as
# infinities and NaNs, Float32 lanes round each result, and a scalar or
# literal on either side splats.
def main():
    var f = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 6.0)
    var g = SIMD[DType.float64, 4](0.0, 0.0, 0.0, 4.0)
    print(f / g, g / f, f / 2.0, 2.0 / f, f / 3)
    var h = SIMD[DType.float32, 4](1.0, -1.0, 0.0, 1.0)
    print(h / SIMD[DType.float32, 4](0.0, 0.0, 0.0, 3.0), 1.0 / h, h / 3.0)
    var inf = f / g
    print(inf + inf, inf - inf, inf * 0.0, inf / inf, -inf)
    var third: Float64 = 1.0 / 3.0
    print(f / third, third / f)
    var q = SIMD[DType.float32, 2](0.1, 0.2) / SIMD[DType.float32, 2](0.3, 0.7)
    print(q, q.cast[DType.float64](), q * 3.0)
