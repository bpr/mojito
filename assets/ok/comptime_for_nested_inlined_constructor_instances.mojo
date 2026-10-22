# A `def` declared in a `comptime for` loses an argument its body never
# uses even when the argument is built by an inlined constructor: each
# unrolled copy's call drops the operand, and the copy's registers that
# computed it go with it, so `looped` and `keyword` keep one instance each,
# as `comptime_for_nested_unused_argument_instances.mojo` does with a
# fieldwise `Arr`.


struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]

    @always_inline("nodebug")
    def __init__(out self, v: SIMD[DType.int64, Self.n]):
        self.v = v


def looped[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(a: Arr[n], y: Int) -> Int:
            return y + 1

        total += inner(Arr[n](SIMD[DType.int64, n](1)), x)
    return total


def indexed(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(a: Arr[i + 1], y: Int) -> Int:
            return y + 3

        total += inner(Arr[i + 1](SIMD[DType.int64, i + 1](1)), x)
    return total


def generic(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner[m: Int](a: Arr[m], y: Int) -> Int:
            return y * 2

        total += inner(Arr[1](SIMD[DType.int64, 1](1)), x) + inner(
            Arr[2](SIMD[DType.int64, 2](1)), x
        )
    return total


def keyword[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(y: Int, a: Arr[n]) -> Int:
            return y + 2

        total += inner(a=Arr[n](SIMD[DType.int64, n](1)), y=x)
    return total


def reads_index(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(a: Arr[i + 1], y: Int) -> Int:
            return y + i

        total += inner(Arr[i + 1](SIMD[DType.int64, i + 1](1)), x)
    return total


def valued[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        def inner(a: Arr[n], y: Int) -> Int:
            return y - 1

        var f = inner
        total += f(Arr[n](SIMD[DType.int64, n](1)), x) + inner(
            Arr[n](SIMD[DType.int64, n](2)), x
        )
    return total


def main():
    print(looped[1](3), looped[2](4))
    print(indexed(5))
    print(generic(3))
    print(keyword[1](3), keyword[2](4))
    print(reads_index(5))
    print(valued[1](3), valued[2](4))
