# expect: invalidated interior reference
# A subtree origin is conservative: any mutation of its base — here a
# `mut self` method call — stales the generation, and the next use of the
# pointer rejects. (An interior-generation pointer would also stale here;
# the subtree form additionally stales on subfield mutation, pinned by the
# sibling fixture.)
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
    b.grow()
    print(p[])
