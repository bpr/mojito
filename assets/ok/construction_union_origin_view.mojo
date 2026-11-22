# A user view built with an explicit union origin, `View[origin_of(a, b)](a)`,
# keeps both members alive until its last use: the construction lends what
# its result type names, the argument it borrowed and the other member, and
# the view's `Pointer[D, Self.o]` field reads at the union.
struct D(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


struct View[o: ImmOrigin](Movable):
    var p: Pointer[D, Self.o]

    def __init__(out self, ref[Self.o] x: D):
        self.p = Pointer(to=x)


def main():
    var a = D(1)
    var b = D(2)
    var v = View[origin_of(a, b)](a)
    print("use")
    print(v.p[].n)
    print("end")
