# The experimental conservative origin form (current Mojo's
# `Origin._subtree`): a method can widen its returned pointer's provenance
# to "the receiver or anything beneath it" with origin_cast, and the
# runtime handle still designates the exact place it was minted from.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).origin_cast[
            origin_of(self)._subtree
        ]()

def main():
    var b = Buf(3)
    var p = b.view()
    print(p[])
