# The `std.math` operation traits are upstream's library traits, bundled in
# `std/math.mojo`: a struct conforms by declaring one, and `abs`, `**`,
# `round`, `divmod`, and the `std.math` rounding helpers reach its
# requirement through a bound. Only `Absable`, `Powable`, and `Roundable`
# are prelude names; the other six need their import. The scalars conform
# as upstream's `SIMD` declares, and answer each requirement as a method.
from std.math import (
    CeilDivable,
    CeilDivableRaising,
    Ceilable,
    DivModable,
    Floorable,
    Truncable,
    ceil,
    ceildiv,
    floor,
    trunc,
)


@fieldwise_init
struct A(Absable, ImplicitlyCopyable, Powable, Roundable, Writable):
    var v: Int

    def __abs__(self) -> Self:
        return A(-self.v if self.v < 0 else self.v)

    def __pow__(self, exp: Self) -> Self:
        return A(self.v**exp.v)

    def __round__(self) -> Self:
        return A(self.v + 100)

    def __round__(self, ndigits: Int) -> Self:
        return A(self.v + ndigits)

    def write_to(self, mut w: Some[Writer]):
        w.write("A(", self.v, ")")


@fieldwise_init
struct B(Ceilable, CeilDivable, DivModable, Floorable, Truncable, Writable):
    var v: Int

    def __divmod__(self, other: Self) -> Tuple[Self, Self]:
        return (B(self.v // other.v), B(self.v % other.v))

    def __ceil__(self) -> Self:
        return B(self.v + 1)

    def __floor__(self) -> Self:
        return B(self.v - 1)

    def __trunc__(self) -> Self:
        return B(self.v * 10)

    def __ceildiv__(self, d: Self) -> Self:
        return B(-((-self.v) // d.v))

    def write_to(self, mut w: Some[Writer]):
        w.write("B(", self.v, ")")


def ab[T: Absable](x: T) -> T:
    return abs(x)


def pw[T: Powable](x: T, y: T) -> T:
    return x**y


def rd[T: Roundable](x: T) -> T:
    return round(x)


def dm[T: DivModable](a: T, b: T) -> Tuple[T, T]:
    return divmod(a, b)


def ab_method[T: Absable](x: T) -> T:
    return x.__abs__()


def dm_method[T: DivModable](a: T, b: T) -> Tuple[T, T]:
    return a.__divmod__(b)


def names[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Absable):
        s += "abs "
    comptime if conforms_to(T, Powable):
        s += "pow "
    comptime if conforms_to(T, Roundable):
        s += "round "
    comptime if conforms_to(T, DivModable):
        s += "divmod "
    comptime if conforms_to(T, Ceilable):
        s += "ceil "
    comptime if conforms_to(T, Floorable):
        s += "floor "
    comptime if conforms_to(T, Truncable):
        s += "trunc "
    comptime if conforms_to(T, CeilDivable):
        s += "ceildiv "
    comptime if conforms_to(T, CeilDivableRaising):
        s += "ceildivr "
    return s


def main():
    comptime a_abs = conforms_to(A, Absable)
    print(ab(A(-3)), a_abs)
    print(pw(A(2), A(5)), rd(A(1)), abs(A(-5)), A(2) ** A(3), round(A(4)))
    var q = dm(B(7), B(2))
    print(q[0], q[1])
    var d = divmod(B(9), B(4))
    print(d[0], d[1])
    print(floor(B(5)), ceil(B(5)), trunc(B(5)), ceildiv(B(7), B(2)))
    print(names[A]())
    print(names[B]())
    print(names[Int]())
    print(names[Float64]())
    print(names[UInt]())
    print(names[Bool]())
    print(names[Tuple[Int, Int]]())
    print(names[String]())
    print(ab(-4), rd(2.5), rd(7), rd(UInt(9)), pw(2, 3), dm(7, 2)[1])
    print(ab_method(-6), dm_method(-7, 2)[0], dm_method(7.5, 2.0)[1])
    var x = 7
    print(
        x.__abs__(),
        x.__pow__(2),
        x.__divmod__(3)[0],
        Float64(2.5).__round__(),
        x.__ceil__(),
        x.__ceildiv__(2),
    )
