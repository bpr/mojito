# A `^` transfer an owned (`var`/`deinit`) parameter takes hands over the
# source's storage, as at Mojo, so no move initializer runs: as an argument,
# a keyword argument, a receiver, or an operator operand. A copy
# construction such a parameter takes at its source's last use forwards the
# source the same way. A move into new storage (a binding, a return, a field,
# a container element) still runs the move initializer.
struct M(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __deinit__(deinit self):
        print("del", self.v)
    def consume(var self):
        print("consume", self.v)
    def finish(deinit self):
        print("finish", self.v)
    def __add__(var self, var other: Self) -> Int:
        print("add", self.v, other.v)
        return 0

struct N(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __neg__(var self) -> Int:
        print("neg", self.v)
        return 0

struct W(Movable):
    var m: M
    def __init__(out self, var m: M):
        self.m = m^

def take(var s: M):
    print("take", s.v)

def take2(var a: M, var b: M):
    print("take2", a.v, b.v)

def take_kw(*, var s: M):
    print("kw", s.v)

def inner(var s: M):
    take(s^)

def take_store(var s: M) -> M:
    return s^

def keep(var s: M):
    var t = s^
    print("keep", t.v)

def ret(var s: M) -> M:
    return M(copy=s)

def gen[T: Movable & Deinitable](var t: T):
    pass

def gen_ret[T: Movable](var t: T) -> T:
    return t^

def maybe(var s: M) raises:
    print("maybe", s.v)

def transfers() raises:
    var x = M(6)
    take(x^)
    take(M(8))
    var w = M(10)
    var r = take_store(w^)
    print("r", r.v)
    var k = M(11)
    keep(k^)
    var c = M(12)
    if c.v > 0:
        take(c^)
    var a = M(13)
    var b = a^
    print("b", b.v)
    var p = M(2)
    var q = M(3)
    take2(p^, q^)
    var g = M(4)
    inner(g^)
    for i in range(20, 22):
        var l = M(i)
        take(l^)
    var a1 = M(1)
    a1^.consume()
    var b1 = M(2)
    b1^.finish()
    var c1 = M(3)
    gen(c1^)
    var d1 = M(4)
    var dd = gen_ret(d1^)
    print("dd", dd.v)
    var e1 = M(5)
    var ww = W(e1^)
    print("w", ww.m.v)
    var f1 = M(6)
    take_kw(s=f1^)
    var l1 = List[M]()
    var g1 = M(7)
    l1.append(g1^)
    print("l", l1[0].v)
    var h1 = M(9)
    var o = Optional(h1^)
    print("o", o.value().v)
    var c2 = M(10)
    var d2 = M(20)
    _ = (c2^) + (d2^)
    var n1 = N(3)
    _ = -(n1^)
    var b3 = M(2)
    var rb = ret(b3^)
    print("rb", rb.v)

def copies() raises:
    var y = M(7)
    take(M(copy=y))
    var c = M(3)
    take_kw(s=M(copy=c))
    var p = M(2)
    var q = M(3)
    take2(M(copy=p), M(copy=q))
    var u = M(4)
    take(M(copy=u))
    print("u", u.v)
    var z = M(9)
    take(z.copy())
    print("z", z.v)
    var e = M(5)
    var e2 = M(copy=e)
    print("e2", e2.v)
    var t = M(6)
    var t2 = M(copy=t)
    take(t2^)
    var m = M(8)
    maybe(M(copy=m))
    for i in range(30, 32):
        var l = M(i)
        take(M(copy=l))
    var v = M(40)
    if v.v > 0:
        take(M(copy=v))
    var s = M(50)
    take2(M(copy=s), M(51))

def main() raises:
    transfers()
    copies()
