# expect: must be mutable
# A read parameter is an immutable source: the loop site resolves the
# parametric-mut yielded reference to immutable, so writing through the
# `for ref` binding is rejected.
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


def bump(values: Numbers):
    for ref v in values:
        v += 1


def main():
    var nums = Numbers()
    bump(nums)
