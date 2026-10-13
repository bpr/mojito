# A list, set, dictionary, or array display passed as a call argument is typed
# by the parameter it binds, so it is built as that argument's conversion, at
# the call's setup: its elements' reads and its construction run after every
# argument is evaluated, as the pinned Mojo builds it, and a later argument's
# write to an element's place is seen by the display. An element's own call
# still runs where written. A tuple display, a comprehension, a display inside
# a nested call or used as a receiver or subscript base, and the spelled
# `List` constructor keep their natural types and are built where written.
from std.collections import Set, Dict


struct A(Movable):
    var t: Int

    def __init__(out self, t: Int):
        self.t = t


@fieldwise_init
struct C(ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", copy.v)


struct S(Movable):
    var x: Int

    def __init__(out self, var l: List[Int], z: Int):
        self.x = l[0]

    def m(self, var l: List[Int], z: Int) -> Int:
        return l[0]


def bump(mut a: A) -> Int:
    a.t += 100
    return 0


def say(s: String) -> Int:
    print(s)
    return 0


def ha(var l: List[Int], z: Int) -> Int:
    return l[0]


def hr(l: List[Int], z: Int) -> Int:
    return l[0]


def hs(var s: Set[Int], z: Int) -> Int:
    var m = 0
    for x in s:
        if x > m:
            m = x
    return m


def harr(l: Array[Int, 2], z: Int) -> Int:
    return l[0]


def hd(d: Dict[Int, Int], z: Int) raises -> Int:
    return d[1]


def hll(l: List[List[Int]], z: Int) -> Int:
    return l[0][0]


def hc(l: List[C], z: Int) -> Int:
    return l[0].v


def two(x: Int, l: List[Int], z: Int) -> Int:
    return x * 1000 + l[0]


def gen[T: Writable](x: T, z: Int):
    print(x)


def fst(t: Tuple[Int, Int], z: Int) -> Int:
    return t[0]


def mk(x: Int) -> List[Int]:
    return [x, 1]


def wrap(var l: List[Int]) -> List[Int]:
    return l^


def id(x: Int) -> Int:
    return x


def main() raises:
    var a = A(5)
    print(ha([a.t, 1], bump(a)))
    a = A(5)
    print(hr([a.t, 1], bump(a)))
    a = A(5)
    print(hs({a.t, 1}, bump(a)))
    a = A(5)
    print(harr([a.t, 1], bump(a)))
    a = A(5)
    print(hd({1: a.t}, bump(a)))
    a = A(5)
    print(hd({a.t - 104: 7, 1: a.t}, bump(a)))
    a = A(5)
    print(hll([[a.t], [1]], bump(a)))
    a = A(5)
    print(ha(l=[a.t, 1], z=bump(a)))
    a = A(5)
    print(S([a.t, 1], bump(a)).x)
    a = A(5)
    var s = S([1], 0)
    print(s.m([a.t, 1], bump(a)))
    a = A(5)
    gen([a.t, 1], bump(a))
    a = A(5)
    print([a.t, 1], bump(a))
    a = A(5)
    print(two(a.t, [a.t, 1], bump(a)))
    a = A(5)
    print(ha([a.t, 1], bump(a)), ha([a.t, 2], bump(a)))
    var c = C(3)
    print(hc([c, C(4)], say("arg")))
    print(c.v)

    a = A(5)
    print(fst((a.t, 1), bump(a)))
    a = A(5)
    print(ha(mk(a.t), bump(a)))
    a = A(5)
    print(ha(wrap([a.t, 1]), bump(a)))
    a = A(5)
    print(ha(List[Int](a.t, 1, __list_literal__=None), bump(a)))
    a = A(5)
    print(ha([id(a.t), 1], bump(a)))
    a = A(5)
    print(ha([a.t + i for i in range(1)], bump(a)))
    a = A(5)
    print([a.t, 1].__len__() + bump(a))
    a = A(5)
    print([a.t, 1][0] + bump(a))
