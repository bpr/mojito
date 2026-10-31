# An unannotated float `comptime` binding is a `FloatLiteral`, local or
# module, imported or not: each read materializes at its use, so it meets
# a `Float32` or a vector of any float lane as a literal does.
from std.math.constants import pi

comptime HALF = 0.5
comptime QUARTER = HALF / 2


def scaled[dtype: DType, width: Int](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    comptime two = 2.0
    var y = x.fma(two, 0.25)
    if y[0].lt(two / 4):
        return y
    return y.fma(two / 8, -0.5)


def main():
    comptime a = 1.5
    print(Float32(1) * a, Float32(2) * HALF, Float32(4) * QUARTER)
    var b = a
    print(b, a / 4, a * 3)
    var v = SIMD[DType.float32, 4](2.0) * a
    print(v)
    print(scaled[DType.float32, 2](SIMD[DType.float32, 2](1.0)))
    print(scaled[DType.float64, 1](3.0))
    var f: Float32 = a
    print(f, Float16(1) + a)
    print(Float64(1) * pi > 3.0)
