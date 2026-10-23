# PROBE: a reference returned through a pointer a call yields
# (`return self.ptr()[unsafe_offset=i]`) names the pointee, as upstream's
# `VariadicList.__getitem__` returns `self._value.unsafe_ptr()[...][]`.
#
# **Differs.** The pin prints `2`. Mojito fails MIR verification ("register
# has no checked type" for the reference's `ref.make`, whose place is rooted
# at an untyped slot holding the call's result), so the bundled
# `VariadicList.__getitem__` binds the span's pointer before indexing it
# (R1305).
struct W:
    var p: UnsafePointer[Int, MutUntrackedOrigin]

    def __init__(out self, p: UnsafePointer[Int, MutUntrackedOrigin]):
        self.p = p

    def ptr(self) -> UnsafePointer[Int, MutUntrackedOrigin]:
        return self.p

    def get(self, i: Int) -> ref[MutUntrackedOrigin] Int:
        return self.ptr()[unsafe_offset=i]


def main():
    var l: List[Int] = [1, 2]
    var w = W(l.unsafe_ptr().unsafe_origin_cast[MutUntrackedOrigin]())
    print(w.get(1))
    _ = l^
