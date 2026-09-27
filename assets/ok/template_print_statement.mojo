# A `print` statement in a runtime body keeps the template's facts: a
# surviving trait-bound `def`, a value-keyed one, and a generic struct's
# method each derive their instances. `print` selects no callee, and each
# instance proves its arguments `Writable` again at its own types.
def shown[T: ImplicitlyCopyable & Deinitable, n: Int](x: T, base: Int) -> Int:
    var kept = x
    var acc = base * n
    print(n)
    print(acc, base + 1)
    return acc


def plain[T: ImplicitlyCopyable & Deinitable](x: T, v: Int) -> Int:
    var kept = x
    if v > 2:
        print("big", v)
    return v * 2


struct Box[T: ImplicitlyCopyable & Deinitable](Movable):
    var item: Self.T
    var count: Int

    def __init__(out self, var item: Self.T, count: Int):
        self.item = item
        self.count = count

    def report(self, extra: Int) -> Int:
        print("count", self.count, extra)
        return self.count + extra


def main():
    print(shown[Int, 3](7, 2), shown[String, 1]("s", 5))
    print(plain[Int](1, 3), plain[String]("s", 1))
    var a = Box[Int](1, 4)
    var b = Box[String]("x", 9)
    print(a.report(2), b.report(3))
