# All six comparisons yield bool masks: signed and unsigned integer lanes
# compare by value, float lanes compare IEEE-wise (NaN is unordered: every
# comparison but `!=` is False), and a scalar operand splats.
def main():
    var i = SIMD[DType.int32, 4](-2, 0, 2, 2147483647)
    var j = SIMD[DType.int32, 4](2, 0, -2, -2147483648)
    print(i < j, i <= j, i > j, i >= j, i == j, i != j)
    var u = SIMD[DType.uint8, 4](255, 0, 128, 127)
    print(u < 128, u <= 128, u > 128, u >= 128, u == 255, u != 0)
    var s = SIMD[DType.int8, 4](-128, -1, 0, 127)
    print(s < 0, s >= 0, s == -1, 0 > s)
    var f = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 2.5)
    var g = SIMD[DType.float64, 4](1.0, 1.0, -0.0, 2.25)
    print(f < g, f <= g, f > g, f >= g, f == g, f != g)
    var zero = SIMD[DType.float64, 4](0.0)
    var nan = zero / zero
    print(nan < f, nan <= f, nan > f, nan >= f, nan == f, nan != f, nan == nan, nan != nan)
    var h = SIMD[DType.float32, 4](0.1, 0.2, 0.3, 1.0)
    print(h < 0.25, h == 0.1, h > SIMD[DType.float32, 4](0.05, 0.25, 0.3, 0.999), h != h)
    var u64 = SIMD[DType.uint64, 2](18446744073709551615, 9223372036854775808)
    print(u64 > 1, u64 < 9223372036854775807, u64 == 18446744073709551615)
    var n = SIMD[DType.int, 2](-1, 1)
    print(n < 0, n == 1, n != n)
