# A struct generic over a parameter its layout never reads, holding a
# `String`: its synthesized and written copy constructors, `copy()`, and an
# explicit `S[n](copy=c)` in a generic function all compile and run.
struct S[n: Int](Copyable, Movable):
    var s: String

    def __init__(out self, s: String):
        self.s = s


struct W[T: AnyType](Copyable, Movable):
    var s: String

    def __init__(out self, s: String):
        self.s = s


struct C[n: Int](Copyable, Movable):
    var s: String

    def __init__(out self, s: String):
        self.s = s

    @always_inline("nodebug")
    def __init__(out self, *, copy: Self):
        self.s = copy.s


def dup[n: Int](c: S[n]) -> S[n]:
    return S[n](copy=c)


def main():
    var a = S[3]("a")
    print(a.s)
    var b = W[Int]("b")
    var b2 = b.copy()
    print(b.s, b2.s)
    var c = C[4]("c")
    var c2 = c.copy()
    print(c.s, c2.s)
    var d = dup(a)
    print(a.s, d.s)
