# `StaticTuple`, upstream's fixed-size array over `!pop.array`, ported from
# `std/utils/static_tuple.mojo`: its constructors at run time and at compile
# time, `len`, `Indexer` subscripts and stores, `get`, the comparisons,
# `mlir_value=` construction, `_replace`, and a tuple as a compile-time
# value.
from std.utils import StaticTuple

comptime T = StaticTuple[Int, 3](7)


def show[t: StaticTuple[Int, 2]]():
    print(t[0], len(t))


def main():
    comptime t = StaticTuple[Int, 3](1, 2, 3)
    print(t[1], len(t))
    var u = StaticTuple[Int, 2].__init__[4, 5]()
    print(u[0], u[1], u.get[1]())
    var v = StaticTuple[Int, 3](7)
    print(v[2], v == v, v < t, t <= v)
    print(t[UInt8(1)], T[0], len(T))
    var a = StaticTuple[Int, 3](fill=4)
    a[1] = 8
    print(a[0], a[1], a[2])
    var e = StaticTuple[Int, 3]()
    e[0] = 1
    e[1] = 2
    e[2] = 3
    print(e[0], e[1], e[2])
    var x = StaticTuple[Int, 2](1, 2)
    var y = StaticTuple[Int, 2](1, 3)
    print(x == x, x != y, x < y, x <= y, y > x, y >= x, x > y)
    show[StaticTuple[Int, 2](fill=3)]()
    var z = StaticTuple[Int, 2](mlir_value=x._mlir_value)
    print(z[1], x._replace[0](9)[0])
