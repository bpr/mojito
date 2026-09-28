# A variadic struct's index-keyed accessors, one copying the element out of
# its `Tuple` storage and one handing out a reference to it, derive each
# unrolled `__getitem__$k` from the checked template: the instance selects
# the generated Tuple's accessor for its position where the template
# requested `Tuple.__getitem_param__[i]`.
struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()


struct Shelf[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](ref self) -> ref[self.storage] Self.Ts[i]:
        return self.storage[i]


def main():
    var b = Bag[Int, String](1, "a")
    var n: Int = b.__getitem__[0]()
    var s: String = b.__getitem__[1]()
    print(n, s)
    var c = Bag[Bool, Int](True, 2)
    print(c.__getitem__[0](), c.__getitem__[1]())
    var shelf = Shelf[Int, String](3, "b")
    ref first = shelf.__getitem__[0]()
    first += 4
    print(shelf.__getitem__[0](), shelf.__getitem__[1]())
