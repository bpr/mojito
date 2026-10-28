# A value-parameter argument of a struct type converts through the struct's
# `@implicit` constructor, as a run-time argument does: the bound value is
# that constructor's application, closed or over a binder, so `T[3]` is
# `T[W(3)]`, a literal selects an `IntLiteral` overload, and a builtin
# constructor folds.
struct W(ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, value: Int, /):
        self.v = value * 2

    @implicit
    def __init__(out self, value: Bool, /):
        self.v = 100


struct Lit(ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, value: IntLiteral, /):
        self.v = 1

    @implicit
    def __init__(out self, value: Int, /):
        self.v = value * 2


struct G[k: Int](ImplicitlyCopyable):
    var v: Int

    @implicit
    def __init__(out self, value: Int, /):
        self.v = value * Self.k


struct L(TrivialRegisterPassable):
    var v: Int

    @implicit
    @always_inline("builtin")
    def __init__(out self, v: Int, /):
        self.v = v


struct T[w: W]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.w.v


struct Tagged[l: L](ImplicitlyCopyable):
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.l.v


struct Holder[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return T[Self.n]().get()


def f[w: W]() -> Int:
    return w.v


def g[n: Int]() -> Int:
    return f[n]()


def lit[w: Lit]() -> Int:
    return w.v


def lit_of[n: Int]() -> Int:
    return lit[n]()


def gen[w: G[3]]() -> Int:
    return w.v


def gen_of[n: Int]() -> Int:
    return gen[n + 1]()


def defaulted[w: W = 3]() -> Int:
    return w.v


def solved[n: Int](x: T[n]) -> Int:
    return n


def same[n: Int](t: Tagged[n]) -> Tagged[L(n)]:
    return t


def widen[n: Int](x: SIMD[DType.int8, L(n).v * 2]) -> SIMD[DType.int8, 2 * n]:
    return x


def main():
    print(f[3](), g[5](), f[True]())
    print(lit[3](), lit_of[5]())
    print(gen[3](), gen_of[4]())
    print(defaulted(), defaulted[w=4]())
    var a: T[3] = T[W(3)]()
    var b: T[W(3)] = T[3]()
    print(a.get(), b.get(), solved(a), solved(T[W(7)]()))
    print(Holder[3]().get())
    var t: Tagged[L(3)] = Tagged[3]()
    print(same(t).get(), same[4](Tagged[4]()).get())
    print(widen[2](SIMD[DType.int8, 4](1)))
