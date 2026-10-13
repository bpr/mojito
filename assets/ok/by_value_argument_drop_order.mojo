# Pinned against Mojo 1.2.0.dev2026100406 (2026-10-13): a scalar read out of
# a value and passed to a concrete register-passable parameter is loaded at
# call setup, in parameter order, so an owner whose last use it is dies
# before the call runs; a slice's bounds load first. A generic, variadic,
# or by-reference argument is passed by address, and its owner dies after
# the call.
# Expected: del 1 / del 2 / 3 / del 6 / del 5 / two / 11 / del 18 / tk / del 17
# / 35 / del 14 / m / del 13 / 27 / del 9 / del 10 / kw / 19 / del 3 / del 4 /
# two / del 12 / del 11 / inner / del 7 / getitem / 107 / del 62 / del 61 /
# two-index / 223 / del 63 / del 64 / slice / 100 / del 65 / del 66 / del 67 /
# index-slice / 167 / del 21 / delP 3 / del 22 / delP 4 / two / del 1 / del 2 /
# two / 63 / 41 42 / del 42 / del 41 / gen / del 52 / del 51 / end
struct W(Movable):
    var t: Int

    def __init__(out self, t: Int):
        self.t = t

    def __deinit__(deinit self):
        print("del", self.t)

    def m(self, x: Int) -> Int:
        print("m")
        return self.t + x


def two(x: Int, y: Int) -> Int:
    print("two")
    return x + y


def tk(x: Int, w: W) -> Int:
    print("tk")
    return x + w.t


def kw(x: Int, *, y: Int) -> Int:
    print("kw")
    return x + y


def gen[T: Writable](x: T, y: T):
    print("gen")


struct P(Movable):
    var w: W
    var n: Int

    def __init__(out self, var w: W, n: Int):
        self.w = w^
        self.n = n

    def __deinit__(deinit self):
        print("delP", self.n)


struct G(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, i: Int) -> Int:
        print("getitem")
        return self.base + i

    def __getitem__(self, i: Int, j: Int) -> Int:
        print("two-index")
        return self.base + i + j

    def __getitem__(self, s: Slice) -> Int:
        print("slice")
        return self.base

    def __getitem__(self, i: Int, s: Slice) -> Int:
        print("index-slice")
        return self.base + i


struct Pair(Movable):
    var a: W
    var b: W

    def __init__(out self):
        self.a = W(1)
        self.b = W(2)

    def __deinit__(deinit self):
        print(two(self.a.t, self.b.t) * 0 + 63)


def operators_and_calls():
    var a = W(1)
    var b = W(2)
    print(a.t + b.t)
    var e = W(5)
    var h = W(6)
    print(two(h.t, e.t))
    var r = W(17)
    var s = W(18)
    print(tk(s.t, r))
    var n = W(13)
    var o = W(14)
    print(n.m(o.t))
    var i = W(9)
    var j = W(10)
    print(kw(y=j.t, x=i.t))


def callees():
    var c = W(3)
    var d = W(4)
    var fp = two
    _ = fp(c.t, d.t)
    var k = W(11)
    var l = W(12)

    def inner(x: Int, y: Int) -> Int:
        print("inner")
        return x + y

    _ = inner(l.t, k.t)


def subscripts():
    var g = G(100)
    var f = W(7)
    print(g[f.t])
    var a = W(61)
    var b = W(62)
    print(g[b.t, a.t])
    var c = W(63)
    var d = W(64)
    print(g[c.t : d.t])
    var e = W(65)
    var h = W(66)
    var i = W(67)
    print(g[i.t, e.t : h.t])


def field_owners():
    var p = P(W(21), 3)
    var q = P(W(22), 4)
    _ = two(p.n, q.n)


def destroy_pair():
    var pair = Pair()
    _ = pair^


def by_address():
    var w = W(41)
    var x = W(42)
    print(w.t, x.t)
    var y = W(51)
    var z = W(52)
    gen(y.t, z.t)


def main():
    operators_and_calls()
    callees()
    subscripts()
    field_owners()
    destroy_pair()
    by_address()
    print("end")
