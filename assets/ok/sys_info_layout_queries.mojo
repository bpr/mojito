# `std.sys.info`'s layout queries are upstream's library bodies over KGEN's
# `get_sizeof` and `get_alignof`, which the elaborator answers under the
# compilation's target: `size_of` and `align_of` of a type or a dtype,
# `bit_width_of` with and without its `target`, `simd_width_of`, and
# `is_32bit`/`is_64bit`, at run time, in a `comptime` binding, a
# `comptime if`, and a signature's width.
# requires: discovery
from std.sys import CompilationTarget, size_of, simd_bit_width
from std.sys.info import align_of, bit_width_of, simd_width_of, is_32bit, is_64bit


@fieldwise_init
struct Padded:
    var flag: Bool
    var value: Int


@fieldwise_init
struct Small:
    var a: UInt8
    var b: UInt16


def generic_align[T: AnyType]() -> Int:
    return align_of[T]()


def lanes(x: SIMD[DType.float32, simd_width_of[DType.float32]()]) -> Int:
    return Int(x[0])


def main():
    print(size_of[DType.int32]())
    print(align_of[Float64](), align_of[DType.int16]())
    print(align_of[Padded](), align_of[Small](), size_of[Small]())
    print(align_of[NoneType](), align_of[Bool]())
    print(generic_align[Padded]())
    print(bit_width_of[DType.int16](), bit_width_of[Int, CompilationTarget.current()]())
    print(is_64bit(), is_32bit())
    comptime S = size_of[Padded]()
    comptime A = align_of[Padded]()
    print(S, A)
    comptime if align_of[Int]() == 8:
        print("aligned")
    print(simd_width_of[DType.float32]() == simd_bit_width() // 32)
    print(simd_width_of[Float64]() * 2 == simd_width_of[DType.float32]())
    print(lanes(SIMD[DType.float32, simd_width_of[DType.float32]()](3.0)))
# stdout: 4
# stdout: 8 2
# stdout: 8 2 4
# stdout: 1 1
# stdout: 8
# stdout: 16 64
# stdout: True False
# stdout: 16 8
# stdout: aligned
# stdout: True
# stdout: True
# stdout: 3
