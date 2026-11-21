# A conformer's method satisfies a trait requirement that declares its own
# origin binder, explicit (`[o: ImmOrigin]`, over a pointer or a `ref`
# argument) or automatic (`Pointer[Int, _]`): the two signatures compare up
# to their own binders, as upstream compares them.
trait Reads:
    def read[o: ImmOrigin](self, p: Pointer[Int, o]) -> Int:
        ...

    def read_ref[o: ImmOrigin](self, ref[o] p: Int) -> Int:
        ...

    def read_any(self, p: Pointer[Int, _]) -> Int:
        ...


struct A(Reads):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def read[o: ImmOrigin](self, p: Pointer[Int, o]) -> Int:
        return self.v + p[]

    def read_ref[o: ImmOrigin](self, ref[o] p: Int) -> Int:
        return self.v * p

    def read_any(self, p: Pointer[Int, _]) -> Int:
        return self.v - p[]


def through[X: Reads](x: X):
    var y = 2
    print(x.read(Pointer(to=y)), x.read_ref(y), x.read_any(Pointer(to=y)))


def main():
    var y = 3
    var a = A(4)
    print(a.read(Pointer(to=y)), a.read_ref(y), a.read_any(Pointer(to=y)))
    through(a)
