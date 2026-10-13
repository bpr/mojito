# PROBE: `_ = two(p.n, q.n)` beside a `deinit self` destructor's call.
#
# **Differs.** The pin runs it (`del 21 / delP 3 / del 22 / delP 4 / two /
# del 1 / del 2 / two / 63`), and so does the VM. Natively the module fails
# LLVM verification, "Number or types of operands does not match intrinsic
# type", attributed to the `two(p.n, q.n)` line. Each half alone compiles:
# dropping the `Pair` lines, or the `two(p.n, q.n)` line, passes. Filed as
# R683 in `docs/roadmap.md`. When it compiles natively, fold it back into
# `assets/ok/by_value_argument_drop_order.mojo`.
#
# Observed 2026-10-13 against `Mojo 1.2.0.dev2026100406`.
#
# Run:    mojo run native_field_argument_beside_deinit_call.mojo

struct W(Movable):
    var t: Int

    def __init__(out self, t: Int):
        self.t = t

    def __deinit__(deinit self):
        print("del", self.t)

    def m(self, x: Int) -> Int:
        print("m")
        return self.t + x


def two(x: Int, y: Int) -> Int:
    print("two")
    return x + y


struct P(Movable):
    var w: W
    var n: Int

    def __init__(out self, var w: W, n: Int):
        self.w = w^
        self.n = n

    def __deinit__(deinit self):
        print("delP", self.n)


struct Pair(Movable):
    var a: W
    var b: W

    def __init__(out self):
        self.a = W(1)
        self.b = W(2)

    def __deinit__(deinit self):
        print(two(self.a.t, self.b.t) * 0 + 63)


def owners_and_fields():
    var p = P(W(21), 3)
    var q = P(W(22), 4)
    _ = two(p.n, q.n)
    var pair = Pair()
    _ = pair^


def main():
    owners_and_fields()
