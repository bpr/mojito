# A subtree pointer (`origin_of(self)._subtree`) held in a generic struct's
# parameter-typed field dereferences in place when a generic `def` returns
# it: `first(h)[]` binds the returned pointer to a hidden slot that keeps
# `a` alive, and the read goes through the handle, which still designates
# the exact field the pointer was minted from.
struct Holder[T: Copyable & Deinitable](Copyable):
    var item: Self.T

    def __init__(out self, item: Self.T):
        self.item = item.copy()

    def get(self) -> Self.T:
        return self.item.copy()


def first[T: Copyable & Deinitable](h: Holder[T]) -> T:
    return h.get()


@fieldwise_init
struct Buf(Copyable):
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._subtree
        ]()


def main():
    var a = Buf(3)
    var h = Holder(a.view())
    print(h.get()[])
    print(first(h)[])
    var r = first(h)
    print(r[], h.item[])
