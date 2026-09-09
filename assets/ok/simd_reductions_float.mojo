# Float reductions fold lanes left to right — the order is observable in
# `reduce_add` — Float32 rounds every step, and `reduce_min`/`reduce_max`
# skip NaN lanes and follow infinities.
def main():
    var f = SIMD[DType.float64, 4](1e16, 1.0, -1e16, 1.0)
    print(f.reduce_add(), f.reduce_mul(), f.reduce_min(), f.reduce_max())
    var g = SIMD[DType.float64, 4](1.0, -1e16, 1e16, 1.0)
    print(g.reduce_add())
    var h = SIMD[DType.float32, 4](16777216.0, 1.0, 1.0, -16777216.0)
    print(h.reduce_add(), h.reduce_mul(), h.reduce_min(), h.reduce_max())
    var k = SIMD[DType.float32, 8](0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8)
    print(k.reduce_add(), k.reduce_mul(), k.reduce_min(), k.reduce_max())
    var zero = SIMD[DType.float64, 4](0.0)
    var nan_first = SIMD[DType.float64, 4](0.0, 3.0, -2.0, 5.0) / SIMD[DType.float64, 4](0.0, 1.0, 1.0, 1.0)
    var nan_mid = SIMD[DType.float64, 4](3.0, 0.0, -2.0, 5.0) / SIMD[DType.float64, 4](1.0, 0.0, 1.0, 1.0)
    var nan_last = SIMD[DType.float64, 4](3.0, -2.0, 5.0, 0.0) / SIMD[DType.float64, 4](1.0, 1.0, 1.0, 0.0)
    print(nan_first, nan_first.reduce_min(), nan_first.reduce_max(), nan_first.reduce_add())
    print(nan_mid.reduce_min(), nan_mid.reduce_max(), nan_last.reduce_min(), nan_last.reduce_max())
    var all_nan = zero / zero
    print(all_nan.reduce_min(), all_nan.reduce_max(), all_nan.reduce_mul())
    var inf = SIMD[DType.float64, 4](1.0, -1.0, 2.0, 3.0) / SIMD[DType.float64, 4](0.0, 0.0, 1.0, 1.0)
    print(inf.reduce_min(), inf.reduce_max(), inf.reduce_add(), inf.reduce_mul())
    var nan32 = SIMD[DType.float32, 4](0.0, 1.5, -1.5, 2.0) / SIMD[DType.float32, 4](0.0, 1.0, 1.0, 1.0)
    print(nan32.reduce_min(), nan32.reduce_max(), nan32.reduce_add())
    var wide = SIMD[DType.float64, 16](0.1)
    print(wide.reduce_add(), wide.reduce_mul(), wide.reduce_max())
