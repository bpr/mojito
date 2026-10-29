@fieldwise_init
struct StopIteration:
    pass


struct Item(Deinitable, Movable):
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
    # A `ref` target over a value (rvalue) result designates no origin, so
    # transferring the item onward with `^` is rejected, as at the pin.
    var source = Values()
    for ref item in source:
        take(item^)
    print("done")
