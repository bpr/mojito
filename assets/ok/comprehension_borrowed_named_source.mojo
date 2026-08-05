# A comprehension over a *named* user iterable borrows its source exactly like
# a `for` statement: the source is bound by reference (not copied/moved), stays
# usable after the comprehension, and its `__del__` runs exactly once at its
# ASAP last use — not the two drops a copy emits.
from std.iterable import StopIteration


@fieldwise_init
struct NumbersIter[o: Origin[mut=False]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[o] Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]


struct Numbers:
    var items: List[Int]

    def __init__(out self, n: Int):
        self.items = List[Int]()
        var i = 0
        while i < n:
            self.items.append(i * 10)
            i += 1

    def __del__(deinit self):
        print(-1)

    def __iter__(ref self) -> NumbersIter:
        ref items = self.items
        return NumbersIter(items, 0)


def main():
    print(-2)
    var nums = Numbers(3)
    var doubled = [x * 2 for x in nums]
    for d in doubled:
        print(d)
    print(len(nums.items))
    print(-3)
