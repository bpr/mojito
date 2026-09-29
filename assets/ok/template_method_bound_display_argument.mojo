# A per-instantiation method clone inherits its checked template's facts when
# the body hands a call through the struct parameter's bound a list, set, or
# dict display, a tuple display, a leading-dot or spelled static call, or a
# construction, by value to a closed parameter type
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `bound_dispatch`). The argument takes its type from the requirement's
# parameter, which every witness declares alike, so an instance builds the
# same temporary the template did, selecting a string literal element's
# conversion again. A display of the struct parameter's own values
# (`item_display`) substitutes its collection type per instance.
from std.collections import Set

@fieldwise_init
struct Point(Copyable, Deinitable, ImplicitlyCopyable):
    var x: Int
    var y: Int

    @staticmethod
    def origin() -> Point:
        return Point(0, 0)


@fieldwise_init
struct Wrap[T: Copyable & Deinitable](Copyable, Deinitable):
    var v: Self.T


trait Totaler:
    def at(self, at: Point, by: Int) -> Int:
        ...

    def pair(self, by: Tuple[Int, Int]) -> Int:
        ...

    def wrapped(self, by: Wrap[Int]) -> Int:
        ...

    def many(self, by: List[Int]) -> Int:
        ...

    def words(self, by: List[String]) -> Int:
        ...

    def distinct(self, by: Set[Int]) -> Int:
        ...

    def table(self, by: Dict[String, Int]) -> Int:
        ...


@fieldwise_init
struct Sum(Copyable, Deinitable, Totaler):
    var base: Int

    def at(self, at: Point, by: Int) -> Int:
        return self.base + at.x + at.y + by

    def pair(self, by: Tuple[Int, Int]) -> Int:
        return self.base + by[0] + by[1]

    def wrapped(self, by: Wrap[Int]) -> Int:
        return self.base + by.v

    def many(self, by: List[Int]) -> Int:
        var s = self.base
        for v in by:
            s += v
        return s

    def words(self, by: List[String]) -> Int:
        return self.base + by[0].byte_length()

    def distinct(self, by: Set[Int]) -> Int:
        return self.base + len(by)

    def table(self, by: Dict[String, Int]) -> Int:
        try:
            return self.base + by["b"]
        except:
            return -1


@fieldwise_init
struct Count(Copyable, Deinitable, Totaler):
    var base: Int

    def at(self, at: Point, by: Int) -> Int:
        return self.base * (at.x + 1) * by

    def pair(self, by: Tuple[Int, Int]) -> Int:
        return self.base * by[0]

    def wrapped(self, by: Wrap[Int]) -> Int:
        return self.base * by.v

    def many(self, by: List[Int]) -> Int:
        return self.base * len(by)

    def words(self, by: List[String]) -> Int:
        return self.base * len(by)

    def distinct(self, by: Set[Int]) -> Int:
        return self.base * len(by)

    def table(self, by: Dict[String, Int]) -> Int:
        return self.base * len(by)


struct Holder[T: Totaler & Copyable & Deinitable](Movable):
    var item: Self.T
    var n: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.n = 7

    def leading_dot(self) -> Int:
        return self.item.at(.origin(), 3)

    def spelled_static(self) -> Int:
        return self.item.at(Point.origin(), 4)

    def constructed(self) -> Int:
        return self.item.at(Point(1, 2), 5)

    def tuple_display(self) -> Int:
        return self.item.pair((1, 2))

    def applied(self) -> Int:
        return self.item.wrapped(Wrap[Int](3))

    def mixed_display(self) -> Int:
        var k = 4
        return self.item.many([self.n, k, 1])

    def empty_display(self) -> Int:
        return self.item.many([])

    def word_display(self) -> Int:
        return self.item.words(["abc", "de"])

    def set_display(self) -> Int:
        return self.item.distinct({1, 2, 2, self.n})

    def dict_display(self) -> Int:
        return self.item.table({"a": 1, "b": self.n})

    def item_display(self) -> Int:
        var items: List[Self.T] = [self.item.copy(), self.item.copy()]
        return items[1].many([len(items)])


def main():
    var s = Holder[Sum](Sum(10))
    var c = Holder[Count](Count(2))
    print(s.tuple_display(), s.applied(), s.mixed_display(), s.empty_display())
    print(c.tuple_display(), c.applied(), c.mixed_display(), c.empty_display())
    print(s.word_display(), s.set_display(), s.dict_display(), s.item_display())
    print(c.word_display(), c.set_display(), c.dict_display(), c.item_display())
    print(s.leading_dot(), s.spelled_static(), s.constructed())
    print(c.leading_dot(), c.spelled_static(), c.constructed())
