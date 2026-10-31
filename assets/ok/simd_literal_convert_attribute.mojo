# `SIMD`'s declared `IntLiteral` and `FloatLiteral` initializers read
# `#pop.int_literal_convert<v> : T` and `#pop<float_literal_convert<v>> : T`,
# which fold the exact literal into every lane once: an integer lane wraps,
# a float lane rounds once, and a `bool` lane is whether the integer is
# nonzero. The attributes read directly, at a closed and a symbolic dtype,
# fold the same way, and `FloatLiteral` prints through `Float64(self)`.


def int_at[
    dt: DType, v: __mlir_type.`!pop.int_literal`
]() -> Scalar[dt]:
    return Scalar[dt](
        mlir_value=__mlir_attr[
            `#pop.int_literal_convert<`, v, `> : `, Scalar[dt]._mlir_type
        ]
    )


def float_at[
    dt: DType, n: Int, v: __mlir_type.`!pop.float_literal`
]() -> SIMD[dt, n]:
    return SIMD[dt, n](
        mlir_value=__mlir_attr[
            `#pop<float_literal_convert<`, v, `>> : `, SIMD[dt, n]._mlir_type
        ]
    )


def narrowed(x: FloatLiteral) -> Float32:
    return Float32(x)


def default_lane[dt: DType, x: Scalar[dt] = 2.0]() -> Scalar[dt]:
    return x


def main():
    print(
        Float32(
            mlir_value=__mlir_attr[
                `#pop.int_literal_convert<`, (3).value, `> : `, Float32._mlir_type
            ]
        )
    )
    print(int_at[DType.float32, (3).value](), int_at[DType.int8, (300).value]())
    print(int_at[DType.bool, (2).value](), int_at[DType.bool, (0).value]())
    print(float_at[DType.float64, 2, (2.5).value]())
    print(Int(3), UInt(4), Float64(2), Float64(2.5), Int8(300), UInt8(257))
    print(SIMD[DType.int8, 2](200), SIMD[DType.float32, 4](1.5), Int(-3))
    print(Scalar[DType.bool](2), Scalar[DType.int16](7), Float64(-2.5))
    var a = narrowed(1.0000000596046448)
    print(a == Float32(1.0000001192092896), a == Float32(1.0))
    print(default_lane[DType.float64](), default_lane[DType.float32]() == 2.0)
    comptime c = SIMD[DType.float64, 2](1.5)
    comptime u = UInt8(257)
    print(c, u)
    var f = FloatLiteral[(2.5).value]()
    print(f.__float__(), f, repr(f))
