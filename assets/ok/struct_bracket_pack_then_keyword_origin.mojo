# A struct application binds its whole bracket in one solve: a keyword
# origin after a pack binds the pack to what precedes it (empty when
# nothing does), and an infer-only binder binds by keyword ahead of the
# positional arguments.
struct P[*Ts: AnyType, o: ImmOrigin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


struct H[o: ImmOrigin, //, T: AnyType]:
    var n: Int

    def __init__(out self):
        self.n = 1


struct M[mut: Bool, //, T: AnyType, o: Origin[mut=mut]]:
    var n: Int

    def __init__(out self):
        self.n = 2


def show(p: P[Int, Bool, o=_]):
    print(p.p[])


def main():
    var x = 5
    var empty = P[o=origin_of(x)](Pointer(to=x))
    print(empty.p[])
    var two = P[Int, Bool, o=origin_of(x)](Pointer(to=x))
    show(two)
    var h = H[o=origin_of(x), Int]()
    print(h.n)
    var m = M[mut=True, Int, origin_of(x)]()
    print(m.n)
