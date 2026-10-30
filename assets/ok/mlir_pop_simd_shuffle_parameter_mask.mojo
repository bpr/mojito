# A `pop.simd.shuffle` mask computed from parameters: upstream's
# `SIMD._shuffle_list` shape, whose mask is the storage of a
# `StaticTuple[SIMDLength, n]` parameter (`mask._mlir_value`), which each
# instance lowers to its lanes and verifies.
from std.utils.static_tuple import StaticTuple


def shuffle_list[
    dt: DType,
    w: SIMDLength,
    n: SIMDLength,
    mask: StaticTuple[SIMDLength, n],
](a: SIMD[dt, w], b: SIMD[dt, w]) -> SIMD[dt, n]:
    var res = __mlir_op.`pop.simd.shuffle`[
        mask=mask._mlir_value,
        _type=SIMD[dt, n]._mlir_type,
    ](a._mlir_value, b._mlir_value)
    return SIMD[dt, n](mlir_value=res)


comptime REVERSE = StaticTuple[SIMDLength, 4](3, 2, 1, 0)
comptime BLEND = StaticTuple[SIMDLength, 4](7, 2, 5, 0)


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = SIMD[DType.int32, 4](5, 6, 7, 8)
    print(shuffle_list[DType.int32, 4, 4, REVERSE](v, w))
    print(shuffle_list[DType.int32, 4, 4, BLEND](v, w))

    comptime ENDS = StaticTuple[SIMDLength, 2](0, 7)
    print(shuffle_list[DType.int32, 4, 2, ENDS](v, w))

    comptime JOINED = StaticTuple[SIMDLength, 8](0, 1, 2, 3, 4, 5, 6, 7)
    print(shuffle_list[DType.int32, 4, 8, JOINED](v, w))

    comptime SWAP = StaticTuple[SIMDLength, 2](1, 2)
    var x = SIMD[DType.float64, 2](1.5, 2.5)
    var y = SIMD[DType.float64, 2](3.5, 4.5)
    print(shuffle_list[DType.float64, 2, 2, SWAP](x, y))

    comptime folded = shuffle_list[DType.int32, 4, 4, BLEND](
        SIMD[DType.int32, 4](1, 2, 3, 4), SIMD[DType.int32, 4](5, 6, 7, 8)
    )
    print(folded)
