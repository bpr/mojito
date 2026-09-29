# A generic struct's overloaded static keeps, at every instance, the member
# its template ranked with the struct's parameters symbolic, as the pin
# does: `Pair[Self.T].pick(v)` calls `pick(v: Self.T)` even at
# `T = Float64`, where the instance makes both members take a `Float64` and
# has no clone of the family, and in either declaration order;
# `Pair[Self.T].pick(1)` calls `pick(v: Float64)` even at `T = Int`, whose
# own types would rank `pick(v: Self.T)` best.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def pick(v: Float64) -> Int:
        return 2

    @staticmethod
    def pick(v: Self.T) -> Int:
        return 1


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def picked(self) -> Int:
        return Pair[Self.T].pick(self.item)

    def literal(self) -> Int:
        return Pair[Self.T].pick(1)

    def given(self, v: Self.T) -> Int:
        return Pair[Self.T].pick(v)


def main():
    print(Shelf[Float64](2.5).picked(), Shelf[Float64](2.5).given(1.5))
    print(Shelf[Int](2).picked(), Shelf[Int](2).literal(), Shelf[Float64](2.5).literal())
    print(Shelf[String]("x").literal())
