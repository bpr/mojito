# A per-instantiation method clone inherits its checked template's facts when
# the body calls a static method of a generic struct whose signature goes
# beyond plain by-value parameters (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `static_calls`): an availability condition the
# template proved, a `ref` or `mut` parameter keeping the caller's place, a
# read-only variadic pack of values or named places, or binders of its own,
# where each instance calls the per-call clone keyed by its own receiver.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def width(n: Int) -> Int where conforms_to(Self.T, Copyable):
        return n * 7

    @staticmethod
    def peek(ref v: Self.T) -> Pair[Self.T]:
        return Pair[Self.T](v.copy())

    @staticmethod
    def swap(mut a: Self.T, mut b: Self.T):
        var t = a.copy()
        a = b.copy()
        b = t^

    @staticmethod
    def total(*values: Int) -> Int:
        var s = 0
        for v in values:
            s += v
        return s

    @staticmethod
    def count(*values: String) -> Int:
        return len(values)

    @staticmethod
    def show[U: Writable](u: U) -> Int:
        print(u)
        return 1


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var other: Self.T

    def __init__(out self, var item: Self.T, var other: Self.T):
        self.item = item^
        self.other = other^

    def width(self) -> Int:
        return Pair[Self.T].width(2)

    def peeked(self) -> Pair[Self.T]:
        return Pair[Self.T].peek(self.item)

    def swapped(mut self):
        Pair[Self.T].swap(self.item, self.other)

    def local_swap(self, v: Self.T) -> Pair[Self.T]:
        var a = self.item.copy()
        var b = v.copy()
        Pair[Self.T].swap(a, b)
        return Pair[Self.T](a^)

    def total(self) -> Int:
        return Pair[Self.T].total(1, 2, 3)

    def counted(self) -> Int:
        var s = String("a")
        var t = String("b")
        return Pair[Self.T].count(s, t, s)

    def none(self) -> Int:
        return Pair[Self.T].count()

    def shown(self) -> Int:
        return Pair[Self.T].show(5)


def main():
    var ints = Shelf[Int](3, 4)
    var words = Shelf[String]("x", "y")
    print(ints.width(), words.width())
    print(ints.peeked().a, words.peeked().a)
    ints.swapped()
    words.swapped()
    print(ints.item, ints.other, words.item, words.other)
    print(ints.local_swap(9).a, words.local_swap("z").a)
    print(ints.total(), words.total())
    print(ints.counted(), words.counted(), ints.none(), words.none())
    print(ints.shown(), words.shown())
