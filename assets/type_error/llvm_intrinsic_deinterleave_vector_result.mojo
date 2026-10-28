# expect: intrinsic return type expected literal non-packed struct with 2 elements
# `llvm.vector.deinterleave2` gives a literal struct of its two halves, so
# a result typed as one vector matches none of its overloads.
from std.sys import llvm_intrinsic


def main():
    var v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    print(
        llvm_intrinsic[
            "llvm.vector.deinterleave2", SIMD[DType.int32, 4], has_side_effect=False
        ](v)
    )
