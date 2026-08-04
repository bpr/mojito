@fieldwise_init
struct StopIteration:
    pass


struct Item(ImplicitlyDeletable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


@fieldwise_init
struct ValueIter:
    var done: Bool

    def __next__(mut self) raises StopIteration -> Item:
        if self.done:
            raise StopIteration()
        self.done = True
        return Item(1)


@fieldwise_init
struct Values:
    pass

    def __iter__(ref self) -> ValueIter:
        return ValueIter(False)


def take(var item: Item):
    print("take", item.value)


def main():
    # A `ref` target over a value (rvalue) result owns its per-iteration storage
    # well enough to transfer the item onward with `^`.
    var source = Values()
    for ref item in source:
        take(item^)
    print("done")
