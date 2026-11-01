# Upstream's `SIMD.__mod__`, `__divmod__`, `__imod__`, `__rmod__`, and
# `__ceildiv__`: a zero integer divisor yields 0, `%` floors on float lanes,
# and compile-time `%` by zero folds through `__mod__` to 0.
from std.math import ceildiv


def f[n: Int]() -> Int:
    comptime k = n % 0
    return k


def main():
    comptime r = UInt(7) % 0
    print(Int(7) % 0, divmod(Int(7), 0), ceildiv(UInt8(250), UInt8(7)), Int(-7) % 2, Float64(-7.5) % 2.0)
    print(r, f[5]())
    var v = SIMD[DType.int32, 4](7, -7, 7, -7)
    var w = SIMD[DType.int32, 4](2, 2, -2, 0)
    var q, m = divmod(v, w)
    print(v % w, q, m)
    var u = SIMD[DType.uint8, 4](250, 7, 0, 9)
    print(ceildiv(u, SIMD[DType.uint8, 4](7, 7, 7, 0)), u % 4)
    var g = SIMD[DType.float32, 2](7.5, -7.5)
    print(g % 2.0, Float32(7.5) % 2.0, ceildiv(Float64(7.5), 2.0))
    print(divmod(Int(-7), 2), divmod(UInt(7), 0), divmod(7, 0), divmod(7, 2), divmod(-7.5, 2.0))
    var x = Int(10)
    x %= 0
    var y = UInt8(9)
    y %= 4
    print(x, y, 3 % Int(2), 10 % Int(0), ceildiv(Int(-7), 2), ceildiv(7, 2))
    print(Int(-1234567), Int8(-128), SIMD[DType.int16, 2](-305, 7))
