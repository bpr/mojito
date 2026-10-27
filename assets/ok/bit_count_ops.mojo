# Upstream's `std.bit` counting and reversing functions, each one
# `llvm_intrinsic` call, over scalars and vectors.
from std.bit import bit_reverse, count_leading_zeros, count_trailing_zeros, pop_count


def main():
    print(count_leading_zeros(Int32(1)))
    print(count_leading_zeros(SIMD[DType.uint8, 4](1, 2, 128, 255)))
    print(count_leading_zeros(Int(1)))
    print(count_trailing_zeros(Int64(8)))
    print(count_trailing_zeros(SIMD[DType.int16, 2](1, 1024)))
    print(pop_count(SIMD[DType.int8, 2](3, 7)))
    print(pop_count(UInt(255)))
    print(bit_reverse(Int8(1)))
    print(bit_reverse(SIMD[DType.uint8, 2](1, 6)))
    comptime c = pop_count(Int32(7))
    print(c)
