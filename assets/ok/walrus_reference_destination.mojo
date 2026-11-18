# A walrus over a memory lvalue other than a name or a field chain yields
# its destination borrowed after the store, as upstream's `emitWalrus` does:
# a list element through its getter's reference, a pointer dereference, a
# call's mutable reference result, and a field below one. A borrowing read
# runs no copy initializer, a non-Copyable destination is read in place, and
# only an explicit `.copy()` copies.
@fieldwise_init
struct C(Copyable, Movable):
    var v: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v


@fieldwise_init
struct N(Movable):
    var v: Int


struct H:
    var c: C

    def __init__(out self):
        self.c = C(0)

    def at(mut self) -> ref[self.c] C:
        return self.c


def show(c: C):
    print("show", c.v)


def main():
    var xs: List[C] = []
    xs.append(C(1))
    print((xs[0] := C(3)).v)
    show(xs[0] := C(4))
    var r = (xs[0] := C(9)).copy()
    print(r.v, xs[0].v)
    print((xs[0].v := 7), xs[0].v)

    var ns: List[N] = []
    ns.append(N(1))
    print((ns[0] := N(5)).v, ns[0].v)

    var p = xs.unsafe_ptr()
    show(p[unsafe_offset=0] := C(11))
    print(xs[0].v)

    var h = H()
    print((h.at() := C(6)).v, h.c.v)
    print((h.at().v := 8), h.c.v)

    var ys: List[Int] = [1, 2]
    var q = (ys[0] := 4)
    print(q, ys[0])
    while (ys[1] := ys[1] + 1) < 5:
        print("loop", ys[1])
