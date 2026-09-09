# A parametric-mut origin iterator (`m: Bool, //, o: Origin[mut=m]`) over a
# mutable named source: the loop site resolves the yielded reference's
# mutability from the source, so `for ref` writes through into the source,
# observed after the loop. The source is stored through a `Pointer[T, Self.o]`
# field.
from std.iterable import StopIteration


struct NumbersIter[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

    def __next__(mut self) raises StopIteration -> ref[Self.o] Int:
        if self.index >= len(self.src[]):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[][r]


struct Numbers:
    var items: List[Int]

    def __init__(out self):
        self.items = [4, 5, 6]

    def __iter__(ref self) -> NumbersIter[origin_of(self.items)]:
        ref items = self.items
        return NumbersIter(items, 0)


def main():
    var nums = Numbers()
    for ref x in nums:
        x += 10
    var total = 0
    for y in nums:
        total += y
    print(total)
    var doubled = [x + x for ref x in nums]
    print(doubled[0], doubled[2])
