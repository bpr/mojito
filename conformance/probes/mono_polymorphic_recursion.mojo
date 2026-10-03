# Probe: expanding polymorphic recursion — each call demands `depth` at a
# strictly larger type.
#
# The pin (2026-10-02) runs out of memory (exit 137 after 49 s): its
# instantiation depth is unlimited by default. Mojito's erased run prints `0`
# in 8 s; its concrete run stops at the 1024-instance budget ("instantiation
# past the 1024-instance budget") in about 20 s in a debug build. The memory
# that keeps the budget at 1024 is docs/roadmap.md 2.2.
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
