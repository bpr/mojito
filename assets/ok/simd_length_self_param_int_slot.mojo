# A struct's `SIMDLength` parameter read as `Self.n` binds an `Int`
# parameter slot as its lane count, as upstream's `SIMD.length` does in
# `IndexList[Self.length]`.
from std.utils import StaticTuple
from std.builtin.simd_length import SIMDLength


struct Lanes[dt: DType, n: SIMDLength]:
    @staticmethod
    def filled() -> Int:
        comptime t = StaticTuple[SIMDLength, Self.n](fill=1)
        return len(t)

    @staticmethod
    def counted() -> Int:
        return count[Self.n]()


def count[width: Int]() -> Int:
    return width


def main():
    print(Lanes[DType.int32, 4].filled())
    print(Lanes[DType.float64, 8].counted())
