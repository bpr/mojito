# A bare `ImmOrigin` / `MutOrigin` bound on an origin binder (upstream's
# `comptime ImmOrigin = Origin[mut=False]`), with the explicit
# `std.origin` import; the mutable binder writes through its pointer.
from std.origin import ImmOrigin, MutOrigin


struct Named[T: AnyType, o: ImmOrigin]:
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p


struct MutNamed[T: AnyType, o: MutOrigin]:
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p


def main():
    var x = 7
    var n = Named[Int, ImmOrigin(origin_of(x))](Pointer(to=x))
    print(n.p[])
    var m = MutNamed[Int, origin_of(x)](Pointer(to=x))
    m.p[] = 9
    print(x)
