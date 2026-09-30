# A `print` with keyword arguments keeps the template's facts: a generic
# struct's methods printing a field of `self` with a literal `sep`, a
# literal `end`, a `String` field as `sep`, and a `flush` flag, and a
# trait-bound `def` doing the same over its parameter, each derive their
# instances. `print` types each keyword by its own spelling, which every
# instance sees again at its own types.
def show[T: Writable & ImplicitlyCopyable & Deinitable](x: T, sep: String) -> Int:
    print(x, x, sep=sep)
    print(x, end="|\n")
    print("done", flush=True)
    return 1


struct Holder[W: Writable & ImplicitlyCopyable & Deinitable](Movable):
    var w: Self.W
    var glue: String

    def __init__(out self, var w: Self.W, glue: String):
        self.w = w
        self.glue = glue

    def spaced(self):
        print(self.w, self.w, sep=" - ")

    def ended(self):
        print(self.w, end="")
        print(" <")

    def glued(self):
        print(self.w, 7, self.w, sep=self.glue, end=";\n")

    def flushed(self, now: Bool):
        print("w", self.w, flush=now)


def main():
    print(show[Int](3, ", "), show[String]("s", "+"))
    var a = Holder[Int](4, "/")
    var b = Holder[String]("q", "::")
    a.spaced()
    b.spaced()
    a.ended()
    b.ended()
    a.glued()
    b.glued()
    a.flushed(True)
    b.flushed(False)
