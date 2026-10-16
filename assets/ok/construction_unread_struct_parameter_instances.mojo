# A construction calls the constructor the check selected, passing the
# struct's parameters as that constructor's, as upstream's call does, so it
# passes only those the constructor kept: `mk[n]()` returning `Box[n](5).v`
# has one instance for every `n`, since `Box.__init__` reads no struct
# parameter, as on the pin. So do a static `make` that constructs
# `Box[Self.n]` (`mk2`), an `__iter__` that constructs `NIter[Self.n]`
# (`fit`), a copy construction `C[n](copy=c)` over an explicit copy
# constructor (`dup`), and an implicit conversion through an overloaded
# constructor (`conv`). `keep`, whose constructor reads `Self.n`, keeps one
# instance per value, as on the pin.
struct Box[n: Int]:
    var v: Int

    @no_inline
    def __init__(out self, v: Int):
        self.v = v

    @staticmethod
    @no_inline
    def make() -> Self:
        return Box[Self.n](7)


struct NIter[n: Int](Iterator, Movable):
    comptime Element = Int
    var i: Int

    @no_inline
    def __init__(out self, i: Int):
        self.i = i

    @no_inline
    def __next__(mut self) raises StopIteration -> Int:
        if self.i <= 0:
            raise StopIteration()
        self.i -= 1
        return self.i


struct Seq[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __iter__(self) -> NIter[Self.n]:
        return NIter[Self.n](self.v)


struct C[n: Int](Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100


struct V[n: Int]:
    var v: Int

    @implicit
    @no_inline
    def __init__(out self, v: Int):
        self.v = v * 2

    def __init__(out self, v: Int, w: Int):
        self.v = v + w


struct K[n: Int]:
    var v: Int

    @no_inline
    def __init__(out self, v: Int):
        self.v = v + Self.n


@no_inline
def mk[n: Int]() -> Int:
    return Box[n](5).v


@no_inline
def mk2[n: Int]() -> Int:
    return Box[n].make().v


@no_inline
def fit[n: Int](s: Seq[n]) -> Int:
    var t = 0
    for x in s:
        t += x
    return t


@no_inline
def dup[n: Int](c: C[n]) -> Int:
    return C[n](copy=c).v


@no_inline
def conv[n: Int]() -> Int:
    var w: V[n] = 4
    return w.v


@no_inline
def keep[n: Int]() -> Int:
    return K[n](5).v


def main():
    print(mk[1](), mk[2]())
    print(mk2[1](), mk2[2]())
    print(fit(Seq[1](3)), fit(Seq[2](4)))
    print(dup(C[1](3)), dup(C[2](4)))
    print(conv[1](), conv[2]())
    print(keep[1](), keep[2]())
