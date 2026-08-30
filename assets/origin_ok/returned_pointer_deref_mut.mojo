# A mutable-origin variant of the pointer-deref return: the origin parameter is
# `mut=True`, so `self.p[0]` yields a write-through `ref[Self.o] Int`. Binding it to a
# `ref` local and assigning writes through the re-rooted offset-0 handle to the
# caller's storage `v`.
@fieldwise_init
struct Borrow[o: Origin[mut=True]]:
    var p: UnsafePointer[Int, Self.o]

    def get(self) -> ref[Self.o] Int:
        return self.p[0]


def main():
    var v = 7
    var b = Borrow(UnsafePointer(to=v))
    print(b.get())
    ref w = b.get()
    w = 42
    print(v)
