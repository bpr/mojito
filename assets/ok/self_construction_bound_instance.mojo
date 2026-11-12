# `Self(...)` and `Self.make()` construct the instance `Self` is bound to,
# the struct over its own parameters, including an infer-only `mut`, an
# origin, and a type pack whose binder the instance forwards.


struct S[mut: Bool, //, T: AnyType, origin: Origin[mut=mut]](ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def again(self) -> Self:
        return Self(self.n + 1)


struct P[*Ts: Writable](Copyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n + 100

    def __init__[F: def[idx: Int]() -> Int](out self, *, init_with: F):
        self.n = init_with[0]()

    def again(self) -> Self:
        return Self(copy=self)

    def bump(self) -> Self:
        return Self(self.n + 1)

    def remade(self) -> Self:
        return Self(init_with=lambda[i: Int]() -> Int: i + 40)

    @staticmethod
    def make() -> Self:
        return Self(7)

    def remake(self) -> Self:
        return Self.make()


def main():
    var x = 1
    var s = S[Int, origin_of(x)](3)
    print(s.again().n)
    var p = P[Int, String](1)
    print(p.again().n, p.bump().n, p.remade().n, p.remake().n)
    print(P[Int].make().n)
