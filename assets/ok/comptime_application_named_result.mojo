# A compile-time call of a function with a named `out` result is the
# function's application, whose result is the `out` parameter's type, as a
# `->` result's is. A result typed by the positional collector reads it.

from std.utils.index import Index, IndexList


@fieldwise_init
struct B(ImplicitlyCopyable, Movable):
    var x: Int


def mk(a: Int, out result: B):
    result = B(a)


def pair(a: Int, b: Int, out result: IndexList[2]):
    result = IndexList[2](a, b)


def count[*Ts: Intable](*args: *Ts, out result: Int):
    result = args.__len__()


def h[b: B]():
    print(b.x)


def g[l: IndexList[2]]():
    print(l[0], l[1])


def n[k: Int]():
    print(k)


def main():
    h[mk(5)]()
    g[pair(3, 4)]()
    g[Index(5, 6)]()
    n[count(1, 2, 3)]()
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v.shuffle[mask=Index(3, 2, 1, 0)]())
