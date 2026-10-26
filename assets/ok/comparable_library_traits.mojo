# `Equatable` and `Comparable` are the bundled library's traits, as upstream
# declares them in `std/builtin/comparable.mojo`: `__ne__` defaults through
# `__eq__`, and `__gt__`/`__le__`/`__ge__` through `__lt__`, for a struct and
# through a bound alike. A struct without `__eq__` derives one comparing its
# fields, and a conditional conformance's defaults are as available as it.


@fieldwise_init
struct P(Equatable, ImplicitlyCopyable):
    var x: Int

    def __eq__(self, other: Self) -> Bool:
        print("eq", self.x, other.x)
        return self.x == other.x


@fieldwise_init
struct S(Comparable, ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, rhs: Self, /) -> Bool:
        print("lt", self.v, rhs.v)
        return self.v < rhs.v


@fieldwise_init
struct Point(Equatable, ImplicitlyCopyable):
    var x: Int
    var y: Int


@fieldwise_init
struct Empty(Equatable, ImplicitlyCopyable):
    pass


@fieldwise_init
struct Box[T: Copyable & Deinitable](
    Copyable, Equatable where conforms_to(T, Equatable)
):
    var v: Self.T

    def __eq__(self, other: Self) -> Bool where conforms_to(Self.T, Equatable):
        return rebind[Int](self.v) == rebind[Int](other.v)


def ne[T: Equatable](a: T, b: T) -> Bool:
    return a != b


def ge[T: Comparable](a: T, b: T) -> Bool:
    return a >= b


def same[T: Equatable](a: T, b: T) -> Bool:
    return a == b


def ordered[T: Comparable](x: T, y: T) -> Bool:
    return x >= y and x != y and x.__lt__(y) and y.__gt__(x)


def main():
    print(P(1) != P(2))
    print(ne(P(3), P(3)))
    print(S(2) >= S(1), S(2) > S(1), S(1) <= S(1))
    print(ge(S(1), S(2)))
    print(ordered(S(1), S(2)))
    print(1 < 2 < 3, P(1) != P(1) != P(2))
    print(True > False, False >= True)
    print(Point(1, 2) == Point(1, 2), Point(1, 2) != Point(1, 3))
    print(same(Point(1, 2), Point(2, 2)), Empty() == Empty())
    print(Box[Int](1) != Box[Int](2), Box[Int](1) != Box[Int](1))
    print(same(String("a"), String("a")), same(1, 1), same(True, False))
    print(ordered(False, True), ordered(1, 2), ordered(2.5, 1.5))
    comptime a = conforms_to(Bool, Comparable)
    comptime b = conforms_to(DType, Comparable)
    comptime c = conforms_to(DType, Equatable)
    comptime d = conforms_to(NoneType, Equatable)
    print(a, b, c, d)
