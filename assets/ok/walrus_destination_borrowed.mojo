# A walrus over a name or a field chain yields its destination borrowed
# after the store, as upstream's `emitWalrus` does: a `read` consumer
# borrows it, a value consumer copies it once, a non-Copyable destination
# is read in place, and `^` moves out of it.
@fieldwise_init
struct C(ImplicitlyCopyable, Writable):
    var v: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v


struct N(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct H(Movable):
    var c: C
    var n: N

    def __init__(out self):
        self.c = C(1)
        self.n = N(1)


def show(c: C):
    print("show", c.v)


def take(var n: N):
    print("take", n.v)


def main():
    var s = String("a")
    print(s := String("b"))
    print(s)

    var h = H()
    show(h.c := C(6))
    var d = (h.c := C(7))
    print(d.v, h.c.v)
    print((h.n := N(5)).v)
    take((h.n := N(8))^)
    h.n = N(9)

    var e = C(0)
    show(e := C(2))
    var f = (e := C(3))
    print(f.v, e.v)

    var x: Float64 = 0
    print(x := 1)
    var n = 3
    while (n := n - 1) > 0:
        print(n)
