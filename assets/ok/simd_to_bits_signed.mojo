# `to_bits` and `_to_bits_signed` are upstream's library bodies over
# `bitcast`: a `bool` lane reads through `uint8`, the default target is the
# unsigned dtype of the lane's width, and a symbolic lane folds per instance.
def f[dt: DType](x: Scalar[dt]) -> Int:
    return Int(x._to_bits_signed())


def main():
    print(Float64(-0.0)._to_bits_signed())
    print(SIMD[DType.float32, 2](1.0, -2.0)._to_bits_signed())
    print(Int8(-1)._to_bits_signed(), UInt8(200)._to_bits_signed())
    print(
        Scalar[DType.bool](True).to_bits(),
        SIMD[DType.bool, 2](True, False).to_bits[DType.uint16](),
    )
    print(Float16(1.0).to_bits(), Int(-1).to_bits())
    print(f(Float32(-1.0)), f(UInt16(65535)), f(Float16(-0.0)))
