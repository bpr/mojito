# A struct whose origin's `mut=` value differs is another type: without a
# converting constructor, a `View` over a mutable origin does not pass where
# a `View` over an `ImmOrigin` binder is declared, as at the pin.
# expect: type mismatch for argument to 'imm': expected View[false], found View[true]
struct View[mut: Bool, //, o: Origin[mut=mut]](Movable):
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


def imm[o: ImmOrigin](v: View[o]):
    print(v.p[])


def main():
    var x = 1
    imm(View(Pointer(to=x)))
