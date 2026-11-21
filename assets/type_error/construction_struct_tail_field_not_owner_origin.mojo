# A struct-typed argument's origin tail must convert to the supplied origin:
# an `S` over `h.x` is no `S[origin_of(h)]`, as at the pin.
# expect: whose origin is the supplied 'o' argument
struct S[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


struct W[mut: Bool, //, o: Origin[mut=mut]]:
    var s: S[Self.o]

    def __init__(out self, var s: S[Self.o]):
        self.s = s^


struct Holder:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def main():
    var h = Holder(1)
    var w = W[origin_of(h)](S(Pointer(to=h.x)))
    print(w.s.p[])
