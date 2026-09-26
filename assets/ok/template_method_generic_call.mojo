# A per-instantiation method clone inherits its checked template's facts when
# the body calls a generic module function on a value of the struct's
# parameter type (`hash(e)`, `hash(self.value)`, as the bundled `Set.__hash__`
# does) (`docs/notes/instantiation-from-template.md`, class MethodBody). The
# call's selection is the template's; each instance re-keys only the
# application.
from std.hashlib import Hasher


@fieldwise_init
struct Point(Copyable, Deinitable, Hashable, Movable):
    var x: Int
    var y: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)


struct Bag[T: Copyable & Deinitable & Hashable](Movable):
    var value: Self.T
    var items: List[Self.T]

    def __init__(out self, var value: Self.T):
        self.items = [value.copy()]
        self.value = value^

    def own(self) -> UInt64:
        return hash(self.value)

    def mixed(self) -> UInt64:
        var h: UInt64 = 0
        h ^= hash(self.value)
        return h

    def total(self) -> UInt64:
        var h: UInt64 = 0
        for e in self.items:
            h ^= hash(e)
        return h


def main():
    var ints = Bag[Int](7)
    var strings = Bag[String](String("a"))
    var points = Bag[Point](Point(1, 2))
    print(ints.own() == hash(7), ints.mixed() == hash(7), ints.total() == hash(7))
    print(strings.own() == hash(String("a")), strings.total() == strings.mixed())
    print(points.own() == hash(Point(1, 2)), points.total() == points.mixed())
    var ints_set: Set[Int] = {1, 2, 3}
    var strings_set: Set[String] = {"x", "y"}
    print(hash(ints_set) == hash(ints_set), hash(strings_set) == hash(strings_set))
