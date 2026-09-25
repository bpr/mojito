# A per-instantiation method clone inherits its checked template's facts when
# the body holds a runtime `for` (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `iteration`): a loop over a field of `self`, over
# a sibling call's temporary result, over `self` through its own iterator,
# and an owned loop over a local. The protocol is selected from the
# iterable's type, which an instance substitutes and selects from again; the
# loop variable is a local of the body. A built-in scalar conversion of a
# closed value (`Int(key_hash)`) records only closed types.
from std.iter import StopIteration


@fieldwise_init
struct ShelfIter[T: Copyable & Equatable & Deinitable, o: Origin[mut=False]]:
    var src: Pointer[Shelf[Self.T], Self.o]
    var index: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.index >= self.src[].size():
            raise StopIteration()
        self.index += 1
        return self.index - 1


struct Shelf[T: Copyable & Equatable & Deinitable](Movable):
    var items: List[Self.T]
    var marks: List[Int]
    var buckets: Int

    def __init__(out self):
        self.items = List[Self.T]()
        self.marks = List[Int]()
        self.buckets = 4

    def size(self) -> Int:
        return len(self.items)

    def __iter__(ref self) -> ShelfIter[Self.T, origin_of(self)]:
        ref source = self
        return ShelfIter[Self.T, origin_of(self)](Pointer(to=source), 0)

    def positions(self) -> List[Int]:
        return self.marks.copy()

    def total(self) -> Int:
        var n = 0
        for mark in self.marks:
            n += mark
        return n

    def find(self, key: Self.T) -> Int:
        for index in self.positions():
            ref item = self.items[index]
            if item == key:
                return index
        return -1

    def count(self) -> Int:
        var n = 0
        for _ in self:
            n += 1
        return n

    def drained(self) -> Int:
        var marks = self.positions()
        var n = 0
        for var mark in marks^:
            n += mark * 2
        return n

    def bucket(self, key_hash: UInt64) -> Int:
        return Int(key_hash) & (self.buckets - 1)


def main():
    var ints = Shelf[Int]()
    ints.items.append(5)
    ints.items.append(9)
    ints.marks.append(0)
    ints.marks.append(1)
    var words = Shelf[String]()
    words.items.append("a")
    words.items.append("b")
    words.items.append("c")
    words.marks.append(2)
    print(ints.total(), words.total())
    print(ints.find(9), ints.find(4), words.find("c"))
    print(ints.count(), words.count())
    print(ints.drained(), words.drained())
    print(ints.bucket(7), words.bucket(13))
