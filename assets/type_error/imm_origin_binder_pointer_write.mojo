# expect: cannot write through a Pointer with an immutable origin
# A pointer whose origin fills an `o: ImmOrigin` binder is read-only.
struct Named[T: AnyType, o: ImmOrigin]:
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p


def main():
    var x = 7
    var n = Named[Int, ImmOrigin(origin_of(x))](Pointer(to=x))
    n.p[] = 1
    print(x)
