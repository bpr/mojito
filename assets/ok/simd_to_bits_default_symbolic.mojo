# `to_bits()` with its defaulted target over a lane whose dtype is still a
# parameter: the target is the unsigned dtype of the lane's width, spelled as
# a parameter expression the checker folds per instance (`uint16` for an
# `int16` lane, `uint32` for `float32`, `uint8` for `bool`), as upstream's
# `_unsigned_integral_type_of[dtype]()` default does.
# requires: discovery
def bits[dt: DType, w: Int](value: SIMD[dt, w]) -> UInt64:
    return value.to_bits().cast[DType.uint64]().reduce_add()


def raw[dt: DType](value: Scalar[dt]) -> Scalar[DType.uint32]:
    return value.to_bits().cast[DType.uint32]()


def main():
    print(bits[DType.int16, 2](SIMD[DType.int16, 2](1, -1)), bits[DType.float32, 1](1.5))
    print(raw(Int8(-1)), raw(Float16(1.0)), raw(Scalar[DType.bool](True)))
