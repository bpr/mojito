# The mutability gate stays honest after the pointer-deref return is accepted:
# an immutable-origin pointer (`o: Origin[mut=False]`) yields a read-only
# `ref[o] Int`, so writing through the returned reference is rejected.
# expect: must be mutable
@fieldwise_init
struct Borrow[o: Origin[mut=False]]:
    var p: UnsafePointer[Int, Self.o]

    def get(self) -> ref[Self.o] Int:
        return self.p[0]


def main():
    var v = 7
    var b = Borrow(UnsafePointer(to=v))
    ref w = b.get()
    w = 42
    print(v)
