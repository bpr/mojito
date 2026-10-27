# A destructuring assignment's destinations evaluate left to right before
# the right-hand side, a nominal subscript's indices included; the value is
# evaluated once, elements copy out of a temporary tuple (a `_` copies
# nothing), and the temporary is destroyed after the stores.
def idx(i: Int) -> Int:
    print("idx", i)
    return i


def pair() -> Tuple[Int, Int]:
    print("pair")
    return (10, 20)


struct Tracked(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def tpair() -> Tuple[Tracked, Tracked]:
    print("tpair")
    return (Tracked(1), Tracked(2))


def main():
    var a = 1
    var b = 2
    (a, b) = (b, a)
    print(a, b)
    var xs: List[Int] = [0, 0, 0]
    (xs[idx(0)], xs[idx(2)]) = pair()
    print(xs[0], xs[2])
    var i = 0
    (i, xs[i]) = (2, 7)
    print(i, xs[0], xs[2])
    var t1 = Tracked(8)
    var t2 = Tracked(9)
    print("assign")
    (t1, t2) = tpair()
    print("after", t1.v, t2.v)
    print("decl")
    var (c, d) = tpair()
    print("after", c.v, d.v)
    print("move")
    var (e, _) = tpair()^
    print("after", e.v)
    print("end")
