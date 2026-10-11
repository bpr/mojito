# A program's own declarations live in its own module: a struct or a
# function it names like a library public shadows the prelude's in the
# program, and the library's own uses still reach the library's.


struct List(Copyable):
    var size: Int

    def __init__(out self, size: Int):
        self.size = size


struct Optional[T: Copyable & Deinitable](Copyable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, first: Self.T, second: Self.T):
        self.first = first.copy()
        self.second = second.copy()


struct Span[width: Int]:
    def __init__(out self):
        pass

    def area(self) -> Int:
        return Self.width * Self.width


struct Writer:
    var lines: Int

    def __init__(out self):
        self.lines = 0

    def write(mut self, text: String):
        self.lines += 1


struct Slice:
    var start: Int

    def __init__(out self, start: Int):
        self.start = start


def hash(n: Int) -> Int:
    return n * 3


def range(n: Int) -> Int:
    return n * 2


def next(n: Int) -> Int:
    return n + 1


def main():
    var mine = List(3)
    print(mine.size)
    print(range(4), next(7))
    var writer = Writer()
    writer.write("a")
    writer.write("b")
    print(writer.lines)
    var values = [10, 20, 30]
    print(len(values), values[1])
    var total = 0
    for value in values:
        total += value
    print(total)
    var pair = Optional[Int](1, 2)
    print(pair.first + pair.second)
    print(Span[5]().area())
    print(Slice(5).start, hash(5))
