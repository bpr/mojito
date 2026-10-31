# expect: LLVM could not constant fold intrinsic: llvm.x86.avx512.mask.scalef.ps.512
# The comptime interpreter calls only an LLVM intrinsic LLVM's constant
# folder folds, and it does not fold AVX-512's `scalef`.
from std.sys import llvm_intrinsic


def main():
    comptime c = llvm_intrinsic[
        "llvm.x86.avx512.mask.scalef.ps.512",
        SIMD[DType.float32, 16],
        has_side_effect=False,
    ](
        SIMD[DType.float32, 16](1.5),
        SIMD[DType.float32, 16](2.7),
        SIMD[DType.float32, 16](0),
        Int16(-1),
        Int32(4),
    )
    print(c)
