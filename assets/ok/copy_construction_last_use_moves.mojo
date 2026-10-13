# A named struct's copy construction whose whole owned source dies at it
# moves the source, as at Mojo; a read-after source, a field, a type
# parameter's T(copy=x), and .copy() still copy.
struct S(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __deinit__(deinit self):
        print("del", self.v)

struct M(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __deinit__(deinit self):
        print("mdel", self.v)

struct B[T: Copyable](Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __deinit__(deinit self):
        print("bdel", self.v)

def mk() -> M:
    return M(5)

def own(var s: S) -> S:
    return S(copy=s)

def g[T: Copyable](var b: B[T]) -> B[T]:
    return B[T](copy=b)

def gen[T: Copyable & Deinitable](var x: T) -> T:
    return T(copy=x)

struct P(Copyable, Movable):
    var a: S
    def __init__(out self, var a: S):
        self.a = a^

def main():
    var a = S(copy=S(1))
    print("a", a.v)
    var am = M(copy=M(1))
    print("am", am.v)
    var amk = M(copy=mk())
    print("amk", amk.v)
    var s = S(2)
    var d = S(copy=s)
    print("d", d.v)
    var sm = M(2)
    var dm = M(copy=sm)
    print("dm", dm.v)
    var o = own(S(2))
    print("o", o.v)
    var r = S(6)
    var t = S(copy=r)
    r = S(7)
    print("t", t.v, r.v)
    var z = S(9)
    if t.v > 0:
        var c2 = S(copy=z)
        print("c", c2.v)
    var w = B[Int](3)
    _ = B[Int](copy=w)
    var x = g(B[Int](1))
    print("x", x.v)
    for _ in range(2):
        var q = S(5)
        var rr = S(copy=q)
        print("r", rr.v, q.v)
    var k = S(10)
    var kk = S(copy=k)
    print("k", kk.v, k.v)
    var gg = gen(S(1))
    print("g", gg.v)
    var p = P(S(3))
    var f = S(copy=p.a)
    print("f", f.v)
    var u = S(4)
    var e = u.copy()
    print("e", e.v)
    print("end")
