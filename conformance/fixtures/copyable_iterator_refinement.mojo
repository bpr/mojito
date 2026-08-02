@fieldwise_init
struct StopIteration:
    pass

trait IteratorContract:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

struct Item(Copyable, ImplicitlyDeletable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value

    def __init__(out self, *, copy: Self):
        print("copy")
        self.value = copy.value

@fieldwise_init
struct ItemRefIter(ImplicitlyDeletable, IteratorContract):
    comptime Element = Item

    var value: Item
    var done: Bool

    def __next__(mut self) raises StopIteration -> ref[origin_of(self.value)] Item:
        if self.done:
            raise StopIteration()
        self.done = True
        return self.value

def take[I: IteratorContract & ImplicitlyDeletable](
    var iterator: I
) raises StopIteration -> I.Element:
    return iterator.__next__()

def main():
    try:
        var iterator = ItemRefIter(Item(41), False)
        var item = take(iterator^)
        print(item.value)
    except StopIteration:
        pass
