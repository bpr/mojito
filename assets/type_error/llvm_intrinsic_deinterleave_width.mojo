# expect: the intrinsic signature of 'llvm.vector.deinterleave2' does not match
# Each half of `llvm.vector.deinterleave2`'s result is half its operand's
# width, so halves as wide as the operand match none of its overloads.
from std.sys import llvm_intrinsic, _RegisterPackType


def main():
    var v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    var res = llvm_intrinsic[
        "llvm.vector.deinterleave2",
        _RegisterPackType[SIMD[DType.int32, 8], SIMD[DType.int32, 8]],
        has_side_effect=False,
    ](v)
    print(res[0], res[1])
