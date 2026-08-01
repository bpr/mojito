# expect: conflicts with live reference
# A user reference-yielding iterator borrows its *named* source for the duration
# of the loop (a live shared loan). Mutating that source while iterating — here a
# `mut self` method call that overwrites the borrowed storage — is rejected by the
# loan analysis, exactly as mutating a collection during iteration would be. Uses
# the bounded (`__len__`) protocol so the fixture needs no stdlib `StopIteration`.


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

    def clear(mut self):
        self.items = List[Int]()

    def __iter__(ref self) -> NumbersIter:
        ref items = self.items
        return NumbersIter(items, 0)


def main():
    var nums = Numbers(3)
    for x in nums:
        nums.clear()
        print(x)
