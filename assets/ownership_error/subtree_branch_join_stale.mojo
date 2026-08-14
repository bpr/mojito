# expect: invalidated interior reference
# Staleness joins across branches: a mutation on one path stales the
# subtree generation for every later use reachable from the join.
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
    if b.value > 2:
        b.grow()
    print(p[])
