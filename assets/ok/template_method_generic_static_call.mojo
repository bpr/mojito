# A per-instantiation method clone inherits its checked template's facts when
# the body calls a static method of a generic struct on its type: with the
# struct's parameters spelled (`Counter[Self.T].start(n)`), inferred from the
# arguments (`Pair.keep(local^)`), or through a leading-dot contextual root
# against an expected `Pair[Self.T]` (`.twice(self.item)`)
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `static_calls`). The static is one declaration with no binders of its own,
# so every instance calls it, or its own instance's clone of it, at the
# substituted types.


@fieldwise_init
struct Counter[T: Copyable & Deinitable](Copyable, Movable):
    var n: Int

    @staticmethod
    def start(n: Int) -> Counter[Self.T]:
        return Counter[Self.T](n * 10)

    @staticmethod
    def width() -> Int:
        return 7

    @staticmethod
    def check(n: Int) raises -> Int:
        if n < 0:
            raise Error("negative")
        return n


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T
    var b: Self.T

    @staticmethod
    def twice(v: Self.T) -> Pair[Self.T]:
        return Pair[Self.T](v.copy(), v.copy())

    @staticmethod
    def keep(var v: Self.T) -> Pair[Self.T]:
        var w = v.copy()
        return Pair[Self.T](v^, w^)


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def counter(self, n: Int) -> Counter[Self.T]:
        return Counter[Self.T].start(n + 1)

    def width(self) -> Int:
        return Counter[Self.T].width()

    def checked(self, n: Int) raises -> Int:
        return Counter[Self.T].check(n)

    def spelled(self) -> Pair[Self.T]:
        return Pair[Self.T].twice(self.item)

    def inferred(self) -> Pair[Self.T]:
        return Pair.twice(self.item)

    def contextual(self) -> Pair[Self.T]:
        return .twice(self.item)

    def moved(self) -> Pair[Self.T]:
        var local = self.item.copy()
        var p = Pair.keep(local^)
        return p^


def main() raises:
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.counter(1).n, words.counter(2).n)
    print(ints.width(), words.width())
    print(ints.checked(4), words.checked(5))
    print(ints.spelled().a, words.spelled().b)
    print(ints.inferred().a, words.inferred().b)
    print(ints.contextual().a, words.contextual().b)
    print(ints.moved().b, words.moved().a)
