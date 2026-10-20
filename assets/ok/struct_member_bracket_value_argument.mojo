# A struct's `comptime` value member passes as a bracket value argument, as
# at the pin: `Box[Self.n]` reads the member `n`, alone, by keyword, or in an
# expression, in a later member's body, a parameterized member's body, a
# method body, a field's `SIMD` width, a member application's argument
# (`Self.E[Self.n]`, `S.F[S.n]`), and through a bound (`Self.U.K`). A
# trait reads its own requirement the same way (`Box[Self.K]`), in a
# default method, a `comptime` default, and a requirement's signature its
# conformer's witness matches, read directly and through a bound.
struct Box[n: Int](Movable):
    comptime w = Self.n * 10

    def __init__(out self):
        pass


def tens[n: Int](b: Box[n]) -> Int:
    return b.w


trait HasK:
    comptime K: Int
    comptime M: Int = Box[Self.K].w

    def get(self) -> Box[Self.K]:
        ...

    def width(self) -> Int:
        return Box[Self.K].w


struct A(HasK):
    comptime K = 4

    def __init__(out self):
        pass

    def get(self) -> Box[Self.K]:
        return Box[Self.K]()


struct S:
    comptime n = 3
    comptime m = Box[Self.n].w
    comptime q = Box[n=Self.n].w
    comptime r = Box[Self.n + 1].w
    comptime E[i: Int] = Box[Self.n + i]
    comptime F[i: Int] = Box[Self.n]

    def __init__(out self):
        pass

    def direct(self) -> Int:
        return Box[Self.n].w

    def built(self) -> Int:
        return tens(Box[Self.n]())

    def applied(self) -> Int:
        var b: Self.E[Self.n] = Box[6]()
        return tens(b)


struct P[k: Int]:
    comptime n = Self.k * 2
    comptime m = Box[Self.n].w


struct T[U: HasK]:
    comptime m = Box[Self.U.K].w

    def __init__(out self):
        pass

    def read(self) -> Int:
        return Box[Self.U.K].w


struct V:
    comptime w = 4
    var v: SIMD[DType.int64, Self.w]

    def __init__(out self):
        self.v = SIMD[DType.int64, Self.w](7)


def show[k: Int]():
    print(P[k].m)


def through[X: HasK](x: X):
    print(x.get().w, x.width(), X.M)


def main():
    print(S.m, S.q, S.r)
    var b: S.E[1] = Box[4]()
    var c: S.F[0] = Box[3]()
    var d: S.F[S.n] = Box[3]()
    print(tens(b), tens(c), tens(d), S().applied())
    print(S().direct(), S().built())
    show[2]()
    print(T[A].m, T[A]().read())
    print(V().v)
    print(A().get().w, A().width(), A.M)
    through(A())
