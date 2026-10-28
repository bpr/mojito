# `llvm.vector.deinterleave2`, through upstream's `SIMD.deinterleave` body
# as a free function: the intrinsic's result is upstream's
# `_RegisterPackType`, a register-passable struct over the `!kgen.struct`
# of the two halves, the even lanes and the odd, whose elements `res[0]`
# and `res[1]` read with `kgen.struct.extract`.
from std.sys import llvm_intrinsic, _RegisterPackType


def deinterleave[
    dtype: DType, size: SIMDLength
](v: SIMD[dtype, size]) -> Tuple[
    SIMD[dtype, Int(size) // 2], SIMD[dtype, Int(size) // 2]
]:
    var res = llvm_intrinsic[
        "llvm.vector.deinterleave2",
        _RegisterPackType[
            SIMD[dtype, Int(size) // 2], SIMD[dtype, Int(size) // 2]
        ],
        has_side_effect=False,
    ](v)
    return res[0], res[1]


def main():
    var v = SIMD[DType.int32, 8](0, 1, 2, 3, 4, 5, 6, 7)
    var halves = deinterleave(v)
    print(halves[0], halves[1])
    var f = deinterleave(SIMD[DType.float32, 4](1.5, 2.5, 3.5, 4.5))
    print(f[0], f[1])
    var b = deinterleave(SIMD[DType.bool, 4](True, False, False, True))
    print(b[0], b[1])
    var u = deinterleave(
        SIMD[DType.uint8, 16](0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15)
    )
    print(u[0], u[1])
    var copied = llvm_intrinsic[
        "llvm.vector.deinterleave2",
        _RegisterPackType[SIMD[DType.int32, 4], SIMD[DType.int32, 4]],
        has_side_effect=False,
    ](v)
    var again = copied
    print(again[1], copied[0])
