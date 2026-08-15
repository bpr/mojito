# expect: invalidated interior reference
# A write through a mutable subtree reference invalidates that reference
# after its first write (current Mojo's rule): the first store succeeds,
# and any later use — read or write — rejects.
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
    p[] = 9
    print(p[])
