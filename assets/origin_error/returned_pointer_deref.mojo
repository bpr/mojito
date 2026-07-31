# The deferred boundary: dereferencing an origin-bearing `UnsafePointer` field and
# returning the reference (`self.p[0]`) stays rejected. Unlike a `ref[origin]`
# aggregate field indexed through (which the runtime now re-roots), the pointer
# deref's place lowering keeps an offset-0 index the runtime cannot yet forward,
# so the checker keeps rejecting it rather than accept a program the VM would fail.
# expect: escapes storage
@fieldwise_init
struct Borrow[o: Origin[mut=False]]:
    var p: UnsafePointer[Int, Self.o]

    def get(self) -> ref[o] Int:
        return self.p[0]


def main():
    var v = 7
    var b = Borrow(UnsafePointer(to=v))
    print(b.get())
