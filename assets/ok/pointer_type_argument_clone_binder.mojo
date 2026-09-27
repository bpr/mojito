# A `Pointer` type argument naming a caller place
# (`Layout[Pointer[Int, origin_of(x)]]`) bakes into its clone with a clone
# origin binder in place of the place, as an origin-slotted struct's tail
# does: `alloc`, `dealloc`, an explicit `unsafe_alloc[...]`, a user generic
# `def` and a generic struct's methods each get one clone for every place
# of the shape, and each call binds the binder from its own argument or
# explicit application. A baked pointer answers `copy()` as upstream's
# `Copyable` pointer does.
from std.memory import Layout, alloc, dealloc
from std.memory.alloc import unsafe_alloc


struct Holder[T: Copyable & Deinitable](Copyable):
    var item: Self.T

    def __init__(out self, item: Self.T):
        self.item = item.copy()

    def get(self) -> Self.T:
        return self.item.copy()


def first[T: Copyable & Deinitable](h: Holder[T]) -> T:
    return h.get()


def main():
    var x = 7
    var y = 9
    var a = alloc(Layout[Pointer[Int, origin_of(x)]](count=1))
    var b = alloc(Layout[Pointer[Int, origin_of(y)]](count=2))
    print(a.layout().count(), b.layout().count())
    dealloc(a^)
    dealloc(b^)

    var hx = Holder(Pointer(to=x))
    var hy = Holder(Pointer(to=y))
    var px = first(hx)
    var py = hy.get()
    print(px[], py[])
    px[] = 11
    py[] = 13

    var p = unsafe_alloc[Pointer[Int, origin_of(x)]](1)
    var q = unsafe_alloc[Pointer[Int, origin_of(y)]](2)
    p.unsafe_free()
    q.unsafe_free()
    print(x, y)
