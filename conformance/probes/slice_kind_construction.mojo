# PROBE: `ContiguousSlice` and `StridedSlice` construct explicitly.
#
# The pin runs it and prints:
#   Slice(1, 3, None) Slice(1, 5, 2) Slice(1, 4, None)
#   1 3
#   2 4
#   [1, 2] [1, 3] [1, 2, 3]
#
# Mojito rejects it: "Undefined variable
# '__module$std$builtin$builtin$slice$ContiguousSlice'" (R1157). The kinds
# are compiler intrinsics with no constructor, where upstream declares them
# as library structs in `builtin_slice.mojo`.
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice

def main():
    var c = ContiguousSlice(1, 3, None)
    var s = StridedSlice(1, 5, 2)
    var t: StridedSlice = Slice(1, 4)
    print(c, s, t)
    print(c.start.value(), c.end.value())
    print(s.indices(10)[2], t.indices(10)[1])
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[c], xs[s], xs[t])
