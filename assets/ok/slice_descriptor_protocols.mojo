# Slice descriptor protocols (upstream `builtin_slice.mojo`): `Slice` is
# Equatable and every descriptor kind writes as `Slice(start, end, step)`
# with `None` for an omitted bound; `ContiguousSlice.indices(length)` yields
# the two-element `(start, end)` while the strided family keeps three; and a
# `Slice`-typed descriptor value indexes a List through the normalizing
# `StridedSlice` overload (upstream's implicit `Slice -> StridedSlice`).
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice

struct Window:
    var size: Int

    def __init__(out self, size: Int):
        self.size = size

    def __getitem__(self, part: ContiguousSlice) -> Tuple[Int, Int]:
        print(part)
        return part.indices(self.size)

    def __getitem__(self, part: StridedSlice) -> Tuple[Int, Int, Int]:
        print(part)
        return part.indices(self.size)

def main():
    print(Slice(1, 4))
    print(slice(3))
    print(Slice(None, None, -1))
    print(Slice(1, 4) == Slice(1, 4, None))
    print(Slice(1, 4) != Slice(1, 4, None))
    print(Slice(1, 4) == slice(1, 5))
    var w = Window(5)
    print(w[1:4])
    print(w[-2:])
    print(w[::-1])
    print(w[1:5:2])
    var xs: List[Int] = [0, 1, 2, 3, 4]
    var s = Slice(None, None, -1)
    print(xs[s])
    print(xs[slice(1, 4)])
    print(xs[1:4])
