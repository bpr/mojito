# expect: immarg operand has non-immediate parameter
# An `llvm.vector.extract` index no constant folds to: LLVM requires its
# `immarg` operand to be an immediate, which a call's result is not.
from std.sys import llvm_intrinsic


@no_inline
def four() -> Int:
    return 4


def main():
    var v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    print(
        llvm_intrinsic[
            "llvm.vector.extract", SIMD[DType.int32, 4], has_side_effect=False
        ](v, Int64(four()))
    )
