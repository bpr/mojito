# A struct's `ImmOrigin(...)` slot is a distinct origin from the uncast one
# over a mutable `var`: a view through the cast does not convert back into a
# view that may write `x`, as at the pin.
# expect: cannot implicitly convert
struct Noisy(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct View[mut: Bool, //, o: Origin[mut=mut]](Movable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p


def main():
    var x = Noisy(1)
    var v = View[ImmOrigin(origin_of(x))](Pointer(to=x))
    var w: View[origin_of(x)] = v^
    w.p[].v = 3
    print(x.v)
