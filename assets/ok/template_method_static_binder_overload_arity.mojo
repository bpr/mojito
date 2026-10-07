# A generic struct's overloaded method with a binder of its own on one
# member (`pick[U: Writable](u: U)`) and a two-argument sibling
# (`pick(u: Int, v: Int)`), called from a generic method on a spelled
# receiver, calls the member each call selects at every instance, as the pin
# does: the template serves the calling method, and the two-argument call
# keeps its own member rather than the generic one. A static on
# `Pair[Self.T]` and an instance method on a `Box[Self.T]` local behave
# alike.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def pick[U: Writable](u: U) -> Int:
        return 1

    @staticmethod
    def pick(u: Int, v: Int) -> Int:
        return 2


@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    def pick[U: Writable](self, u: U) -> Int:
        return 10

    def pick(self, u: Int, v: Int) -> Int:
        return 20


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def picked(self) -> Int:
        return Pair[Self.T].pick(1) + Pair[Self.T].pick(1, 2)

    def boxed(self) -> Int:
        var box = Box[Self.T](self.item.copy())
        return box.pick(1) + box.pick(1, 2)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.picked(), ints.boxed())
    print(words.picked(), words.boxed())
