# An overloaded `def` whose parameter is a struct instance over another
# struct (`Pair[Point]`) names one symbol at its declaration and at its
# call: the checker spells the declaration from its typed signature, as the
# call is spelled, beside a generic overload over `Pair[T]`, at run time and
# in a compile-time application. The pin prints 4, 1, 7, 6, and 3.
@fieldwise_init
struct Point(Copyable):
    var x: Int


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable):
    var a: Self.T


def pick[T: Copyable & Deinitable](p: Pair[T]) -> Int:
    return 1


def pick(p: Pair[Point]) -> Int:
    return p.a.x


def pick(n: Int) -> Int:
    return n


def main():
    print(pick(Pair[Point](Point(4))))
    print(pick(Pair[Int](5)))
    print(pick(7))
    comptime j = pick(Pair[Point](Point(6)))
    print(j)
    comptime k = pick(3)
    print(k)
