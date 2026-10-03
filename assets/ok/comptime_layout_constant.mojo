# A module constant whose initializer is a layout query stays the application
# `size_of[Pair]()` through the check, as at the pin, and the elaborator
# answers it under the compilation's target: a value read is the query
# itself, folded to the constant in concrete MIR, and an expression over the
# constant reads the same answer.
from std.sys import size_of


@fieldwise_init
struct Pair(Copyable, Movable):
    var a: Int
    var b: Bool


comptime S = size_of[Pair]()


def doubled() -> Int:
    return S * 2


def main():
    print(S)
    print(doubled())
    print(S == size_of[Pair]())
