# A `pop.simd.shuffle` mask computed from parameters is lowered by each
# instance before the op's verifier reads it: a `StaticTuple[Int, n]`'s
# elements lower to `!pop.scalar<index>`, which is no list of indices.
# expect: function instantiation of `gather` failed: 'pop.simd.shuffle' op expected mask to be a list of indices
from std.utils.static_tuple import StaticTuple


def gather[
    n: Int, mask: StaticTuple[Int, n]
](a: SIMD[DType.int32, n]) -> SIMD[DType.int32, n]:
    var res = __mlir_op.`pop.simd.shuffle`[
        mask=mask._mlir_value,
        _type=SIMD[DType.int32, n]._mlir_type,
    ](a._mlir_value, a._mlir_value)
    return SIMD[DType.int32, n](mlir_value=res)


comptime MASK = StaticTuple[Int, 2](1, 0)


def main():
    print(gather[mask=MASK](SIMD[DType.int32, 2](1, 2)))
