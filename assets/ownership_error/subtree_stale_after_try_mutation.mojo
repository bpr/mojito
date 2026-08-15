# expect: invalidated interior reference
# Invalidations inside structured regions reach fall-through uses: a base
# mutation inside a `try` body stales the subtree reference for the code
# after the region.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._subtree
        ]()

    def grow(mut self):
        self.value += 1

def may_raise(flag: Bool) raises:
    if flag:
        raise Error("boom")

def main():
    var b = Buf(3)
    var p = b.view()
    try:
        may_raise(False)
        b.grow()
    except e:
        print("caught")
    print(p[])
