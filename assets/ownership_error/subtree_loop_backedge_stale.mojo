# expect: invalidated interior reference
# Staleness flows around loop backedges: a base mutation at the bottom of
# the body invalidates the read at the top of the next iteration.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).origin_cast[
            origin_of(self)._subtree
        ]()

    def grow(mut self):
        self.value += 1

def main():
    var b = Buf(3)
    var p = b.view()
    var i = 0
    while i < 2:
        print(p[])
        b.grow()
        i += 1
