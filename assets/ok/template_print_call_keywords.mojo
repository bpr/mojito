# A `print` whose keyword is a call result keeps the template's facts: a
# generic struct's methods printing a field of `self` with a constructed
# `String` as `sep`, a `String` of a field as `end`, and a constructed
# `FileDescriptor` as `file`, and a trait-bound `def` doing the same over
# its parameter, each derive their instances. A keyword's type never
# mentions a parameter, so every instance builds and drops the same
# temporary.
def show[T: Writable & ImplicitlyCopyable & Deinitable](x: T, n: Int) -> Int:
    print(x, x, sep=String("-"))
    print(x, end=String(n) + "\n")
    print("done", file=FileDescriptor(1))
    return 1


struct Holder[W: Writable & ImplicitlyCopyable & Deinitable](Movable):
    var w: Self.W
    var n: Int

    def __init__(out self, var w: Self.W, n: Int):
        self.w = w
        self.n = n

    def spaced(self):
        print(self.w, self.w, sep=String(" = "))

    def ended(self):
        print(self.w, end=String(self.n))
        print(" <")

    def filed(self):
        print("w", self.w, file=FileDescriptor(1), sep=String(":"))


def main():
    print(show[Int](3, 5), show[String]("s", 6))
    var a = Holder[Int](4, 7)
    var b = Holder[String]("q", 8)
    a.spaced()
    b.spaced()
    a.ended()
    b.ended()
    a.filed()
    b.filed()
