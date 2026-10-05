# A module constant whose initializer is a layout query stays the application
# `size_of[Pair]()` through the check, as at the pin, and the elaborator
# answers it under the compilation's target: a value read is the query
# itself, folded to the constant in concrete MIR, and an expression over the
# constant reads the same answer.
# requires: discovery
from std.sys import size_of


@fieldwise_init
struct Pair(Copyable, Movable):
    var a: Int
    var b: Bool


comptime S = size_of[Pair]()


def doubled() -> Int:
    return S * 2


def at_layout_width[T: AnyType](x: SIMD[DType.float32, size_of[T]()]) -> Int:
    return Int(x[1])


def filled[T: AnyType]() -> SIMD[DType.float32, size_of[T]()]:
    return SIMD[DType.float32, size_of[T]()](5.0)


def main():
    print(S)
    print(doubled())
    print(S == size_of[Pair]())
    var v = SIMD[DType.float32, size_of[Pair]()](3.0)
    print(at_layout_width[Pair](v))
    print(at_layout_width[Pair](SIMD[DType.float32, S](4.0)))
    print(at_layout_width[Pair](filled[Pair]()))
