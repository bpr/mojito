# An `@implicit` constructor generic over its own origin binder, whose
# refined `out self` names that binder's `ImmOrigin(...)` cast, solves the
# binder from the source's origin tail: `V(v)` and `Span(s)` construct
# through it, as at the pin.
struct V[mut: Bool, //, T: AnyType, o: Origin[mut=mut]](TrivialRegisterPassable):
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p

    @implicit
    def __init__[
        m: Bool, //, src: Origin[mut=m]
    ](other: V[Self.T, src], out self: V[Self.T, ImmOrigin(src)]):
        print("conv")
        self = rebind[V[Self.T, ImmOrigin(src)]](other)


def main():
    var x = 3
    var v = V(Pointer(to=x))
    var w: V[Int, ImmOrigin(origin_of(x))] = V(v)
    print(w.p[])
    var xs: List[Int] = [1, 2, 3]
    var s = Span(xs)
    var t: Span[Int, ImmOrigin(origin_of(xs))] = Span(s)
    print(len(t), t[2])
