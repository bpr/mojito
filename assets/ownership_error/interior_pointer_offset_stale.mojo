# expect: invalidated interior reference
# `unsafe_offset` preserves provenance: the offset pointer carries the same
# interior-generation loan as its source, so a mutation of the owner stales
# it too. (Link-free twin of the `List.unsafe_ptr()` contract; the rejection
# fires in the ownership analysis, so the program never executes.)
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[
        Int, origin_of(self)._get_owned_interior["items"]
    ]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._get_owned_interior["items"]
        ]()

    def grow(mut self):
        self.value += 1

def main():
    var b = Buf(3)
    var q = b.view().unsafe_offset(1)
    b.grow()
    print(q[])
