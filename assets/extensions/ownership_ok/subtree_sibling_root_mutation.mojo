# Subtree staleness is rooted: mutating an unrelated owner leaves another
# owner's subtree reference live.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._subtree
        ]()

    def grow(mut self):
        self.value += 1

def main():
    var a = Buf(3)
    var b = Buf(7)
    var p = a.view()
    b.grow()
    print(p[])
