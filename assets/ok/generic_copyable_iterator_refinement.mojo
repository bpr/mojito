@fieldwise_init
struct StopIteration:
    pass

trait IteratorContract:
    comptime Element: Movable

    def __next__(mut self) raises StopIteration -> Self.Element:
        ...

trait IterableIteratorContract(IteratorContract):
    comptime Element: ImplicitlyCopyable & ImplicitlyDeletable

    def __iter__(ref self) -> Self:
        ...

@fieldwise_init
struct IntRefIter(
    ImplicitlyCopyable, ImplicitlyDeletable, IterableIteratorContract, Movable
):
    comptime Element = Int

    var value: Int
    var done: Bool

    def __iter__(ref self) -> Self:
        return self

    def __len__(self) -> Int:
        if self.done:
            return 0
        return 1

    def __next__(mut self) raises StopIteration -> ref[origin_of(self.value)] Int:
        if self.done:
            raise StopIteration()
        self.done = True
        return self.value

def first[I: IterableIteratorContract & ImplicitlyDeletable](
    var iterator: I
) raises StopIteration -> I.Element:
    for item in iterator:
        return item
    raise StopIteration()

def main():
    try:
        var iterator = IntRefIter(42, False)
        print(first(iterator^))
    except StopIteration:
        pass
