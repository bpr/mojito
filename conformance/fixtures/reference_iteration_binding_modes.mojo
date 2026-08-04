from std.iterable import StopIteration


@fieldwise_init
struct Item(ImplicitlyCopyable, ImplicitlyDeletable, Movable):
    var value: Int


# A user iterator that borrows its source and yields a reference into it. The
# origin is immutable, so the yielded handle is a shared borrow of `Item`.
@fieldwise_init
struct ItemIter[o: Origin[mut=False]]:
    var src: ref[o] Item
    var done: Bool

    def __next__(mut self) raises StopIteration -> ref[o] Item:
        if self.done:
            raise StopIteration()
        self.done = True
        return self.src


struct Items:
    var value: Item

    def __init__(out self, value: Item):
        self.value = value

    def __iter__(ref self) -> ItemIter:
        ref v = self.value
        return ItemIter(v, False)


def main():
    # Immutable target over a reference result: bind the yielded handle and read
    # through it without copying.
    var imm = Items(Item(1))
    for item in imm:
        print("imm", item.value)

    # `var` target over a reference result: read through the handle and copy the
    # referent into owned storage. Mutating the copy leaves the source intact.
    var cop = Items(Item(2))
    for var item in cop:
        item.value += 100
        print("var", item.value)
    print("src", cop.value.value)

    # `ref` target over a shared (immutable-origin) reference result reads the
    # borrowed referent through the retained handle.
    var rf = Items(Item(3))
    for ref item in rf:
        print("ref", item.value)
