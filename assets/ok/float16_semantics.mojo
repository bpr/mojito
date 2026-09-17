# `Float16` is a half-precision `SIMD[DType.float16, 1]` lane: every result is
# rounded to binary16 once, so a strided range yields the fused
# `k * step + start` at half precision. Values print through `Float64` so the
# exact binary16 value is visible.
from std.sys import size_of

def main():
    for x in range(Float16(0.5), Float16(2.0), Float16(0.3)):
        print(Float64(x))
    for y in range(Float16(1.0), Float16(0.0), Float16(-0.3)):
        print(Float64(y))

    var a = Float16(0.8)
    var b: Float16 = 0.1
    print(Float64(a), Float64(b))
    print(Float64(a + b), Float64(a - b), Float64(a * a), Float64(a / Float16(3.0)))
    print(Float64(-a))
    var c = Float16(1.25)
    c += Float16(0.5)
    c *= Float16(2.0)
    print(Float64(c))
    print(a == Float16(0.8), a != b, a < b, a >= b, Bool(Float16(0.0)))

    var big: Float64 = 70000.0
    print(Float64(Float16(big)), Float64(Float16(60000.0) + Float16(60000.0)))
    print(Float64(Float16(1.0) / Float16(0.0)), Float64(Float16(0.0) / Float16(0.0)))
    print(Float64(Float16(-0.0)), Float64(Float16(65519.0)))

    print(Float64(Float16(Float64(0.1))), Float64(Float16(Float32(0.1))))
    print(Float64(Float16(Int(7))), Float64(Float16(UInt8(200))))
    print(Float64(Float32(a)), Int(Float16(3.7)), Int(Float16(-3.5)))
    print(Float64(Float16(2.5).__ceil__()), Float64(Float16(2.5).__floor__()))

    print(DType.float16, DType.float16 == DType.float32)
    print(DType.float16.is_floating_point(), DType.float16.is_half_float())
    print(DType.float16.is_signed(), DType.float16.is_integral())
    print(hash(DType.float16) == hash(UInt8(79)))
    print(size_of[Float16](), hash(a), a.to_bits())

    var v = SIMD[DType.float16, 4](1.5, 2.25, 0.1, 3.0)
    print((v * v).cast[DType.float64](), (v / SIMD[DType.float16, 4](3.0)).cast[DType.float64]())
    print(Float64(v.reduce_add()), Float64(v.reduce_mul()))
    print(Float64(v.reduce_min()), Float64(v.reduce_max()))
    print(v.gt(SIMD[DType.float16, 4](2.0)), v.to_bits(), v.cast[DType.int32]())

    var xs = List[Float16]()
    xs.append(Float16(0.1))
    xs.append(a)
    print(Float64(xs[0] + xs[1]))
