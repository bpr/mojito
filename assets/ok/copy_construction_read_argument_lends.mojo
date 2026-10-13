# A copy construction a read parameter takes at its source's last use lends
# the source's storage to the parameter, as at Mojo: no copy or move
# initializer runs, and the source is destroyed when the call returns. A
# source read afterwards still copies, and a raising argument evaluated after
# the construction makes it a move into the temporary.
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
    def __add__(self, other: Self) -> Int:
        print("add", self.v, other.v)
        return 0

def boom(x: Int) raises -> Int:
    if x > 0:
        raise Error("boom")
    return x

def pure(x: Int) -> Int:
    print("pure", x)
    return x

def show(s: M):
    print("show", s.v)

def show_n(s: M, n: Int):
    print("show_n", s.v, n)

def show2(s: M, t: M):
    print("show2", s.v, t.v)

def show_kw(*, s: M):
    print("kw", s.v)

def show_r(s: M) raises:
    print("show_r", s.v)
    raise Error("r")

def from_var(var p: M):
    show(M(copy=p))

def lends():
    var a = M(1)
    show(M(copy=a))
    var b = M(2)
    show(M(copy=b))
    print("b", b.v)
    var c = M(3)
    var d = M(4)
    show2(M(copy=c), M(copy=d))
    var e = M(5)
    show_kw(s=M(copy=e))
    var f = M(6)
    var g = M(60)
    _ = M(copy=f) + g
    var h = M(7)
    show_n(M(copy=h), pure(70))
    show(M(copy=M(8)))
    from_var(M(9))
    for i in range(10, 12):
        var l = M(i)
        show(M(copy=l))
    var k = M(12)
    if k.v > 0:
        show(M(copy=k))
    print("lends end")

def raising():
    var a = M(20)
    try:
        show_n(M(copy=a), boom(1))
    except e:
        print("caught", e)
    var b = M(21)
    try:
        show_r(M(copy=b))
    except e:
        print("caught", e)
    try:
        var c = M(22)
        show_r(M(copy=c))
    except e:
        print("caught", e)
    print("raising end")

def main():
    lends()
    raising()
