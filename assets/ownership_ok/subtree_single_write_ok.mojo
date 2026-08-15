# The first write through a mutable subtree reference is legal; only a
# use AFTER that write rejects. A single write with no later use passes,
# and the owner reads its updated storage afterwards.
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
    print(b.value)
