# A per-instantiation method clone inherits its checked template's facts when
# the body calls an overloaded static method of a generic struct on its type
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `static_calls`). The members differ only in closed parameter types, so every
# instance ranks the member the template ranked: on a spelled receiver
# (`Pair[Self.T].pick(v, 1)`) the instance calls its own clone of that member,
# and on an inferred or contextual one the erased member itself.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T
    var b: Self.T

    @staticmethod
    def pick(v: Self.T, n: Int) -> Pair[Self.T]:
        return Pair[Self.T](v.copy(), v.copy())

    @staticmethod
    def pick(v: Self.T, f: Float64) -> Pair[Self.T]:
        var w = v.copy()
        return Pair[Self.T](w^, v.copy())

    @staticmethod
    def count(n: Int) -> Int:
        return n + 1

    @staticmethod
    def count(f: Float64) -> Int:
        return Int(f) + 2

    @staticmethod
    def check(n: Int) raises -> Int:
        if n < 0:
            raise Error("negative")
        return n

    @staticmethod
    def check(f: Float64) raises -> Int:
        return Int(f)


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def by_int(self) -> Pair[Self.T]:
        return Pair[Self.T].pick(self.item, 1)

    def by_float(self) -> Pair[Self.T]:
        return Pair[Self.T].pick(self.item, 1.5)

    def counted(self, n: Int) -> Int:
        return Pair[Self.T].count(n) + Pair[Self.T].count(2.5)

    def checked(self, n: Int) raises -> Int:
        return Pair[Self.T].check(n) + Pair[Self.T].check(1.5)

    def inferred(self) -> Pair[Self.T]:
        return Pair.pick(self.item, 2)

    def contextual(self) -> Pair[Self.T]:
        return .pick(self.item, 2.5)


def main() raises:
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.by_int().a, words.by_int().b)
    print(ints.by_float().a, words.by_float().b)
    print(ints.counted(1), words.counted(2))
    print(ints.checked(4), words.checked(5))
    print(ints.inferred().a, words.inferred().b)
    print(ints.contextual().a, words.contextual().b)
