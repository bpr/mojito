# `ContiguousSlice` and `StridedSlice` are the library structs upstream's
# `builtin_slice.mojo` declares: each constructs explicitly, `StridedSlice`
# converts implicitly from a `Slice`, both write as `Slice(start, end,
# step)`, `ContiguousSlice` exposes its bounds and a two-element `indices`,
# and a value of either kind indexes a `List` through the overload taking it.
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice

def main():
    var c = ContiguousSlice(1, 3, None)
    var s = StridedSlice(1, 5, 2)
    var t: StridedSlice = Slice(1, 4)
    print(c, s, t)
    print(c.start.value(), c.end.value())
    print(s.indices(10)[2], t.indices(10)[1], c.indices(2)[1])
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[c], xs[s], xs[t])
