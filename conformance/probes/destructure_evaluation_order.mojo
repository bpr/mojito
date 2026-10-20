# PROBE: destinations evaluate left to right before the right-hand side, the RHS tuple is evaluated once, elements copy out of a temporary tuple (a `_` copies nothing), then the temporary is destroyed.
#
# The pin runs it and prints:
#   2 1
#   idx 0
#   idx 2
#   pair
#   10 20
#   2 7 20
#   del 8
#   del 9
#   assign
#   tpair
#   copy 1
#   copy 2
#   del 1
#   del 2
#   after 1 2
#   del 2
#   del 1
#   decl
#   tpair
#   copy 1
#   copy 2
#   del 1
#   del 2
#   after 1 2
#   del 2
#   del 1
#   move
#   tpair
#   copy 1
#   del 1
#   del 2
#   after 1
#   del 1
#   end
# Diagnostics:
#   warning at line 50: transfer from an owned value has no effect and can be removed
#   warning at line 41: assignment to 't1' was never used; assign to '_' instead?
#   warning at line 42: assignment to 't2' was never used; assign to '_' instead?
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects the nominal subscript target `xs[i]` until R1142 lands; the other forms run.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run destructure_evaluation_order.mojo
#         cargo run -- run conformance/probes/destructure_evaluation_order.mojo
def idx(i: Int) -> Int:
    print("idx", i)
    return i


def pair() -> Tuple[Int, Int]:
    print("pair")
    return (10, 20)


struct Tracked(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def tpair() -> Tuple[Tracked, Tracked]:
    print("tpair")
    return (Tracked(1), Tracked(2))


def main():
    var a = 1
    var b = 2
    (a, b) = (b, a)
    print(a, b)
    var xs: List[Int] = [0, 0, 0]
    (xs[idx(0)], xs[idx(2)]) = pair()
    print(xs[0], xs[2])
    var i = 0
    (i, xs[i]) = (2, 7)
    print(i, xs[0], xs[2])
    var t1 = Tracked(8)
    var t2 = Tracked(9)
    print("assign")
    (t1, t2) = tpair()
    print("after", t1.v, t2.v)
    print("decl")
    var (c, d) = tpair()
    print("after", c.v, d.v)
    print("move")
    var (e, _) = tpair()^
    print("after", e.v)
    print("end")
