# A `for` loop over a user-defined iterator whose `__next__` returns a *reference*
# into the borrowed source: the yielded reference flows through the loop as a
# handle (read here), not a copied value. The iterator (`NumbersIter`) holds a
# `ref[o] List[Int]` into the source; the loop invokes `__iter__`/`__next__` with
# the loop frame reachable, so that borrow resolves. The source temporary is
# destroyed exactly once after the loop.
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
    for x in Numbers(3):
        print(x)
    print(-3)
