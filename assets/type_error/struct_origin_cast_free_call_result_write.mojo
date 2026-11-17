# expect: write through a Pointer whose origin is immutable
# A free function's `Pointer[T, o]` result over a `View[ImmOrigin(...)]`
# argument names its origin immutably, as the slot's `mut` is `False`.
struct Noisy(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct View[mut: Bool, //, o: Origin[mut=mut]](Copyable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p


def through[mut: Bool, //, o: Origin[mut=mut]](v: View[o]) -> Pointer[Noisy, o]:
    return v.p


def main():
    var x = Noisy(7)
    var v = View[ImmOrigin(origin_of(x))](Pointer(to=x))
    var p = through(v)
    p[].v = 8
    print(x.v)
