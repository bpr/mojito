# expect: conflicts with live reference
# A comprehension over a named user iterable borrows its source with the same
# whole-place shared loan as a `for` statement. Mutating that source while the
# comprehension's borrowing iterator is live — here a `mut self` method call in
# the element expression — is rejected by the loan analysis. Uses the bounded
# (`__len__`) protocol so the fixture needs no stdlib `StopIteration`.


@fieldwise_init
struct NumbersIter[o: Origin[mut=False]]:
    var src: ref[o] List[Int]
    var index: Int

    def __len__(self) -> Int:
        return len(self.src) - self.index

    def __next__(mut self) -> ref[o] Int:
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

    def __iter__(ref self) -> NumbersIter:
        ref items = self.items
        return NumbersIter(items, 0)


def main():
    var nums = Numbers(3)
    var out = [x + nums.bump() for x in nums]
    print(len(out))
