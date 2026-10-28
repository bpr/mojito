# The shuffling LLVM intrinsics upstream's `SIMD` and `math.iota` call,
# through upstream's bodies as free functions: `vector.extract` of
# `slice`, `vector.insert` of `insert`, the splices of `rotate_left` and
# `shift_left`/`shift_right`, `vector.interleave2` of `interleave`, and
# `llvm.stepvector` of `iota`, at run time, where each index reaches LLVM
# as the constant the pipeline folds through the intrinsic's pack, and
# `vector.extract` and `interleave2` at compile time, which the pin's
# comptime interpreter folds.
from std.sys import llvm_intrinsic


def rotate_left[
    dtype: DType, size: SIMDLength, shift: Int
](v: SIMD[dtype, size]) -> SIMD[dtype, size]:
    comptime if shift >= 0:
        return llvm_intrinsic[
            "llvm.vector.splice.left", SIMD[dtype, size], has_side_effect=False
        ](v, v, Int32(shift))
    else:
        return llvm_intrinsic[
            "llvm.vector.splice.right", SIMD[dtype, size], has_side_effect=False
        ](v, v, Int32(-shift))


def shift_left[
    dtype: DType, size: SIMDLength, shift: Int
](v: SIMD[dtype, size]) -> SIMD[dtype, size]:
    return llvm_intrinsic[
        "llvm.vector.splice.left", SIMD[dtype, size], has_side_effect=False
    ](v, SIMD[dtype, size](), Int32(shift))


def shift_right[
    dtype: DType, size: SIMDLength, shift: Int
](v: SIMD[dtype, size]) -> SIMD[dtype, size]:
    return llvm_intrinsic[
        "llvm.vector.splice.right", SIMD[dtype, size], has_side_effect=False
    ](SIMD[dtype, size](), v, Int32(shift))


def interleave[
    dtype: DType, size: SIMDLength
](a: SIMD[dtype, size], b: SIMD[dtype, size]) -> SIMD[dtype, Int(size) * 2]:
    return llvm_intrinsic[
        "llvm.vector.interleave2",
        SIMD[dtype, Int(size) * 2],
        has_side_effect=False,
    ](a, b)


def iota[
    dtype: DType, width: Int
](offset: Scalar[dtype] = Scalar[dtype](0)) -> SIMD[dtype, width]:
    comptime step_dtype = dtype if dtype.is_integral() else DType.int
    var step = llvm_intrinsic[
        "llvm.stepvector", SIMD[step_dtype, width], has_side_effect=False
    ]()
    return step.cast[dtype]() + offset


def slice[
    dtype: DType, size: SIMDLength, output_width: Int, offset: Int
](v: SIMD[dtype, size]) -> SIMD[dtype, output_width]:
    return llvm_intrinsic[
        "llvm.vector.extract",
        SIMD[dtype, output_width],
        has_side_effect=False,
    ](v, Int64(offset))


def insert[
    dtype: DType, size: SIMDLength, input_width: SIMDLength, offset: Int
](v: SIMD[dtype, size], value: SIMD[dtype, input_width]) -> SIMD[dtype, size]:
    return llvm_intrinsic[
        "llvm.vector.insert", SIMD[dtype, size], has_side_effect=False
    ](v, value, Int64(offset))


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(rotate_left[DType.int32, 4, 1](v))
    print(rotate_left[DType.int32, 4, -1](v))
    print(shift_left[DType.int32, 4, 1](v))
    print(shift_right[DType.int32, 4, 2](v))
    var flags = SIMD[DType.bool, 4](True, False, False, True)
    print(rotate_left[DType.bool, 4, 3](flags))
    var f = SIMD[DType.float32, 2](1.5, -2.5)
    print(interleave(f, f * 2))
    print(interleave(v, v + 10))
    print(iota[DType.int32, 4]())
    print(iota[DType.uint8, 8](250))
    print(iota[DType.float64, 4](0.5))
    var wide = iota[DType.uint16, 8]()
    print(slice[DType.uint16, 8, 2, 6](wide))
    print(insert[DType.uint16, 8, 4, 4](wide, SIMD[DType.uint16, 4](9)))

    comptime w = SIMD[DType.int16, 8](0, 1, 2, 3, 4, 5, 6, 7)
    comptime high = slice[DType.int16, 8, 4, 4](w)
    print(high)
    comptime pairs = interleave(SIMD[DType.int8, 2](1, 2), SIMD[DType.int8, 2](3, 4))
    print(pairs)
