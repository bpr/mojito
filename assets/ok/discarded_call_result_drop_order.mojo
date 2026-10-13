# A non-raising call writes its result in place, so a result nothing uses
# dies at the call itself: ahead of the call's argument temporaries and of
# the owners whose last use is the call. That holds for free, method,
# infix, reflected, function-value, and generic calls, and for a `var`
# declaration that is never read. A raising call's result is taken after
# its argument cleanup, and `_ =` and an assignment are separate stores,
# so their arguments die first.
struct S(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __add__(self, o: S) -> S:
        return S(self.v + o.v)

    def __radd__(self, o: Int) -> S:
        return S(self.v + o)

    def __sub__(self, o: S) raises -> S:
        return S(self.v - o.v)

    def __deinit__(deinit self):
        print("del", self.v)


def mk(a: S, b: S) -> S:
    return S(a.v * b.v)


def mk1(a: S) -> S:
    return S(a.v * 10)


def mkr(a: S) raises -> S:
    return S(a.v * 10)


def gen[T: AnyType](a: S, b: S) -> S:
    return S(a.v * b.v + 1)


def main() raises:
    S(7).__add__(S(8))
    print("-- infix")
    S(1) + S(2)
    print("-- reflected")
    1 + S(18)
    print("-- free")
    mk(S(2), S(3))
    print("-- one argument")
    mk1(S(5))
    print("-- dying owner")
    var a = S(2)
    mk(a, S(3))
    print("-- nested")
    mk(mk(S(1), S(2)), S(3))
    print("-- chained infix")
    S(1) + S(2) + S(3)
    print("-- function value")
    var f = mk
    f(S(2), S(3))
    print("-- generic")
    gen[Int](S(2), S(3))
    print("-- dead declaration")
    var k = mk(S(5), S(6))
    print("-- dead declaration over an owner")
    var g = S(11)
    var m = mk(g, S(3))
    print("-- raising call")
    mkr(S(4))
    print("-- raising infix")
    S(9) - S(2)
    print("-- raising declaration")
    var h = mkr(S(4))
    print("-- discard")
    var d = S(8)
    _ = mk(d, S(9))
    print("-- assignment")
    var e = S(1)
    e = mk(S(2), S(3))
    print("end")
