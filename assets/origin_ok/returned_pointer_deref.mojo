# Dereferencing an origin-bearing `UnsafePointer` field and returning the
# reference (`self.p[0]`) executes: the pointer's origin is a struct parameter,
# so the returned `ref[o] Int` stays within that region. `UnsafePointer(to=v)`
# lowers to a handle straight at `v`, and the VM re-roots the returned reference
# at that surviving storage, forwarding the offset-0 index to the single pointee.
@fieldwise_init
struct Borrow[o: Origin[mut=False]]:
    var p: UnsafePointer[Int, Self.o]

    def get(self) -> ref[o] Int:
        return self.p[0]


def main():
    var v = 7
    var b = Borrow(UnsafePointer(to=v))
    print(b.get())
