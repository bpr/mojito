# A pointer held in a generic struct's parameter-typed field dereferences in
# place: `h.item[]` reads through the field, and `h.get()[]` binds the
# returned pointer to a hidden slot that keeps `x` alive for the read. A
# pointer a method returns is an alias like `Pointer(to=x)`, so several
# copies of it coexist with the holder's own.
struct Holder[T: Copyable & Deinitable](Copyable):
    var item: Self.T

    def __init__(out self, item: Self.T):
        self.item = item.copy()

    def get(self) -> Self.T:
        return self.item.copy()


def main():
    var x = 7
    var h = Holder(Pointer(to=x))
    print(h.get()[])
    print(h.get()[])
    print(h.item[])
    var p = h.get()
    var q = h.get()
    print(p[], q[])
    p[] = 9
    print(h.item[], q[])
