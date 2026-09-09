# A parametric-mut origin iterator (`m: Bool, //, o: Origin[mut=m]`) over a
# mutable named source: the loop site resolves the yielded reference's
# mutability from the source, so `for ref` writes through into the source,
# observed after the loop.
from std.iterable import StopIteration


@fieldwise_init
struct NumbersIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[Self.o] Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]


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
