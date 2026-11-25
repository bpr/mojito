# expect: invalidated interior reference
# requires: stdlib
# A comprehension over a named user iterable borrows the owned interior its
# iterator's `__next__` yields references into, as a `for` statement does.
# Mutating the source while the comprehension iterates — here a `mut self`
# method call in the element expression — stales that interior, and the next
# element's use is rejected, as at the pin.


@fieldwise_init
struct NumbersIter[o: Origin[mut=False]]:
    var src: ref[o] List[Int]
    var index: Int

    def __next__(
        mut self,
    ) raises StopIteration -> ref[Self.o._get_owned_interior["element"]] Int:
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

    def bump(mut self) -> Int:
        self.items.append(99)
        return 0

    def __iter__(ref self) -> NumbersIter[origin_of(self.items)]:
        ref items = self.items
        return NumbersIter(items, 0)


def main():
    var nums = Numbers(3)
    var out = [x + nums.bump() for x in nums]
    print(len(out))
