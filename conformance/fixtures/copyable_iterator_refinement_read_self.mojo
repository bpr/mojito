trait IteratorContract:
    comptime Element: Movable

    def __next__(self) -> Self.Element:
        ...

@fieldwise_init
struct ReadIterator(IteratorContract):
    comptime Element = Int

    var value: Int

    def __next__(self) -> ref[origin_of(self.value)] Int:
        return self.value

def take[I: IteratorContract](iterator: I) -> I.Element:
    return iterator.__next__()

def main():
    var iterator = ReadIterator(7)
    print(take(iterator))
