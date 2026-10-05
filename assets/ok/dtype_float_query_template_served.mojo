# A `DType`-keyed `def` that asks `DType`'s floating-point format queries of
# its binder is served by its template: each instance answers the query at
# its own dtype, in a runtime position, an expression, and a `comptime if`,
# as does a generic struct's method asking it of the struct's own binder.
def format_of[dt: DType]() -> Int:
    return (
        DType.mantissa_width[dt]() * 1000000
        + DType.max_exponent[dt]() * 1000
        + DType.exponent_width[dt]() * 10
        + DType.exponent_bias[dt]() // 100
    )


def mantissa_of[dt: DType]() -> Int:
    return DType.mantissa_width[dt]()


def bias_of[dt: DType](x: Scalar[dt]) -> Int:
    return DType.exponent_bias[dt]() + Int(x)


def wide[dt: DType]() -> Bool:
    comptime if DType.mantissa_width[dt]() > 20:
        return True
    else:
        return False


struct Format[dt: DType]:
    var unit: Scalar[Self.dt]

    def __init__(out self, unit: Scalar[Self.dt]):
        self.unit = unit

    def bias(self) -> Int:
        return DType.exponent_bias[Self.dt]()


def main():
    print(
        mantissa_of[DType.float16](),
        mantissa_of[DType.float32](),
        mantissa_of[DType.float64](),
    )
    print(format_of[DType.float16]())
    print(format_of[DType.float32]())
    print(format_of[DType.float64]())
    print(bias_of(Float32(1.0)), bias_of(Float64(2.0)))
    print(wide[DType.float16](), wide[DType.float32](), wide[DType.float64]())
    print(Format(Float32(1.0)).bias(), Format(Float64(1.0)).bias())
