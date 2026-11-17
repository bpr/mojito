# An empty register-passable value passed by value has no physical
# argument natively, and a by-address slot of one keeps its pointer.

trait G:
    def tag(self) -> Int: ...
    def bump(mut self, x: Int) -> Int: ...


struct E(Copyable, G):
    def __init__(out self):
        pass

    def tag(self) -> Int:
        return 7

    def bump(mut self, x: Int) -> Int:
        return x + 1


struct R(TrivialRegisterPassable, G):
    def __init__(out self):
        pass

    def tag(self) -> Int:
        return 9

    def __getitem__(self, i: Int) -> Int:
        return i * 2

    @no_inline
    def bump(mut self, x: Int) -> Int:
        self = R()
        return x + 2


struct D(RegisterPassable, Movable):
    def __init__(out self):
        print("init D")

    def __deinit__(deinit self):
        print("drop D")


struct Holder(Copyable):
    var r: R
    var x: Int

    def __init__(out self, x: Int):
        self.r = R()
        self.x = x


@no_inline
def take[T: G](e: T) -> Int:
    return e.tag()


@no_inline
def mg[T: G](mut t: T, x: Int) -> Int:
    return t.bump(x) + t.tag()


@no_inline
def kg[T: G](a: Int, *, t: T, b: Int) -> Int:
    return a * 10 + b + t.tag()


def dbl(x: Int) -> Int:
    return x * 2


@no_inline
def apply[F: def(Int) -> Int](f: F) -> Int:
    return f(21)


def plus(r: R, x: Int) -> Int:
    return r.tag() + x


@no_inline
def via[F: def(R, Int) -> Int](f: F, r: R) -> Int:
    return f(r, 5)


@no_inline
def dd(var d: D, x: Int) -> Int:
    return x + 1


@no_inline
def field(h: Holder) -> R:
    return h.r


@no_inline
def pair(t: Tuple[R, R], *, r: R) -> Int:
    return t[0].tag() + r.tag()


def main():
    var r = R()
    var e = E()
    print(take(E()), take(r), apply(dbl))
    print(mg(r, 4), mg(e, 4), kg(1, b=2, t=r))
    print(via(plus, r), r[5], pair((r, r), r=r))
    var h = Holder(5)
    print(field(h).tag(), h.x)
    var d = D()
    print(dd(d^, 1))
    var t = (D(), D())
    print("tuple made")
    _ = t^
    print("done")
