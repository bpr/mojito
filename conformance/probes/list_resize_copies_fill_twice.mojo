# PROBE (divergence): `List.resize(n, fill)` copies `fill` twice per element.
#
# `List.resize`'s `unsafe_write(copy=fill)` lowers as a `var.use copy` of
# `fill` and then a `value.copy` of that copy, and drop elaboration destroys
# the first copy with a `drop.reg`. The pinned Mojo copies `fill` once per
# element.
#
# Observed 2026-10-24 against `Mojo 1.2.0.dev2026101705 (46f49943)`:
#   mojo:   copy 1 / copy 1 / del 1 / del 11 / del 11 / len 2 / end
#   mojito: copy 1 / copy 11 / del 11 / copy 1 / copy 11 / del 11 / del 1 /
#           del 21 / del 21 / len 2 / end
#
# When fixed: promote to `assets/ok`.
struct S(Copyable, Movable, Boolable):
    var n: Int
    var s: String
    def __init__(out self, n: Int):
        self.n = n
        self.s = String("v")
    def __init__(out self, *, copy: Self):
        print("copy", copy.n)
        self.n = copy.n + 10
        self.s = copy.s.copy()
    def __bool__(self) -> Bool:
        return self.n > 0
    def __deinit__(deinit self):
        print("del", self.n)

def main():
    var a = S(1)
    var l = List[S]()
    l.resize(2, a)
    print("len", len(l))
    print("end")
