# A value's struct parameter is a compile-time argument: `x.dtype` and
# `x.length` read the parameters of `type_of(x)`, never the value, so they
# key a `DType` query, a `SIMD` type, a method's parameter list, and a
# plugin hook's application, as at the pin.
from std._plugin import CurrentPlugin


def exponent_bits[dt: DType](x: Scalar[dt]) -> Int:
    return DType.exponent_width[x.dtype]()


def mantissa_bits(y: Float64) -> Int:
    return DType.mantissa_width[y.dtype]()


def zeros_like(x: SIMD) -> type_of(x):
    return SIMD[x.dtype, x.length](0)


def int8_lanes(x: SIMD[DType.float32, _]) -> SIMD[DType.int8, x.length]:
    return SIMD[DType.int8, x.length](1)


def roundtrip[dt: DType](x: Scalar[dt]) -> Scalar[dt]:
    var wide = x.cast[DType.float64]()
    return wide.cast[x.dtype]()


def hooked[width: SIMDLength](base: SIMD[_, width]) -> Bool:
    comptime binary = CurrentPlugin.binary_math_fn["pow", base.dtype, Int(width)]
    comptime unary = CurrentPlugin.unary_math_fn[
        "exp", base.dtype, Int(base.length)
    ]
    comptime if binary or unary:
        return True
    else:
        return False


def main():
    print(exponent_bits(Float32(1.5)))
    print(mantissa_bits(2.0))
    print(zeros_like(SIMD[DType.int16, 2](3, 4)))
    print(int8_lanes(SIMD[DType.float32, 4](1.0)))
    print(roundtrip(Int32(7)))
    print(hooked(SIMD[DType.float32, 2](1.0, 2.0)))
