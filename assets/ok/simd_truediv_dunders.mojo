# `/` over a `SIMD`, `Int` and `UInt` included, is `SIMD`'s declared
# `__truediv__`, upstream's `pop.div`: integer lanes truncate toward zero
# and wrap `MIN / -1`, float lanes divide, and only a literal pair divides
# exactly into a `FloatLiteral`. A literal left operand calls the right
# one's `__rtruediv__`, and `/=` calls `__itruediv__`.
@fieldwise_init
struct P(Copyable, Movable):
    var n: Int
    var v: SIMD[DType.int8, 2]


def main():
    var a = Int(7)
    var b = Int(-2)
    print(a / b, -a / 2, a / 2, 1 / a, 7 / a, 7 / 2)
    var u = UInt(7)
    print(u / UInt(2), UInt(18446744073709551615) / u)
    print(Int8(-128) / Int8(-1), Int8(-7) / Int8(2), UInt8(255) / UInt8(16))
    var v = SIMD[DType.int32, 4](7, -7, 8, 9)
    print(v / SIMD[DType.int32, 4](2), v / 3, 100 / v)
    print(Float64(7.5) / 2.0, 2.0 / Float64(4), Float64(1) / 8)
    var g = 9
    g /= 2
    var h = Float64(9)
    h /= 2
    var p = P(-9, SIMD[DType.int8, 2](9, -9))
    p.n /= 2
    p.v /= SIMD[DType.int8, 2](2)
    var xs = [10, -10, 3]
    xs[1] /= 3
    var w = SIMD[DType.int16, 4](10, 20, 30, 40)
    w[1] /= 3
    print(g, h, p.n, p.v, xs[1], w)
