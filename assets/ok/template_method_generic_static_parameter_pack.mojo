# A generic struct's static taking a read-only pack of the struct's
# parameter type, called on a spelled receiver from another generic struct's
# method. Each instance of the caller reaches its own instance of the static:
# the pack's element type is part of the static's identity, so `Int` and
# `String` packs of one length do not share a body.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def count(*values: Self.T) -> Int:
        return len(values)

    @staticmethod
    def first(*values: Self.T) -> Self.T:
        return values[0].copy()

    @staticmethod
    def tagged(tag: Int, *values: Self.T) -> Int:
        return tag + len(values)


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def counted(self) -> Int:
        return Pair[Self.T].count(self.item, self.item)

    def head(self) -> Self.T:
        return Pair[Self.T].first(self.item, self.item)

    def tag(self) -> Int:
        return Pair[Self.T].tagged(10, self.item, self.item, self.item)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.counted(), words.counted())
    print(ints.head(), words.head())
    print(ints.tag(), words.tag())
    print(Pair[Int].first(7, 8), Pair[String].first("y", "z"))
