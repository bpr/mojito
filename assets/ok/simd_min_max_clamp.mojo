# `max` and `min` are upstream's `std.math` overloads: a `SIMD` pair (an
# `Int` or float literal materializes to its default type) or a `Comparable`
# value followed by any number of others, so a mask, a wrapped `UInt8`, a
# `String`, and a three-argument call run, as at the pin. `SIMD.clamp` and
# `std.math.clamp` are upstream's library bodies over them.
from std.math import clamp

comptime k = max(3, 4)

def main():
    var n = 5
    var f = 2.5
    print(max(1, 2), min(1, 2))
    print(max(0, n), min(n, 0), max(n, 7))
    print(max(1.5, f), min(f, 1), max(1.5, 2))
    var v = SIMD[DType.int32, 4](1, 5, -3, 8)
    print(max(v, 2), min(v, SIMD[DType.int32, 4](0)))
    print(v.clamp(0, 4))
    print(max(UInt(3), UInt(9)), min(Float32(1.5), 2))
    print(max(String("a"), String("b")))
    print(max(1, 2, 3), min(n, 3, 9), max(UInt8(1), 2, 3))
    print(min(1), max(String("x")))
    var b = SIMD[DType.bool, 2](True, False)
    print(max(b, SIMD[DType.bool, 2](False, False)))
    comptime j = max(Int(3), 4)
    print(k, j)
    var w = SIMD[DType.float64, 4](1.5, -2.0, 7.25, 0.5)
    print(clamp(w, -1.0, 1.0))
    var u = SIMD[DType.uint8, 2](250, 3)
    print(min(u, 10))
