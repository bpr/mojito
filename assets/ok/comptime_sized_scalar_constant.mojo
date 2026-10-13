# A module constant of a one-lane SIMD type (`UInt8`, `Int32`, `UInt64`) reads
# natively as its lane scalar, whether declared, imported, or folded from a call.
from std.os.os import SEEK_END

comptime S: UInt8 = 2
comptime H: UInt64 = 7


def lane(v: SIMD[DType.int32, 4]) -> Int32:
    return v[3]


comptime W = lane(SIMD[DType.int32, 4](1, 2, 3, 4))


def take(x: UInt64) -> UInt64:
    return x + 1


def main():
    print(S)
    print(SEEK_END)
    print(W)
    print(take(H))
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    comptime V = SIMD[DType.int32, 4](5, 6, 7, 8)
    print(v + V)
