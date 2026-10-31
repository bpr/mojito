# `SIMD`'s rotations, shifts, and `reversed`, and the two `shuffle`
# overloads taking an `IndexList` mask: upstream's declared members over
# `_rotate_mask`, `_shift_mask`, and `llvm.vector.splice`.
from std.utils import Index, IndexList


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v.rotate_left[1]())
    print(v.rotate_left[-1]())
    print(v.rotate_right[1]())
    print(v.shift_left[1]())
    print(v.shift_right[1]())
    print(v.shift_left[4]())
    print(v.shift_right[0]())
    print(v.reversed())
    comptime rev = Index(3, 2, 1, 0)
    print(v.shuffle[mask=rev]())
    var w = SIMD[DType.int32, 4](5, 6, 7, 8)
    print(v.shuffle[mask=IndexList[4](0, 4, 1, 5)](w))
    print(SIMD[DType.float32, 8](1, 2, 3, 4, 5, 6, 7, 8).rotate_right[3]())
    print(SIMD[DType.uint8, 4](1, 2, 3, 4).reversed())
    print(Scalar[DType.int32](7).rotate_left[0]())
    print(Scalar[DType.int32](7).reversed())
