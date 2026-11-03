# The bundled collections' handler APIs take their closure through a
# closure-trait binder, as upstream's do: a nested `def`, capturing or not,
# passed to `List`/`Set`/`Dict.deinit_with`, `Set`/`Dict.clear_with`, and
# `Optional.map`/`and_then`/`deinit_with`/`init_with=`, including a
# reference-returning method on the temporary such a call returns.
from std.collections import Dict, Optional, Set


def make() -> Int:
    return 7


def main():
    var total = 0

    def add(var e: Int) {mut total}:
        total += e

    def add_product(var k: Int, var v: Int) {mut total}:
        total += k * v

    var l = [1, 2, 3]
    l^.deinit_with(add)
    print(total)

    var s = Set[Int]()
    s.add(100)
    s.clear_with(add)
    print(total, len(s))
    s.add(1000)
    s^.deinit_with(add)
    print(total)

    var d = Dict[Int, Int]()
    d[2] = 3
    d.clear_with(add_product)
    print(total, len(d))
    d[4] = 5
    d^.deinit_with(add_product)
    print(total)

    var o = Optional(10)
    o^.deinit_with(add)
    print(total)

    def double(var e: Int) -> Int:
        return e * 2

    def half(var e: Int) -> Optional[Int]:
        if e % 2 == 0:
            return e // 2
        return None

    var k = 3

    def times(var e: Int) {imm k} -> Int:
        return e * k

    print(Optional(21).map(double).value())
    print(Optional(21).map(times).value())
    print(Optional(42).and_then(half).value())
    print(Bool(Optional(41).and_then(half)))
    print(Optional[Int](init_with=make).value())
