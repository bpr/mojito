# A view over a union origin of two mutable owners, `MV[origin_of(x, y)](x)`
# with `o: Origin`, is mutable, as every member is: a store through its
# `Pointer[Int, Self.o]` field writes `x`.
struct MV[o: Origin](Movable):
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref[Self.o] x: Int):
        self.p = Pointer(to=x)


def main():
    var x = 1
    var y = 2
    var v = MV[origin_of(x, y)](x)
    v.p[] = 5
    print(x, y)
