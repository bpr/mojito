# A generic struct's static with a binder of its own (`both[U: Writable]`),
# called from generic methods both on a spelled receiver,
# `Pair[Self.T].both(1, self.item)`, and on the bare struct name,
# `Pair.both(7, self.item)`, infers `Pair`'s parameter from the caller's
# `Self.T` argument at every instance, as the pin does: the bare call does
# not take the spelled instance's clone (`both$y3:Int`, baking `T`) for a
# per-call clone of its own (baking `U`).


@fieldwise_init
struct Pair[T: Copyable & Deinitable & Writable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def both[U: Writable](u: U, t: Self.T) -> Int:
        print(u, t)
        return 3


struct Shelf[T: Copyable & Deinitable & Writable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def spelled(self) -> Int:
        return Pair[Self.T].both(1, self.item)

    def inferred(self) -> Int:
        return Pair.both(7, self.item)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.spelled(), words.spelled())
    print(ints.inferred(), words.inferred())
