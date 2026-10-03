# A module constant applying a layout query shapes a signature as the
# application, not as the size it will fold to: `SIMD[DType.float32, S]`
# over `comptime S = size_of[Pair]()` is `SIMD[DType.float32, size_of[Pair]()]`,
# which a `SIMD[DType.float32, 16]` does not convert to, as at the pin.
# expect: expected SIMD[DType.float32, size_of[Pair]()], found SIMD[DType.float32, 16]
from std.sys import size_of


@fieldwise_init
struct Pair(Copyable, Movable):
    var a: Int
    var b: Bool


comptime S = size_of[Pair]()


def g(x: SIMD[DType.float32, S]) -> Int:
    return Int(x[1])


def main():
    print(g(SIMD[DType.float32, 16](2.0)))
