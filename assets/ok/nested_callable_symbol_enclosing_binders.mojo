# A capture-free nested `def` or lambda declared over a type pack, a value
# pack, a callable pack, a callable binder, or a callable default is the
# symbol of its generator applied to every enclosing parameter, as at the
# pin: a pack to its list, a callable binder to the enclosing instance's
# function. Compile-time calls apply the same generator, and at a
# `def(...)` bound the body inflates to an empty type.
from std.sys import size_of


def g[h: def() thin -> Int]() -> Int:
    return h()


def apply[F: def() -> Int](f: F) -> Int:
    print(size_of[F]())
    return f()


def seven() -> Int:
    return 7


def nine() -> Int:
    return 9


def pack[*Ts: AnyType]():
    def k() -> Int:
        return len(Ts) * 10

    print(g[k]())
    comptime v = k()
    print(v)


def vp[*ns: Int]():
    def k() -> Int:
        var t = 0
        comptime for i in range(len(ns)):
            t += ns[i]
        return t

    print(g[k]())
    comptime v = k()
    print(v)


def fp[*fs: def() thin -> Int]():
    def k() -> Int:
        var t = 0
        comptime for i in range(len(fs)):
            t += fs[i]()
        return t

    print(g[k]())


def fwd[f: def() thin -> Int]():
    def k() -> Int:
        return f() + 1

    print(g[k]())
    comptime v = k()
    print(v)


def dflt[f: def() thin -> Int = seven]():
    def k(x: Int) -> Int:
        return f() + x

    comptime v = k(5)
    print(v)


def mixed[f: def() thin -> Int, *Ts: AnyType]():
    def k() -> Int:
        def j() -> Int:
            return f() + len(Ts)

        return g[j]() * 2

    print(g[k]())


struct W[*Ts: AnyType]:
    @staticmethod
    def m[f: def() thin -> Int]():
        def k() -> Int:
            return f() + len(Self.Ts)

        print(g[k]())


def bound_pack[*Ts: AnyType]():
    def k() -> Int:
        return len(Ts) + 40

    print(apply(k))


def bound_callable[f: def() thin -> Int]():
    def k() -> Int:
        return f() + 1

    print(apply(k))
    print(apply(lambda -> Int: f() + 2))


def main():
    pack[Int, String]()
    pack[Int]()
    vp[1, 2, 3]()
    vp[4]()
    fp[seven, nine]()
    fp[seven]()
    fwd[seven]()
    fwd[nine]()
    dflt()
    dflt[nine]()
    mixed[seven, Int, Bool]()
    mixed[nine]()
    W[Int, Bool, Int].m[nine]()
    bound_pack[Int, String]()
    bound_callable[seven]()
