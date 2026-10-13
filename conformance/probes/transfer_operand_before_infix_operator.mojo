# PROBE: a `^` transfer written directly before an infix operator.
#
# **Differs.** The pin lexes `c^ + d^` as `c ^ (+d)`, XOR over a unary plus,
# and rejects it: "'M' does not implement the '__pos__' method". Mojito
# parses two transfers and runs the program, printing 3. `(c^) + (d^)` is
# the spelling both accept. Filed in `docs/roadmap.md` as R741. When Mojito
# rejects it, delete this file.
struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __add__(var self, var other: Self) -> Int:
        return self.v + other.v


def main():
    var c = M(1)
    var d = M(2)
    var s = c^ + d^
    print(s)
