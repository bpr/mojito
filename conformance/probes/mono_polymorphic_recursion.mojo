# Probe: expanding polymorphic recursion — each call demands `depth` at a
# strictly larger type.
#
# The pin (2026-10-02) runs out of memory (exit 137 after 49 s): its
# instantiation depth is unlimited by default. Mojito's erased run prints `0`
# in 8 s; its concrete run stops at the 4096-instance budget ("instantiation
# past the 4096-instance budget") in about 70 s and 380 MB in a debug build.
# The elaboration time, quadratic in the nesting depth, is docs/roadmap.md R19.
struct W[T: Copyable & Deinitable](Copyable, Deinitable):
    var v: Self.T
    def __init__(out self, var v: Self.T):
        self.v = v^

def depth[T: Copyable & Deinitable](x: T, n: Int) -> Int:
    if n == 0:
        return 0
    return depth(W[T](x.copy()), n - 1)

def main():
    print(depth(1, 3))
