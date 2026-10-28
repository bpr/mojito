# expect: LLVM could not constant fold intrinsic: llvm.vector.insert
# The comptime interpreter calls only an LLVM intrinsic LLVM's constant
# folder folds, and it does not fold `llvm.vector.insert`.
from std.sys import llvm_intrinsic


def main():
    comptime v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    comptime w = llvm_intrinsic[
        "llvm.vector.insert", SIMD[DType.int32, 8], has_side_effect=False
    ](v, SIMD[DType.int32, 2](40, 50), Int64(2))
    print(w)
