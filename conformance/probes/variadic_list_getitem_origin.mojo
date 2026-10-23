# PROBE: upstream's `VariadicList.__getitem__` result origin, the union of
# the elements' origin and the list's, cast to the elements' mutability.
#
# **Differs.** The pin prints `3`. Mojito rejects the signature with
# "unsupported origin contract" (`checker/origins/sig.rs`), so the bundled
# `VariadicList.__getitem__` returns `ref[origin_of(self)]` (R1299).
struct Box[mut: Bool, //, origin: Origin[mut=mut]]:
    var p: Pointer[Int, Self.origin]

    def __init__(out self, ref[Self.origin] v: Int):
        self.p = Pointer(to=v)

    def get[self_origin: ImmOrigin](ref[self_origin] self) -> ref[
        origin_of(Self.origin, self_origin).unsafe_mut_cast[Self.mut]()
    ] Int:
        return self.p[]


def main():
    var x = 3
    var b = Box(x)
    print(b.get())
