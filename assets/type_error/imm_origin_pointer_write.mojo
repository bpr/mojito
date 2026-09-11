# expect: cannot write through a Pointer with an immutable origin
# A pointer whose origin is `ImmOrigin(o)` is read-only even when `o`
# itself is mutable.
struct Cell[mut: Bool, //, origin: Origin[mut=mut]](Copyable, Movable):
    var p: Pointer[Int, Self.origin]

    def __init__(out self, ref [Self.origin] x: Int):
        self.p = Pointer(to=x)

    def poke(self, q: Pointer[Int, ImmOrigin(Self.origin)]):
        q[] = 3


def main():
    var x = 7
    print(x)
