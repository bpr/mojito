# `DType`'s static floating-point format queries answer for each float dtype,
# in a runtime position, a compile-time binding, a `DType`-keyed function, and
# a type argument.
def mantissa_of[dt: DType]() -> Int:
    return DType.mantissa_width[dt]()


def exponent_lanes[dt: DType]():
    print(SIMD[DType.int32, DType.exponent_width[dt]()](3))


def main():
    print(
        DType.mantissa_width[DType.float16](),
        DType.mantissa_width[DType.float32](),
        DType.mantissa_width[DType.float64](),
    )
    print(
        DType.max_exponent[DType.float16](),
        DType.max_exponent[DType.float32](),
        DType.max_exponent[DType.float64](),
    )
    print(
        DType.exponent_width[DType.float16](),
        DType.exponent_width[DType.float32](),
        DType.exponent_width[DType.float64](),
    )
    print(
        DType.exponent_bias[DType.float16](),
        DType.exponent_bias[DType.float32](),
        DType.exponent_bias[DType.float64](),
    )
    comptime bias = DType.exponent_bias[DType.float64]()
    print(bias)
    print(mantissa_of[DType.float32]())
    print(DType.exponent_bias[Float32.dtype]())
    print(SIMD[DType.int32, DType.exponent_width[DType.float32]()](2))
    exponent_lanes[DType.float32]()
