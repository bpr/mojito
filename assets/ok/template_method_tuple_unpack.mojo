# A per-instantiation method clone inherits its checked template's facts when
# its body unpacks a tuple (`docs/notes/instantiation-from-template.md`, class
# MethodBody, feature `tuple_unpacks`). The element reads are synthesized from
# the tuple's type, which each instance substitutes and derives them from
# again: a parameter, a sibling call's result, and a reassignment of declared
# locals, with a discarded `_` element, derive for an `Int` and a user-struct
# instance, and the sibling call's unpack for a `String` instance too. A
# `String` element unpacked from a parameter is `docs/roadmap.md` 3.6.
@fieldwise_init
struct Point(Equatable, ImplicitlyCopyable, Movable, Writable):
    var x: Int
    var y: Int

    def __eq__(self, other: Self) -> Bool:
        return self.x == other.x and self.y == other.y

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Point(", self.x, ", ", self.y, ")")


struct Holder[T: ImplicitlyCopyable & Deinitable & Equatable & Writable](
    Movable
):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def pair(self, count: Int) -> Tuple[Self.T, Int]:
        return (self.value, count)

    def first(self, t: Tuple[Self.T, Int]) -> Self.T:
        var v, n = t
        return v

    def count(self, t: Tuple[Self.T, Int]) -> Int:
        var _, n = t
        return n

    def own_count(self) -> Int:
        var v, n = self.pair(7)
        return n

    def own_first(self) -> Self.T:
        var v, _ = self.pair(3)
        return v

    def swapped(self, t: Tuple[Self.T, Int]) -> Int:
        var v = self.value
        var n = 0
        v, n = t
        if v == self.value:
            return n
        return -n


def main():
    var some_int = Holder[Int](1)
    var some_str = Holder[String](String("s"))
    var some_point = Holder[Point](Point(1, 2))
    print(some_int.first((4, 5)), some_point.first((Point(3, 4), 5)))
    print(some_int.count((4, 5)), some_point.count((Point(3, 4), 8)))
    print(some_int.own_count(), some_str.own_count(), some_point.own_count())
    print(some_int.own_first(), some_str.own_first(), some_point.own_first())
    print(some_int.swapped((1, 9)), some_point.swapped((Point(1, 2), 4)))
    print(some_point.swapped((Point(0, 2), 4)))
