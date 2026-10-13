# A construction is its `__init__`'s call, so its arguments bind as a
# call's do: a temporary at a read parameter dies when the initializer
# returns, after a result nothing uses and before a result bound by `_ =`,
# and a place at a read parameter is read where it lies, without a copy.
# A raising initializer makes the construction a raising call, whose result
# is taken after its argument temporaries die.
@fieldwise_init
struct S(Copyable, Movable):
    var v: Int

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", copy.v)

    def __deinit__(deinit self):
        print("del", self.v)


struct P(Movable):
    var t: Int

    def __init__(out self, x: S, y: S):
        self.t = x.v + y.v

    def __init__(out self, x: S, y: S, z: Int) raises:
        self.t = x.v + y.v + z

    def __deinit__(deinit self):
        print("delP", self.t)


struct G[T: Copyable & Movable](Movable):
    var n: Int

    def __init__(out self, x: Self.T, y: S):
        self.n = y.v

    def __deinit__(deinit self):
        print("delG", self.n)


struct V(Movable):
    var n: Int

    def __init__(out self, *xs: S):
        self.n = len(xs)

    def __deinit__(deinit self):
        print("delV", self.n)


def take(p: P):
    print("take", p.t)


def main() raises:
    print(P(S(1), S(2)).t)
    print("--")
    var p = P(S(3), S(4))
    print("--")
    _ = P(S(5), S(6))
    print("--")
    P(S(7), S(8))
    print("--")
    var z = P(y=S(9), x=S(10))
    print("--")
    take(P(S(11), S(12)))
    print("--")
    var s = S(13)
    var q = P(s, s)
    print("--")
    var g = G(S(14), S(15))
    print("--")
    var h = G[Int](1, S(16))
    print("--")
    var k = G[Int](x=1, y=S(17))
    print("--")
    var v = V(S(18), S(19))
    print("--")
    var r = P(S(20), S(21), 1)
    print("--")
    _ = P(S(22), S(23), 1)
    print("--")
