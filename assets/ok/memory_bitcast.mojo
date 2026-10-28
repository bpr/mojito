# `std.memory`'s `bitcast` and `pack_bits` are upstream's library bodies over
# `pop.bitcast`, which reinterprets a value's bits as a type of the same
# total width: lanes pack little-endian, lane 0 lowest, and a `bool` lane is
# one bit.
from std.memory import bitcast, pack_bits


def main():
    print(bitcast[DType.uint8, 4](UInt32(4631)))
    print(bitcast[DType.uint32, 1](SIMD[DType.uint8, 4](23, 18, 0, 0)))
    print(bitcast[DType.float32](UInt32(1065353216)))
    print(bitcast[DType.int16, 2](Float32(-2.0)))
    print(bitcast[DType.uint64, 1](SIMD[DType.float32, 2](1.0, -2.0)))
    print(bitcast[DType.int8](UInt8(255)))
    print(bitcast[DType.uint64](Float64(-0.0)))
    var bits = SIMD[DType.bool, 8](True, True, False, True, False, False, False, False)
    print(pack_bits[DType.uint8](bits))
    print(pack_bits(SIMD[DType.bool, 16](fill=True)))
    var packed = __mlir_op.`pop.bitcast`[_type=SIMD[DType.uint8, 1]._mlir_type](
        bits._mlir_value
    )
    var unpacked = __mlir_op.`pop.bitcast`[_type=SIMD[DType.bool, 8]._mlir_type](
        packed
    )
    print(SIMD[DType.bool, 8](mlir_value=unpacked))
    var pair = SIMD[DType.float32, 2](1.0, -2.0)
    var halves = __mlir_op.`pop.bitcast`[_type=SIMD[DType.int16, 4]._mlir_type](
        pair._mlir_value
    )
    print(SIMD[DType.int16, 4](mlir_value=halves))
