# A member's `where` clause over a trivial lifecycle predicate or a value
# comparison decides whether an instance has it: `Cell[Int, 2]` and
# `Cell[Point, 5]` have `bits`, `Cell[String, 3]` does not, and only an `n`
# above 2 has `wide`.
from std.traits import IsTriviallyCopyable


@fieldwise_init
struct Point(Copyable):
    var x: Int
    var y: Int


struct Cell[T: Copyable & Deinitable, n: Int]:
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def __init__(out self, *, twice: Self.T) where IsTriviallyCopyable[Self.T]:
        self.item = twice.copy()

    def bits(self) -> Int where IsTriviallyCopyable[Self.T]:
        return Self.n

    def wide(self) -> Int where Self.n > 2:
        return Self.n * 10

    def tight(self) -> Int where Self.n + 1 == 3:
        return Self.n


def main():
    var a = Cell[Int, 2](4)
    print(a.bits(), a.tight())
    var b = Cell[Point, 5](twice=Point(1, 2))
    print(b.item.y, b.bits(), b.wide())
    var c = Cell[String, 3]("s")
    print(c.item, c.wide())
