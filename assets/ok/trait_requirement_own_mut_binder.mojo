# A conformer's method satisfies a trait requirement that declares its own
# origin binder together with that origin's `mut=` binder, over a struct
# instance whose `mut=` slot reads the binder: the two signatures compare up
# to their own binders, so the requirement's `m` and the witness's `mm` are
# one slot.
struct V[mut: Bool, //, T: Copyable, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


trait Reads:
    def read[m: Bool, //, o: Origin[mut=m]](self, v: V[Int, o]) -> Int:
        ...


struct C(Reads):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def read[mm: Bool, //, oo: Origin[mut=mm]](self, v: V[Int, oo]) -> Int:
        return self.k + v.n


def through[X: Reads](x: X, v: V[Int, ImmutAnyOrigin]) -> Int:
    return x.read(v)


def main():
    var c = C(1)
    var v = V[Int, ImmutAnyOrigin](3)
    var w = V[Int, MutAnyOrigin](5)
    print(c.read(v), through(c, v), c.read(w))
