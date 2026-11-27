# A method of a struct with origin parameters satisfies a trait requirement
# that declares its own origin binders: the witness numbers its own binders
# past its struct's origin parameters while the requirement numbers them from
# zero, and the two signatures still compare up to their own binders, in a
# pointer argument, a `ref` argument and result, and a `mut=` binder.
struct V[mut: Bool, //, T: Copyable, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


trait Reads:
    def read[o: ImmOrigin](self, p: Pointer[Int, o]) -> Int:
        ...

    def pick[o: ImmOrigin](self, ref[o] p: Int) -> ref[o] Int:
        ...

    def view[m: Bool, //, o: Origin[mut=m]](self, v: V[Int, o]) -> Int:
        ...


struct B[so: ImmOrigin](Reads):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def read[o: ImmOrigin](self, p: Pointer[Int, o]) -> Int:
        return self.k + p[]

    def pick[o: ImmOrigin](self, ref[o] p: Int) -> ref[o] Int:
        return p

    def view[m: Bool, //, o: Origin[mut=m]](self, v: V[Int, o]) -> Int:
        return self.k * v.n


def through[X: Reads](x: X, v: V[Int, ImmutAnyOrigin]):
    var y = 2
    print(x.read(Pointer(to=y)), x.pick(y), x.view(v))


def main():
    var y = 3
    var b = B[ImmutAnyOrigin](4)
    var v = V[Int, ImmutAnyOrigin](5)
    print(b.read(Pointer(to=y)), b.pick(y), b.view(v))
    through(b, v)
