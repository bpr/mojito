# A struct operator is its dunder's call, so a temporary operand the dunder
# reads dies when it returns, the argument before the receiver, as a method
# call's does: on either side, as an operator result, under a prefix
# operator, a reflected dunder, Equatable's derived `!=`, an `@implicit`
# conversion, a chain's two ends, and a comparison through a bound.
struct S(Comparable, Copyable, Equatable):
    var v: Int

    @implicit
    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v

    def __le__(self, other: Self) -> Bool:
        return self.v <= other.v

    def __gt__(self, other: Self) -> Bool:
        return self.v > other.v

    def __ge__(self, other: Self) -> Bool:
        return self.v >= other.v

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __add__(self, other: Self) -> Self:
        return S(self.v + other.v)

    def __radd__(self, other: Int) -> Self:
        return S(self.v + other)

    def __neg__(self) -> Self:
        return S(-self.v)


def mk(v: Int) -> S:
    return S(v)


def f(s: S):
    print("f", s.v)


def g[T: Comparable & Copyable & Deinitable](a: T, b: T) -> Bool:
    return a.copy() < b


def main():
    print(S(1) < S(2))
    print("--")
    var a = S(3)
    print(a < S(4))
    print("--")
    print((S(8) + a).v)
    print("--")
    print(mk(5) < mk(6) < mk(7))
    print("--")
    print(mk(6) < mk(5) < mk(7))
    print("--")
    print((S(1) + S(2)) < S(4))
    print("--")
    f(S(1) + S(2))
    print("--")
    print(-S(6) < S(1))
    print("--")
    print((-S(5)).v)
    print("--")
    print((S(1) + S(2)).v)
    print("--")
    print((1 + S(6)).v)
    print("--")
    print(S(6) != S(7))
    print("--")
    var b = S(1)
    print((b + 5).v)
    print("--")
    print(g(S(1), S(2)))
    print("--")
    print(mk(1) < mk(2) < mk(3) < mk(4))
    print("end")
