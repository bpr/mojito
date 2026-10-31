# `std.math.modf` is upstream's, over `std.simd._modf`: a scalar, a
# negative scalar, a vector, an integral vector, and a compile-time
# binding split as at the pin.

from std.math import modf


def main():
    var r = modf(Float64(2.5))
    print(r[0], r[1])
    var n = modf(Float32(-3.75))
    print(n[0], n[1])
    var v = modf(SIMD[DType.float64, 4](0.25, -1.5, 7.0, 12.125))
    print(v[0], v[1])
    var i = modf(SIMD[DType.int32, 2](4, -9))
    print(i[0], i[1])
    comptime folded = modf(Float64(6.5))
    print(folded[0], folded[1])
