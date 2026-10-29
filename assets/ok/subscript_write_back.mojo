# A value `__getitem__` result used as a mutable place is written back
# through `__setitem__` (get, modify, set): the base of a field store, a
# `mut self` receiver, the receiver of a nested setter, and a `mut`
# argument, over one index and over two. The getter runs after the
# operation's other operands, and a raising operation skips the store.


@fieldwise_init
struct P(Copyable, Movable, ImplicitlyCopyable):
    var x: Int

    def bump(mut self, n: Int):
        print("bump")
        self.x += n

    def risky(mut self, fail: Bool) raises:
        self.x += 1
        if fail:
            raise Error("boom")


struct S(Movable):
    var items: List[P]

    def __init__(out self):
        self.items = [P(0), P(10)]

    def __getitem__(self, i: Int) -> P:
        print("get", i)
        return self.items[i]

    def __setitem__(mut self, i: Int, value: P):
        print("set", i)
        self.items[i] = value


@fieldwise_init
struct Pair(Copyable, Movable, ImplicitlyCopyable):
    var a: Int
    var b: Int

    def __getitem__(self, i: Int) -> Int:
        if i == 0:
            return self.a
        return self.b

    def __setitem__(mut self, i: Int, value: Int):
        print("pair set", i)
        if i == 0:
            self.a = value
        else:
            self.b = value


struct Outer(Movable):
    var items: List[Pair]

    def __init__(out self):
        self.items = [Pair(0, 0)]

    def __getitem__(self, i: Int) -> Pair:
        print("outer get", i)
        return self.items[i]

    def __setitem__(mut self, i: Int, value: Pair):
        print("outer set", i)
        self.items[i] = value


struct Grid(Movable):
    var items: List[P]

    def __init__(out self):
        self.items = [P(0), P(10), P(20), P(30)]

    def __getitem__(self, i: Int, j: Int) -> P:
        print("grid get", i, j)
        return self.items[i * 2 + j]

    def __setitem__(mut self, i: Int, j: Int, value: P):
        print("grid set", i, j)
        self.items[i * 2 + j] = value


def rhs(v: Int) -> Int:
    print("rhs")
    return v


def idx(v: Int) -> Int:
    print("idx")
    return v


def bump(mut p: P, n: Int):
    print("bump arg")
    p.x += n


def bump_after(n: Int, mut p: P):
    print("bump after")
    p.x += n


def main():
    var s = S()
    s[0].x = 5
    print(s.items[0].x)
    s[idx(0)].bump(rhs(1))
    print(s.items[0].x)
    bump(s[idx(1)], rhs(2))
    print(s.items[1].x)
    bump_after(rhs(3), s[idx(1)])
    print(s.items[1].x)
    try:
        s[0].risky(False)
        s[0].risky(True)
    except e:
        print("caught", e)
    print(s.items[0].x)

    var o = Outer()
    o[0][1] = rhs(7)
    print(o.items[0].b)

    var g = Grid()
    g[1, 0].x = 7
    print(g.items[2].x)
    g[0, 1].bump(1)
    print(g.items[1].x)
    bump(g[1, 1], 100)
    print(g.items[3].x)
