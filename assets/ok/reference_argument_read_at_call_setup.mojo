# A by-value operand with a reference result is evaluated to its lvalue where
# written and read at call setup, as the pinned Mojo reads it: the accessor or
# `ref`-returning call runs in source order, and a later argument's write to
# the referent is seen by the earlier one. A tuple element, a user `ref`
# accessor, a field's and a nested tuple's element, a free and a method
# `ref` call, an operator operand, a `String` element at a read and at a `var`
# parameter, an implicit conversion, and two `Dict` lookups; a
# value-returning accessor is an rvalue and runs in place.
struct S(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __getitem__(ref self, i: Int) -> ref[self.v] Int:
        return self.v

    def get(ref self) -> ref[self.v] Int:
        return self.v


struct V(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __getitem__(self, i: Int) -> Int:
        return self.v + i


@fieldwise_init
struct P(Copyable):
    var t: Tuple[Int, Int]


struct W(Copyable):
    var v: Int

    @implicit
    def __init__(out self, v: Int):
        print("conv", v)
        self.v = v


def bt(mut t: Tuple[Int, Int]) -> Int:
    t[0] += 100
    return 0


def bt2(mut t: Tuple[Tuple[Int, Int], Int]) -> Int:
    t[0][1] += 100
    return 0


def bp(mut p: P) -> Int:
    p.t[0] += 100
    return 0


def bump(mut s: S) -> Int:
    s.v += 100
    return 0


def bumpv(mut s: V) -> Int:
    s.v += 100
    return 0


def bs(mut t: Tuple[String, Int]) -> Int:
    t[0] += "!"
    return 0


def pick(ref t: Tuple[Int, Int]) -> ref[t] Int:
    return t[0]


def ix() -> Int:
    print("ix")
    return 1


def two(a: Int, b: Int):
    print("two", a, b)


def twos(a: String, b: Int):
    print("twos", a, b)


def twov(var a: String, b: Int):
    print("twov", a, b)


def tw(a: W, b: Int):
    print("tw", a.v, b)


def main() raises:
    var tup = (3, 4)
    two(tup[0], bt(tup))
    var s = S(3)
    two(s[0], bump(s))
    two(s.get(), bump(s))
    var p = P((3, 4))
    two(p.t[0], bp(p))
    var tt = ((1, 2), 5)
    two(tt[0][1], bt2(tt))
    var q = (3, 4)
    two(pick(q), bt(q))
    var r = (3, 4)
    print(r[0] + bt(r))
    var u = (String("ab"), 4)
    twos(u[0], bs(u))
    twov(u[0], bs(u))
    var w = (3, 4)
    tw(w[0], bt(w))
    var k = (3, 4)
    two(b=bt(k), a=k[0])
    var v = V(3)
    two(v[ix()], bumpv(v))
    var d = Dict[String, Int]()
    d["x"] = 3
    d["y"] = 4
    print(d["x"] + d["y"])
