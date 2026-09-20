# A derived method clone realizes what a clone check decides per instance and
# nothing more. A call of an argument-free method on `self` or on a field
# keeps the template's contract and takes the instance's own clone as its
# target (`Outer.size$y3:Int` calls `Inner.size$y3:Int`), the built-in `len`
# over a field takes the concrete witness, and the struct applications the
# body reaches are recorded per instance, so a derived run requests exactly
# the instances an inferred run does.
@fieldwise_init
struct Inner[T: Copyable & Movable & Deinitable](Copyable):
    var item: Self.T
    var count: Int

    def size(self) -> Int:
        return self.count


struct Outer[T: Copyable & Movable & Deinitable]:
    var inner: Inner[Self.T]
    var items: List[Self.T]

    def __init__(out self, var inner: Inner[Self.T], var items: List[Self.T]):
        self.inner = inner^
        self.items = items^

    def size(self) -> Int:
        return self.inner.size()

    def twice(self) -> Int:
        return self.size() * 2

    def held(self) -> Int:
        return len(self.items)

    def stored(self) -> Int:
        return self.items.__len__() + self.inner.size()


def main():
    var ints: List[Int] = [1, 2, 3]
    var a = Outer(Inner(7, 3), ints^)
    var words: List[String] = [String("x")]
    var b = Outer(Inner(String("y"), 5), words^)
    print(a.size(), a.twice(), a.held(), a.stored())
    print(b.size(), b.twice(), b.held(), b.stored())
