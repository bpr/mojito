# A call inside a generic struct's method is bound once, while the method is
# checked with `T` symbolic: `pick(self.value)` can only select the generic
# overload, and every instance inherits that choice. The pinned Mojo prints
# 2 for each method call below; re-checking the `Int` instance used to rank
# the set again and pick `pick(x: Int)`. `pick(5)` outside any generic body
# still selects it.
def pick(x: Int) -> Int:
    return 1


def pick[T: Copyable](x: T) -> Int:
    return 2


struct Box[T: Copyable & Deinitable](Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def get(self) -> Int:
        return pick(self.value)

    def each(self) -> Int:
        var local = self.value.copy()
        return pick(local)


def main():
    var ints = Box[Int](3)
    var bools = Box[Bool](True)
    print(ints.get(), ints.each(), bools.get(), bools.each(), pick(5))
