# expect: LLVM intrinsic has unknown result type
# The comptime interpreter types no struct result, so it cannot call
# `llvm.vector.deinterleave2`, whose result is a `_RegisterPackType`.
from std.sys import llvm_intrinsic, _RegisterPackType


def evens(v: SIMD[DType.int32, 8]) -> SIMD[DType.int32, 4]:
    var res = llvm_intrinsic[
        "llvm.vector.deinterleave2",
        _RegisterPackType[SIMD[DType.int32, 4], SIMD[DType.int32, 4]],
        has_side_effect=False,
    ](v)
    return res[0]


def main():
    comptime w = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    comptime even = evens(w)
    print(even)
