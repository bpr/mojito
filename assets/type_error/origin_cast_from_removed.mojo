# expect: was removed; use 'ImmOrigin(o)'
# `Origin[mut=False].cast_from[o]` does not exist at the pinned head; the
# capability cast is spelled `ImmOrigin(o)`.
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.p = Pointer(to=x)

    def first(self) -> ref[Origin[mut=False].cast_from[Self.o]] Int:
        return self.p[]


def main():
    var x = 4
    var c = Cell(x)
    print(c.first())
