# expect: vector_extract index must be a constant multiple of the result type's known minimum vector length
# An `llvm.vector.extract` index that is not a multiple of the result's
# width, which LLVM's verifier rejects once the pipeline has folded it.
from std.sys import llvm_intrinsic


def main():
    var v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    print(
        llvm_intrinsic[
            "llvm.vector.extract", SIMD[DType.int32, 4], has_side_effect=False
        ](v, Int64(2))
    )
