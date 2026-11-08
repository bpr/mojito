# An instance method read through its type is a function value taking the
# receiver as its first argument, under the method's own convention: a
# read, `mut`, or `deinit` receiver; stored, passed, selected among
# overloads by the expected type, specialized by a bracket, read through
# `Self` or a parametric struct's instance, inherited from a trait's
# default, returning a reference, or raising. A call spelling the bracket
# (`S.gen[3](s)`) passes its first argument as the receiver too.
struct P[T: Copyable & Deinitable](Copyable):
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def get(self) -> Self.T:
        return self.v.copy()


trait Named:
    def name(self) -> String:
        return "named"


struct S(Copyable, Named):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x

    def add(self, d: Int) -> Int:
        return self.x + d

    def add(self, d: Bool) -> Int:
        return self.x + Int(d)

    def plus(self, d: Int, e: Int = 10) -> Int:
        return self.x + d + e

    def bump(mut self, d: Int):
        self.x += d

    def take(deinit self) -> Int:
        return self.x

    def gen[k: Int](self) -> Int:
        return self.x + k

    def first(self) -> ref[self.x] Int:
        return self.x

    def fails(self) raises -> Int:
        raise Error("boom")

    def via_self(self) -> Int:
        var f = Self.plus
        return f(self, 1)


def apply(f: def(S) thin -> Int, s: S) -> Int:
    return f(s)


def main() raises:
    var s = S(41)
    var h = S.get
    print(h(s))
    print(apply(S.get, s))
    var g: def(S, Int) thin -> Int = S.add
    print(g(s, 1))
    var b: def(mut S, Int) thin -> None = S.bump
    b(s, 5)
    print(s.x)
    var q = S.gen[3]
    print(q(s))
    print(S.gen[3](s))
    var t = S.take
    print(t(S(7)))
    var a = S.plus
    print(a(s, 2))
    print(a(s, 2, 3))
    print(a(s, d=2, e=4))
    print(s.via_self())
    var n = S.name
    print(n(s))
    var r = S.first
    print(r(s))
    var e = S.fails
    try:
        print(e(s))
    except err:
        print(err)
    var p = P[Int](5)
    var pg = P[Int].get
    print(pg(p))
