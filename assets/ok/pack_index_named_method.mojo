# A variadic struct's index-keyed method under a name other than
# `__getitem__`: `item[i]` copies its element out of the `Tuple` storage and
# `at[i]` hands out a reference to it. Each unrolls per element like the
# accessor, so `b.item[1]()` selects the element's own `item$1`.
struct Bag[*Ts: Copyable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def item[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()

    def at[i: Int](ref self) -> ref[self.storage] Self.Ts[i]:
        return self.storage[i]


def main():
    var b = Bag[Int, String](1, "a")
    print(b.item[1]())
    var c = Bag[Int, String, Float64](2, "b", 2.5)
    print(c.item[0](), c.item[1](), c.item[2]())
    ref first = c.at[0]()
    first += 4
    print(c.item[0](), c.at[1]())
