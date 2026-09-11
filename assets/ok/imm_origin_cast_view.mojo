# `ImmOrigin(o)` (equivalently `Origin[mut=False](o)`) downcasts an origin
# to read-only while keeping its provenance: in a struct's origin argument,
# in an explicit construction, and in a `ref` result clause.
struct Cell[mut: Bool, //, origin: Origin[mut=mut]](Copyable, Movable):
    var p: Pointer[Int, Self.origin]

    def __init__(out self, ref [Self.origin] x: Int):
        self.p = Pointer(to=x)

    def __init__(out self, *, unsafe_ptr: Pointer[Int, Self.origin]):
        self.p = unsafe_ptr

    def read_only(self) -> Cell[ImmOrigin(Self.origin)]:
        return Cell[ImmOrigin(Self.origin)](unsafe_ptr=self.p)

    def read_only_spelled(self) -> Cell[Origin[mut=False](Self.origin)]:
        return Cell[Origin[mut=False](Self.origin)](unsafe_ptr=self.p)

    def first(self) -> ref[ImmOrigin(Self.origin)] Int:
        return self.p[]

    def get(self) -> Int:
        return self.p[]


def main():
    var x = 7
    var cell = Cell(x)
    print(cell.read_only().get(), cell.read_only_spelled().get(), cell.first())
