# An `@implicit` constructor whose refined `out self` names its own origin
# binder's `ImmOrigin(...)` cast converts only a source whose origin that
# binder solves to the target's: a `y` view is no `ImmOrigin(origin_of(x))`
# one, as at the pin.
# expect: cannot implicitly convert
struct V[mut: Bool, //, T: AnyType, o: Origin[mut=mut]](TrivialRegisterPassable):
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p

    @implicit
    def __init__[
        m: Bool, //, src: Origin[mut=m]
    ](other: V[Self.T, src], out self: V[Self.T, ImmOrigin(src)]):
        self = rebind[V[Self.T, ImmOrigin(src)]](other)


def main():
    var x = 3
    var y = 4
    var v: V[Int, ImmOrigin(origin_of(x))] = V(Pointer(to=y))
    print(v.p[])
