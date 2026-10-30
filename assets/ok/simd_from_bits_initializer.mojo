# `SIMD`'s `@staticmethod` `from_bits=` initializer, upstream's body over
# `bitcast` and, for a `bool` vector from `uint8` or `int8` lanes, the
# nonzero mask, at run time, at compile time, and through `type_of(x)`.
def round_trip[dt: DType](x: Scalar[dt]) -> Scalar[dt]:
    var bits = x.to_bits()
    return type_of(x)(from_bits=bits)


def main():
    print(Float32(from_bits=UInt32(1065353216)))
    print(Float64(from_bits=Int64(4611686018427387904)))
    print(SIMD[DType.bool, 2](from_bits=SIMD[DType.uint8, 2](0, 3)))
    print(SIMD[DType.bool, 2](from_bits=SIMD[DType.int8, 2](-1, 0)))
    print(SIMD[DType.int32, 2](from_bits=SIMD[DType.uint32, 2](1, 4294967295)))
    print(
        SIMD[DType.float32, 4](
            from_bits=SIMD[DType.int32, 4](0, 1065353216, 1073741824, -1082130432)
        )
    )
    print(UInt8(from_bits=Int8(-1)))
    comptime c = Float32(from_bits=UInt32(1065353216))
    print(c)
    print(round_trip(Float64(2.5)))
