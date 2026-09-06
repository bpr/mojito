# A generic struct's `Pointer[Self.T, Self.o]` field infers `T` from the
# pointee and binds `o` from the tracked argument (`Cell(Pointer(to=w))`); the
# explicit application `Cell[Int, origin_of(w)]` checks the pointer's
# provenance against the supplied origin. The stored pointer keeps its source
# lent, and writes through it are visible in the source.

struct Cell[T: Copyable & Deinitable, o: Origin[mut=True]]:
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p


def main():
    var w = 5
    var b = Cell(Pointer(to=w))
    print(b.p[])
    b.p[] = 7
    print(b.p[])
    print(w)
    var c = Cell[Int, origin_of(w)](Pointer(to=w))
    c.p[] = 9
    print(c.p[])
    print(w)
