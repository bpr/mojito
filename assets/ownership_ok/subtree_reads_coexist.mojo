# A subtree reference is stale-on-mutation, not exclusive: ordinary reads
# of the base and reads through the reference interleave freely.
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
    print(b.value)
    print(p[])
    print(b.value + p[])
