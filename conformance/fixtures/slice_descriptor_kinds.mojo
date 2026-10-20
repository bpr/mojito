# Upstream's slice literal binds a subscript parameter through the
# parameter type's `__slice_literal__` constructor: a literal without a step
# value passes `None` and so builds a `ContiguousSlice`, even when it spells
# the second colon, and only a written step builds a `StridedSlice`. Both
# kinds are imported from their module, as on the pin.
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice

struct Grid:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, part: ContiguousSlice) -> Int:
        return self.base + part.start.or_else(0)

    def __getitem__(self, part: StridedSlice) -> Int:
        return self.base + part.indices(100)[2] * 1000

def main():
    var g = Grid(7)
    print(g[2:5], g[2:5:3], g[::], g[2:5:], g[::-1])
