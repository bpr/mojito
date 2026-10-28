# `SIMD`'s declared `__init__` overloads: the default, the cast from a vector
# of the same width, the `SIMDLength` splat, `from_int=`, and the scalar
# splat, reached through `SIMD[...]`, `Scalar[...]`, and the sized aliases.
# An initializer's own value parameter (`other_dtype`) is solved from its
# argument on a user struct too.


struct Lanes[dt: DType, n: Int](Copyable):
    var count: Int

    def __init__(out self):
        self.count = 0

    def __init__[other: DType, //](out self, x: SIMD[other, Self.n], /):
        self.count = Self.n


def main():
    print(SIMD[DType.int32, 4]())
    print(Int8(Float64(3.9)), UInt8(Float32(200.5)), Int16(Int8(-3)))
    var v = SIMD[DType.float64, 4](1.5, 2.5, 3.5, 4.5)
    print(SIMD[DType.int32, 4](v))
    print(Int32(SIMDLength(3)), SIMD[DType.int, 2](from_int=4))
    var k: SIMD[DType.int64, 2] = Int64(5)
    print(k, Scalar[DType.int16](Int8(-7)))
    var n = 6
    print(UInt64(n * 8), SIMD[DType.float32, 2](n))
    print(Lanes[DType.int32, 4](v).count, Lanes[DType.int8, 2]().count)
