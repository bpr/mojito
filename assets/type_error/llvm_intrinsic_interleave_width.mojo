# expect: the intrinsic signature of 'llvm.vector.interleave2' does not match
# `llvm.vector.interleave2` gives twice its operands' width, so a result as
# wide as one operand matches none of its overloads.
from std.sys import llvm_intrinsic


def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(
        llvm_intrinsic[
            "llvm.vector.interleave2", SIMD[DType.int32, 4], has_side_effect=False
        ](a, a)
    )
