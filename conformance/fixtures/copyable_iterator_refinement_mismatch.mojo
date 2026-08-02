trait IteratorContract:
    comptime Element: Movable

    def __next__(mut self) -> Self.Element:
        ...

@fieldwise_init
struct MismatchedIterator(IteratorContract):
    comptime Element = Int

    var value: Bool

    def __next__(mut self) -> ref[origin_of(self.value)] Bool:
        return self.value

def main():
    pass
