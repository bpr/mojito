# expect: invalidated interior reference
# Upstream's canonical example, in the pointer spelling: the first
# augmented write through a mutable subtree reference is legal, the second
# observes the self-invalidation and rejects.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._subtree
        ]()

def main():
    var b = Buf(3)
    var p = b.view()
    p[] += 1
    p[] += 2
    print(b.value)
