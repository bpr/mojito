# An `ImmOrigin(...)` origin argument is part of a struct's type: a free
# function's `Pointer[T, o]` result over a `View[o]` argument, and a static
# method's `Pointer[T, Self.o]` over an explicit `View[...]`, take the
# slot's origin and its `mut` from the type.
struct Noisy(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct View[mut: Bool, //, o: Origin[mut=mut]](Copyable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p

    @staticmethod
    def same(p: Pointer[Noisy, Self.o]) -> Pointer[Noisy, Self.o]:
        return p


def through[mut: Bool, //, o: Origin[mut=mut]](v: View[o]) -> Pointer[Noisy, o]:
    return v.p


def main():
    var x = Noisy(7)
    var v = View[ImmOrigin(origin_of(x))](Pointer(to=x))
    var p: Pointer[Noisy, ImmOrigin(origin_of(x))] = through(v)
    print(p[].v)
    var w = View[origin_of(x)](Pointer(to=x))
    var m: Pointer[Noisy, origin_of(x)] = through(w)
    m[].v = 8
    var q = View[origin_of(x)].same(Pointer(to=x))
    print(q[].v, x.v)
