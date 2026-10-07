# A generic struct's method body that calls a static method of a generic
# struct with binders of its own, spelled with explicit compile-time
# arguments or chosen from an overload family one of whose members declares
# binders, is served by its template at every instance: each call runs the
# member it selects, keyed by its own receiver. A string literal bound to
# the static's own binder converts to the parameter type that call closes.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def scaled[k: Int](n: Int) -> Int:
        return n * k

    @staticmethod
    def pick[U: Writable](u: U) -> Int:
        print(u)
        return 1

    @staticmethod
    def pick(n: Int, m: Int) -> Int:
        return n + m

    @staticmethod
    def pick(v: Self.T, n: Int, m: Int) -> Int:
        return n * m


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def scaled(self) -> Int:
        return Pair[Self.T].scaled[3](2)

    def twice(self) -> Int:
        return Pair[Self.T].scaled[2](Pair[Self.T].scaled[3](1))

    def picked(self) -> Int:
        return Pair[Self.T].pick(5)

    def applied(self) -> Int:
        return Pair[Self.T].pick[Int](7)

    def worded(self) -> Int:
        return Pair[Self.T].pick("s")

    def spelled(self) -> Int:
        return Pair[Self.T].pick[String]("t")

    def summed(self) -> Int:
        return Pair[Self.T].pick(5, 6)

    def valued(self) -> Int:
        return Pair[Self.T].pick(self.item, 2, 3)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.scaled(), words.scaled())
    print(ints.twice(), words.twice())
    print(ints.picked(), words.picked())
    print(ints.applied(), words.applied())
    print(ints.worded(), words.worded())
    print(ints.spelled(), words.spelled())
    print(ints.summed(), words.summed())
    print(ints.valued(), words.valued())
