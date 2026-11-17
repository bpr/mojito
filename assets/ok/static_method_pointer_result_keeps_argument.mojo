# A static method's `Pointer[T, o]` result over a `Pointer(to=z)` argument
# keeps `z` alive for the result's later reads, as an instance method's and a
# free function's do: the result's type names the argument's origin. The
# struct-parameter form (`View[origin_of(x)].same(...)`, over
# `-> Pointer[Noisy, Self.o]`) keeps `x` alive the same way.
struct Noisy(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


@fieldwise_init
struct S:
    @staticmethod
    def same[m: Bool, //, o2: Origin[mut=m]](p: Pointer[Noisy, o2]) -> Pointer[Noisy, o2]:
        return p

    def isame[m: Bool, //, o2: Origin[mut=m]](self, p: Pointer[Noisy, o2]) -> Pointer[Noisy, o2]:
        return p


struct View[mut: Bool, //, o: Origin[mut=mut]](Copyable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p

    @staticmethod
    def same(p: Pointer[Noisy, Self.o]) -> Pointer[Noisy, Self.o]:
        return p


def main():
    var y = Noisy(8)
    var s = S()
    var r = s.isame(Pointer(to=y))
    print(r[].v)
    var z = Noisy(9)
    var t = S.same(Pointer(to=z))
    print(t[].v)
    var x = Noisy(7)
    var q = View[origin_of(x)].same(Pointer(to=x))
    print(q[].v)
