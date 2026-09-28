# A `print` of a whole value keeps the template's facts: a trait-bound `def`
# printing its parameter, a local copied from it, and a `String` parameter,
# and a generic struct's method printing a field of `self`, each derive
# their instances. `print` reads such an argument where it lies and selects
# no callee; each instance proves it `Writable` again at its own type, and
# reads a named place of a nominal type in place.
def echo[T: Writable & ImplicitlyCopyable & Deinitable](x: T, s: String) -> Int:
    var kept = x
    print(x, s)
    print("kept", kept, 3)
    return 1


struct Box[T: Writable & ImplicitlyCopyable & Deinitable](Movable):
    var item: Self.T
    var count: Int

    def __init__(out self, var item: Self.T, count: Int):
        self.item = item
        self.count = count

    def report(self, extra: String) -> Int:
        print("item", self.item, extra)
        return self.count


def main():
    print(echo[Int](7, "a"), echo[String]("s", "b"))
    var a = Box[Int](1, 4)
    var b = Box[String]("x", 9)
    print(a.report("p"), b.report("q"))
