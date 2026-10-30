# A value parameter's type reads the arguments bound before it:
# `mask: StaticTuple[Int, n]` after `n: Int` takes a tuple of that size,
# closed or over a caller's own binder, and `n` is inferred from a later
# argument when the mask is passed by keyword.
from std.utils.static_tuple import StaticTuple


def first[n: Int, mask: StaticTuple[Int, n]]() -> Int:
    return mask[0]


def via[n: Int]() -> Int:
    comptime tup = StaticTuple[Int, n](fill=5)
    return first[n, tup]()


def lane[n: SIMDLength, mask: StaticTuple[SIMDLength, n]]() -> Int:
    return Int(mask[0])


def gather[
    n: SIMDLength, mask: StaticTuple[SIMDLength, n]
](a: SIMD[DType.int32, n]) -> Int:
    return Int(mask[0])


comptime REV = StaticTuple[SIMDLength, 2](1, 0)


def main():
    comptime tup = StaticTuple[Int, 2](3, 0)
    print(first[2, tup]())
    print(via[2]())
    comptime lanes = StaticTuple[SIMDLength, 2](fill=5)
    print(lane[2, lanes]())
    print(gather[mask=REV](SIMD[DType.int32, 2](1, 2)))
