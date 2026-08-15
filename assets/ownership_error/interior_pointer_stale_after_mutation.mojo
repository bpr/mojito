# expect: invalidated interior reference
# A multi-element pointer whose origin is a collection-owned interior
# generation stales when a mutating method on the owner starts a new
# generation — the `List.unsafe_ptr()` contract, exercised link-free so the
# raw ownership seam checks it too. (The rejection fires in the ownership
# analysis; the program never executes.)
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
    var p = b.view()
    b.grow()
    print(p[0])
