# expect: invalidated interior reference
# A user iterator whose `__next__` yields references into its source's owned
# element interior, through a `Pointer` over an immutable origin: replacing
# the source's list while iterating stales the yielded reference, and its
# use is rejected, as at the pin.
struct NumbersIter[o: Origin[mut=False]](Iterator):
    comptime Element = Int
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] src: List[Int]):
        self.src = Pointer(to=src)
        self.index = 0

    def __next__(
        mut self,
    ) raises StopIteration -> ref[Self.o._get_owned_interior["element"]] Int:
        if self.index >= len(self.src[]):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[][r]


struct Numbers:
    var items: List[Int]

    def __init__(out self, n: Int):
        self.items = List[Int]()
        var i = 0
        while i < n:
            self.items.append(i * 10)
            i += 1

    def clear(mut self):
        self.items = List[Int]()

    def __iter__(ref self) -> NumbersIter[origin_of(self.items)]:
        return NumbersIter(self.items)


def main():
    var nums = Numbers(3)
    for x in nums:
        nums.clear()
        print(x)
