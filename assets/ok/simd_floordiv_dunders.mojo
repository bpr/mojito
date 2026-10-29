# `//` over a `SIMD`, `Int` and `UInt` included, is `SIMD`'s declared
# `__floordiv__`: a zero integer divisor lane yields 0, an integer `//`
# floors, and `//=` calls `__ifloordiv__`.
@fieldwise_init
struct P(Copyable, Movable):
    var n: Int
    var v: SIMD[DType.int8, 2]


def main():
    var a = Int(7)
    var z = Int(0)
    print(a // z, UInt(7) // UInt(0), Float64(7.5) // 0.0)
    print(Int(-7) // 2, Int(7) // -2, Float64(-7.5) // 2.0)
    var v = SIMD[DType.int32, 4](7, -7, 9, 1)
    var w = SIMD[DType.int32, 4](2, 2, 0, -3)
    print(v // w)
    var fv = SIMD[DType.float32, 2](7.5, -7.5)
    var two = SIMD[DType.float32, 2](2.0, 2.0)
    print(fv // two)
    var x = Int(17)
    x //= 5
    print(x)
    x //= 0
    print(x)
    var p = P(17, SIMD[DType.int8, 2](-7, 9))
    p.n //= 5
    p.v //= SIMD[DType.int8, 2](2, 0)
    print(p.n, p.v)
    var xs = [Int(7), Int(-7)]
    xs[1] //= 2
    xs[0] //= 0
    print(xs[0], xs[1])
    print(7 // Int(2), Int(2).__rfloordiv__(Int(7)), Int32(7).__floordiv__(2))
    print(UInt8(250) // UInt8(7), Int8(-128) // Int8(3))
