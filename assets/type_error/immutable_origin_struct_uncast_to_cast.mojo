# A view over a mutable `var` does not implicitly convert into the view
# whose slot is the `ImmOrigin(...)` cast of the same origin: the two
# origins differ in `mut`, and the struct declares no conversion, as at
# the pin.
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
    var v = View(Pointer(to=x))
    var w: View[ImmOrigin(origin_of(x))] = v^
    print(w.p[].v)
