# A reference returned through an origin binder keeps the argument it names
# alive. `pick` returns its `ref[o]` parameter as `ref[o]`; the binder is
# inferred from `w` at each call, so the result loans `w` until the referent
# is read, including at `w`'s last use.
# The same holds for a keyword argument, a field argument, a binding
# initialized from the result, and a temporary receiver.
struct Pair:
    var a: String
    var b: String

    def __init__(out self):
        self.a = String("a")
        self.b = String("b")


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var count: Int

    def __init__(out self):
        self.count = 0

    def pick[o: Origin](self, ref[o] x: Self.T) -> ref[o] Self.T:
        return x


def main():
    var words = Shelf[String]()
    var w = String("w")
    var p = Pair()
    print(words.pick(x=w))
    print(words.pick(p.b))
    var v = String("v")
    var copy = words.pick(v)
    print(copy)
    var u = String("u")
    print(Shelf[String]().pick(u))
