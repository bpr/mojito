# An operator over a type parameter or an associated projection is the call
# of the requirement its bounds declare, as any method call through a bound
# is: `a + b` over a user trait declaring `__add__` is `a.__add__(b)`, a
# reflected `2 + a` is `a.__radd__(2)`, and a comparison or `**` through a
# library trait is its requirement's call, also under a `where` clause.


trait Arith:
    def __add__(self, other: Self) -> Self:
        ...

    def __sub__(self, other: Int) -> Self:
        ...

    def __radd__(self, other: Int) -> Self:
        ...

    def __neg__(self) -> Self:
        ...

    def __invert__(self) -> Self:
        ...

    def __mul__(self, other: Self) raises -> Self:
        ...

    def get(self) -> Int:
        ...


@fieldwise_init
struct N(Arith, Comparable, ImplicitlyCopyable):
    var v: Int

    def __add__(self, other: Self) -> Self:
        return N(self.v + other.v)

    def __sub__(self, other: Int) -> Self:
        return N(self.v - other)

    def __radd__(self, other: Int) -> Self:
        return N(other + self.v * 10)

    def __neg__(self) -> Self:
        return N(-self.v)

    def __invert__(self) -> Self:
        return N(~self.v)

    def __mul__(self, other: Self) raises -> Self:
        if other.v == 0:
            raise Error("zero")
        return N(self.v * other.v)

    def get(self) -> Int:
        return self.v

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v


def add2[T: Arith & Deinitable](a: T, b: T) -> T:
    return a + b


def arith[T: Arith & Deinitable](a: T, b: T) raises -> Int:
    var s = a + b
    var d = a - 3
    var r = 2 + a
    var n = -a
    var i = ~b
    var m = a * b
    return (
        s.get() * 100000
        + d.get() * 10000
        + r.get() * 1000
        + n.get() * 100
        + i.get() * 10
        + m.get()
    )


def ascending[T: Comparable & ImplicitlyCopyable & Deinitable](a: T, b: T, c: T) -> Bool:
    return a < b < c


def same[T: Copyable](a: T, b: T) -> Bool where conforms_to(T, Equatable):
    return a == b


def power[T: Powable](a: T, b: T) -> T:
    return a**b


trait Holder:
    comptime Elem: Comparable & ImplicitlyCopyable & Deinitable

    def first(self) -> Self.Elem:
        ...


@fieldwise_init
struct H(Holder):
    comptime Elem = Int
    var a: Int

    def first(self) -> Int:
        return self.a


def bigger[C: Holder](x: C, y: C) -> Bool:
    return x.first() > y.first()


def main() raises:
    print(add2(N(3), N(4)).v)
    print(arith(N(5), N(1)))
    try:
        print(arith(N(5), N(0)))
    except e:
        print(e)
    print(ascending(N(1), N(2), N(3)), ascending(N(1), N(3), N(2)))
    print(ascending(1, 2, 3), ascending(UInt8(3), UInt8(2), UInt8(1)))
    print(same(N(2), N(2)), same(String("a"), String("b")), same(4, 4))
    print(power(2, 10), power(3.0, 2.0))
    print(bigger(H(3), H(2)), bigger(H(1), H(2)))
