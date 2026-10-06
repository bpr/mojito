# A struct keyed on a `DType` or on a vector value is a generator its
# template serves: its members are checked once with the value symbolic, and
# each instance's members, the sibling calls among them included, reuse those
# facts.
struct Tally[dt: DType](Copyable, Movable):
    var count: Int
    var total: Int

    def __init__(out self):
        self.count = 0
        self.total = 0

    def add(mut self, x: Int):
        self.count += 1
        self.total += x

    def add_twice(mut self, x: Int):
        self.add(x)
        self.add(x)

    def mean(self) -> Int:
        if self.count == 0:
            return 0
        return self.total // self.count


struct Salted[key: SIMD[DType.uint64, 2]](Copyable, Movable):
    var state: UInt64

    def __init__(out self):
        self.state = 7

    def mix(mut self, value: UInt64):
        self.state = (self.state ^ value) * 31

    def mix_pair(mut self, first: UInt64, second: UInt64):
        self.mix(first)
        self.mix(second)

    def digest(self) -> UInt64:
        return self.state


def main():
    var narrow = Tally[DType.int32]()
    narrow.add(4)
    narrow.add_twice(7)
    var wide = Tally[DType.float64]()
    wide.add_twice(10)
    print(narrow.count, narrow.total, narrow.mean())
    print(wide.count, wide.total, wide.mean())
    var salted = Salted[SIMD[DType.uint64, 2](1, 2)]()
    salted.mix_pair(3, 5)
    print(salted.digest())
