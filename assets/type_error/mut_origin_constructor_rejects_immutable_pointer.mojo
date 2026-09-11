# expect: a mutable-origin pointer for parameter 'o' of 'H'
# An `ImmOrigin(o)` pointer cannot fill a constructor parameter over a
# `MutOrigin` binder.
struct H[o: MutOrigin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, *, unsafe_ptr: Pointer[Int, Self.o]):
        self.p = unsafe_ptr


def main():
    var x = 7
    var q: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    var h = H(unsafe_ptr=q)
    print(h.p[])
