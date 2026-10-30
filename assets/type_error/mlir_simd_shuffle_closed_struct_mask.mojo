# A `pop.simd.shuffle` mask whose type is closed is verified as written:
# `StaticTuple[SIMDLength, 2]`'s storage is `!pop.array<2, SIMDLength>`,
# whose struct elements are no indices until an instance lowers them.
# expect: op expected mask to be a list of indices
from std.utils.static_tuple import StaticTuple


def gather[
    mask: StaticTuple[SIMDLength, 2]
](a: SIMD[DType.int32, 2]) -> SIMD[DType.int32, 2]:
    var res = __mlir_op.`pop.simd.shuffle`[
        mask=mask._mlir_value,
        _type=SIMD[DType.int32, 2]._mlir_type,
    ](a._mlir_value, a._mlir_value)
    return SIMD[DType.int32, 2](mlir_value=res)


comptime MASK = StaticTuple[SIMDLength, 2](1, 0)


def main():
    print(gather[MASK](SIMD[DType.int32, 2](1, 2)))
