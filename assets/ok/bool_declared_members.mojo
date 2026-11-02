# `Bool` is upstream's struct in `std.builtin.bool`: `MIN`/`MAX`, the
# default initializer, and the `__bool__`, `__mlir_i1__`, `__int__`,
# `__float__`, and `__hash__` bodies run as declared members, through an
# instance, the type receiver, a conversion, a comptime value, and the
# `Boolable`/`Intable`/`Floatable`/`Hashable`/`Defaultable` bounds the
# declaration lists.
from std.collections import Dict, Set


def show[T: Intable & Floatable & Boolable](t: T):
    print(t.__int__(), t.__float__(), t.__bool__())


def make[T: Defaultable]() -> T:
    return T()


def main() raises:
    print(Bool.MIN, Bool.MAX)
    comptime m = Bool.MAX
    print(m)
    var t = True
    var f = False
    print(t.__int__(), f.__int__(), t.__float__(), f.__float__(), t.__bool__())
    print(Bool.__int__(False), Bool.__float__(True), Bool.__bool__(True))
    var b: Bool = Bool.__mlir_i1__(t)
    print(b)
    print(Int(t), Float64(f), UInt8(t))
    comptime k = Bool.MAX.__int__()
    comptime g = True.__float__()
    print(k, g)
    print(hash(True), hash(False))
    comptime h = hash(True)
    print(h)
    print(
        conforms_to(Bool, Hashable),
        conforms_to(Bool, Intable),
        conforms_to(Bool, Floatable),
        conforms_to(Bool, Boolable),
    )
    show(t)
    var d = Dict[Bool, Int]()
    d[False] = 1
    d[True] = 2
    print(d[False], d[True])
    var s = Set[Bool]()
    s.add(True)
    s.add(False)
    s.add(True)
    print(len(s))
    print(make[Bool]())
