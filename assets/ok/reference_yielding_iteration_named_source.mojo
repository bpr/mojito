# A `for` loop over a user-defined reference-yielding iterator whose source is a
# *named* binding (`for x in nums`), not an owned temporary. The named source is
# borrowed (not copied/moved): its yielded references flow through the loop as
# handles, `nums` remains usable after the loop, and its `__del__` runs exactly
# once at enclosing-scope end (after the loop and the post-loop use).
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
    for x in nums:
        print(x)
    print(len(nums.items))
    print(-3)
