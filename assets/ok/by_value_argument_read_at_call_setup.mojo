# A by-value place argument is read at call setup, after every argument has
# been evaluated, as the pinned Mojo reads it: a later argument that writes
# the place is seen by the earlier one. Plain, keyword, method, receiver,
# operator, comparison-chain, subscript, setter, and indirect calls; a scalar
# field, a scalar local, a register-passable struct, a memory-only aggregate,
# an implicit conversion; a method's `var` copies made in parameter order.
@fieldwise_init
struct C(Copyable, Movable):
    var t: Int

    def __deinit__(deinit self):
        print("del", self.t)

    def show(self, b: Int):
        print("show", self.t, b)

    def m(self, a: Int, b: Int):
        print("m", a, b)

    def __getitem__(self, a: Int, b: Int) -> Int:
        print("getitem", a, b)
        return a + b

    def __add__(self, o: Int) -> Int:
        return self.t + o


@fieldwise_init
struct S(Copyable, Movable):
    var t: Int

    def __getitem__(self, a: Int) -> Int:
        return a

    def __setitem__(mut self, a: Int, b: Int):
        print("setitem", a, b)


@fieldwise_init
struct D(ImplicitlyCopyable, Movable):
    var t: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.t)
        self.t = copy.t

    def own2(self, var x: D, var y: D):
        print("own2", x.t, y.t)


@fieldwise_init
struct H(Copyable, Movable):
    var c: C


@fieldwise_init
struct P(TrivialRegisterPassable):
    var t: Int

    def show(self, b: Int):
        print("pshow", self.t, b)


struct W(ImplicitlyCopyable, Movable):
    var v: Float64

    @implicit
    def __init__(out self, i: Int):
        print("conv", i)
        self.v = Float64(i)


def bump(mut w: C) -> Int:
    w.t += 100
    return 0


def bumph(mut w: H) -> Int:
    w.c.t += 100
    return 0


def bumpp(mut w: P) -> Int:
    w.t += 100
    return 0


def setn(mut n: Int) -> Int:
    n += 100
    return 0


def two(a: Int, b: Int):
    print("two", a, b)


def own(var x: Int, var y: Int):
    print("own", x, y)


def g(d: C) -> Int:
    print("g", d.t)
    return d.t


def rd(x: C, y: Int):
    print("rd", x.t, y)


def twop(a: P, b: Int):
    print("twop", a.t, b)


def tf(a: W, b: Int):
    print("tf", a.v, b)


def nested(a: Int, b: Int) -> Int:
    print("nested", a, b)
    return a


def main():
    var c = C(3)
    two(c.t, bump(c))
    var d = C(4)
    print(d.t + bump(d))
    var e = C(5)
    var f = C(6)
    own(e.t, g(f))
    var k = C(1)
    two(b=k.t, a=bump(k))
    var n = 2
    two(n, setn(n))
    var r = C(6)
    two(r.t, nested(r.t, bump(r)))
    var s = C(7)
    two(s.t, bump(s) + s.t)
    var w = C(10)
    print(w.t, bump(w))
    var a = C(1)
    tf(a.t, bump(a))
    var h = H(C(3))
    rd(h.c, bumph(h))
    var o = C(4)
    print(o + bump(o))
    _ = o.t
    var p = P(2)
    twop(p, bumpp(p))
    var q = P(5)
    q.show(bumpp(q))
    var u = C(1)
    u.show(bump(u))
    var v = C(0)
    var x = C(3)
    v.m(x.t, bump(x))
    var y = C(4)
    _ = v[y.t, bump(y)]
    var fp = two
    var z = C(5)
    fp(z.t, bump(z))
    var l = C(4)
    print(l.t > bump(l) + 50 < 1000)
    var m = C(5)
    var st = S(0)
    st[m.t] = bump(m)
    var obj = D(0)
    var d2 = D(2)
    var d3 = D(3)
    obj.own2(y=d2, x=d3)
    _ = d2.t
    _ = d3.t
