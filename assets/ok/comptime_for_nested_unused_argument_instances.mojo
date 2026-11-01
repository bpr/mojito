# A `def` declared in a `comptime for` loses an argument its body never
# uses, as upstream's `RemoveUnusedParams` treats the lifted function like
# any generator: each unrolled copy's call names the generator without it
# and passes no operand for it, so `looped[1]` and `looped[2]` share one
# `inner(a: Arr[n], y)`, `indexed`'s `inner(a: Arr[i + 1])` is one instance
# for both indexes, a generic `inner[m](a: Arr[m])` one for both `m`, and a
# keyword call loses its keyword. A body reading the loop index keeps one
# per index, and a function value names the full generator beside the
# reduced one its call names.


# A slot over the generator's `n` whose construction asserts nothing: a
# `SIMD[dt, n]` construction checks its width, which keeps `n` read.
@fieldwise_init
struct Lane[n: Int](Copyable, ImplicitlyCopyable, Movable):
    var x: Int


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: Lane[Self.n]


@no_inline
def looped[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner(a: Arr[n], y: Int) -> Int:
            return y + 1

        total += inner(Arr[n](Lane[n](1)), x)
    return total


@no_inline
def indexed(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner(a: Arr[i + 1], y: Int) -> Int:
            return y + 3

        total += inner(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def generic(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner[m: Int](a: Arr[m], y: Int) -> Int:
            return y * 2

        total += inner(Arr[1](Lane[1](1)), x) + inner(
            Arr[2](Lane[2](1)), x
        )
    return total


@no_inline
def keyword[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner(y: Int, a: Arr[n]) -> Int:
            return y + 2

        total += inner(a=Arr[n](Lane[n](1)), y=x)
    return total


@no_inline
def reads_index(x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner(a: Arr[i + 1], y: Int) -> Int:
            return y + i

        total += inner(Arr[i + 1](Lane[i + 1](1)), x)
    return total


@no_inline
def valued[n: Int](x: Int) -> Int:
    var total = 0

    comptime for i in range(2):

        @no_inline
        def inner(a: Arr[n], y: Int) -> Int:
            return y - 1

        var f = inner
        total += f(Arr[n](Lane[n](1)), x) + inner(
            Arr[n](Lane[n](2)), x
        )
    return total


def main():
    print(looped[1](3), looped[2](4))
    print(indexed(5))
    print(generic(3))
    print(keyword[1](3), keyword[2](4))
    print(reads_index(5))
    print(valued[1](3), valued[2](4))
