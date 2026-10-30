# A `pop.simd.shuffle` mask computed from parameters is verified by each
# instance: lane 9 is past the two operands' four lanes.
# expect: function instantiation of `gather` failed: 'pop.simd.shuffle' op mask element 9 is out of bounds
from std.utils.static_tuple import StaticTuple


def gather[
    n: SIMDLength, mask: StaticTuple[SIMDLength, n]
](a: SIMD[DType.int32, n]) -> SIMD[DType.int32, n]:
    var res = __mlir_op.`pop.simd.shuffle`[
        mask=mask._mlir_value,
        _type=SIMD[DType.int32, n]._mlir_type,
    ](a._mlir_value, a._mlir_value)
    return SIMD[DType.int32, n](mlir_value=res)


comptime MASK = StaticTuple[SIMDLength, 2](9, 0)


def main():
    print(gather[mask=MASK](SIMD[DType.int32, 2](1, 2)))
