# Each link of a chained comparison is its dunder's call, and the operands
# it reads die when that call returns. Right after the first link the first
# middle becomes an owned temporary for the second link: a place is copied,
# a temporary or `^` moves in. Later middles are read in place.
struct S(ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __lt__(self, o: Self) -> Bool:
        print("lt", self.v, o.v)
        return self.v < o.v

    def __deinit__(deinit self):
        print("del", self.v)


struct E(Equatable, ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __eq__(self, o: Self) -> Bool:
        print("eq", self.v, o.v)
        return self.v == o.v

    def __deinit__(deinit self):
        print("del", self.v)


struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __lt__(self, o: Self) -> Bool:
        print("lt", self.v, o.v)
        return self.v < o.v

    def __deinit__(deinit self):
        print("del", self.v)


struct C(Comparable, ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __eq__(self, o: Self) -> Bool:
        return self.v == o.v

    def __lt__(self, o: Self) -> Bool:
        print("lt", self.v, o.v)
        return self.v < o.v

    def __deinit__(deinit self):
        print("del", self.v)


@fieldwise_init
struct H(Movable):
    var s: S


def ordered[T: Comparable & ImplicitlyCopyable & Deinitable](x: T, y: T, z: T) -> Bool:
    return x < y < z


def main():
    var a = S(1)
    var b = S(2)
    var c = S(3)
    print(a < b < c)
    print(b.v)
    print("--")
    var d = S(9)
    var e = S(8)
    var f = S(10)
    print(d < e < f)
    print(e.v, f.v)
    print("--")
    var g = S(1)
    var h = S(2)
    var i = S(3)
    var j = S(4)
    print(g < h < i < j)
    print(h.v, i.v, j.v)
    print("--")
    var k = S(1)
    var holder = H(S(2))
    var l = S(3)
    print(k < holder.s < l)
    print(holder.s.v)
    print("--")
    var p = E(1)
    var q = E(2)
    var r = E(3)
    print(p != q != r)
    print(q.v)
    print("--")
    var m1 = M(1)
    var m3 = M(3)
    var m4 = M(4)
    print(m1 < M(2) < m3 < m4)
    print(m1.v)
    print("--")
    var n1 = M(1)
    var n2 = M(2)
    var n3 = M(3)
    var n4 = M(4)
    print(n1 < n2^ < n3 < n4)
    print("--")
    var x = C(1)
    var y = C(2)
    var z = C(3)
    print(ordered(x, y, z))
    print(x.v, y.v, z.v)
