# `abs` is upstream's `std.math.abs[T: Absable]`, which calls the argument's
# `__abs__`: a scalar alias, a vector, and a scalar over a generic `dtype`
# each bind `T` as at the pin.


def magnitude[dtype: DType](x: Scalar[dtype]) -> Scalar[dtype]:
    return abs(x)


def main():
    var i: Int32 = -3
    print(abs(i))
    print(abs(Float32(-2.5)))
    print(abs(UInt8(5)))
    print(abs(Int8(-128)))
    print(abs(SIMD[DType.int16, 4](1, -2, 3, -4)))
    print(magnitude(Float64(-0.25)))
    print(magnitude[DType.int64](-11))
