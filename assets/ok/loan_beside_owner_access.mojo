# A loan names an origin and borrows nothing exclusively, as upstream's
# references do: an element read beside a live mutable `Span`, a store to a
# pointer's owner, and two mutable user views over one place all run, as at
# the pin (R1160).
struct MV[o: MutOrigin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref[Self.o] x: Int):
        self.p = Pointer(to=x)


struct P[o: Origin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref[Self.o] to: Int):
        self.p = Pointer(to=to)


def span_element_store():
    var l: List[Int] = [1, 2, 3]
    var s = Span(l)
    s[0] = 7
    print(l[0], len(s))


def owner_store_beside_pointer():
    var x = String("ab")
    var p = Pointer(to=x)
    x = String("zz")
    print(p[])


def owner_store_beside_user_view():
    var x = 1
    var v = P(to=x)
    x = 5
    print(v.p[])


def two_mutable_views():
    var x = 1
    var v = MV(x)
    var w = MV(x)
    w.p[] += 1
    v.p[] += 10
    print(x)


def main():
    span_element_store()
    owner_store_beside_pointer()
    owner_store_beside_user_view()
    two_mutable_views()
