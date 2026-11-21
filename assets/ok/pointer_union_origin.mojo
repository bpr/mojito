# A `Pointer` whose origin argument is a union, `origin_of(x, y)`: it
# constructs, converts to an immutable or placeholder origin, passes through
# a struct field and an origin-generic function, survives an untracked round
# trip, and keeps every member alive until its last use.


struct D(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


struct Holder[o: Origin](Movable):
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


def show(p: Pointer[Int, _]):
    print(p[])


def ident[o: Origin](p: Pointer[Int, o]) -> Pointer[Int, o]:
    return p


def members_stay_alive():
    var a = D(1)
    var b = D(2)
    var p = Pointer[D, origin_of(a, b)](to=a)
    print("use")
    print(p[].n)
    print("end")


def main():
    var x = 5
    var y = 6
    var p = Pointer[Int, origin_of(x, y)](to=x)
    print(p[])
    var annotated: Pointer[Int, origin_of(x, y)] = Pointer(to=y)
    print(annotated[])
    var q = Pointer[Int, origin_of(x, y)](to=y)
    show(q)
    q[] = 9
    print(y)
    var h = Holder[origin_of(x, y)](Pointer[Int, origin_of(x, y)](to=x))
    print(h.p[])
    var imm: Pointer[Int, ImmOrigin(origin_of(x, y))] = p
    print(imm[])
    var spelled = Pointer[Int, ImmOrigin(origin_of(x, y))](to=x)
    print(spelled[], spelled.mut)
    print(ident(p)[])
    var u = p.unsafe_origin_cast[MutUntrackedOrigin]()
    var back = u.unsafe_origin_cast[origin_of(x, y)]()
    print(back[])
    p[] = 8
    print(x)
    members_stay_alive()
