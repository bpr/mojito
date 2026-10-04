# A generic struct's `Tuple` field over its parameter reads, writes, and
# indexes as the closed `Tuple` it names at each instance, from `main` and
# from the struct's own methods.
struct Holder[T: ImplicitlyCopyable & Deinitable](Movable):
    var pair: Tuple[Self.T, Int]

    def __init__(out self, var value: Self.T, count: Int):
        self.pair = Tuple[Self.T, Int](value, count)

    def first(self) -> Self.T:
        return self.pair[0]

    def count(self) -> Int:
        return self.pair[1]


struct Keeper[T: ImplicitlyCopyable & Deinitable](Movable):
    var pair: Tuple[Self.T, Bool]

    def __init__(out self, var pair: Tuple[Self.T, Bool]):
        self.pair = pair


def main():
    var some_int = Holder[Int](1, 2)
    print(some_int.pair[1])
    print(some_int.pair[0])
    var some_bool = Holder[Bool](True, 7)
    print(some_bool.pair[0], some_bool.pair[1])
    print(some_bool.first(), some_bool.count())
    some_int.pair = Tuple[Int, Int](5, 6)
    print(some_int.first(), some_int.count())
    var s = Holder[String](String("hi"), 3)
    print(s.pair[0], s.first(), s.count())
    var k = Keeper[Float64](Tuple[Float64, Bool](1.5, False))
    print(k.pair[0], k.pair[1])
