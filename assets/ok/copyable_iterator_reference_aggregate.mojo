trait IteratorContract:
    comptime Element: Movable

    def __next__(mut self) -> Self.Element:
        ...


struct RefBox[origin: Origin[mut=False]](
    Copyable, Deinitable, Movable
):
    var value: ref[origin] Int

    def __init__(out self, ref[Self.origin] value: Int):
        self.value = value

    def __init__(out self, *, copy: Self):
        print("copy", copy.value)
        self.value = copy.value


@fieldwise_init
struct RefIter[origin: Origin[mut=False]](
    Deinitable, IteratorContract
):
    comptime Element = RefBox

    var box: RefBox[Self.origin]

    def __next__(mut self) -> ref[origin_of(self.box)] RefBox:
        return self.box


def take[I: IteratorContract & Deinitable](
    var iterator: I
) -> I.Element:
    return iterator.__next__()


def main():
    var value = 41
    ref alias = value
    try:
        var iterator = RefIter(RefBox(alias))
        var copied = take(iterator^)
        print(copied.value)
    except error:
        print(error)
