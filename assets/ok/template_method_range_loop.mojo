# A per-instantiation method clone inherits its checked template's facts when
# the body loops, or builds a comprehension, over `range(...)`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, features
# `direct_calls` and `iteration`). The call selects an overload of a closed
# module function from closed scalar arguments, which no instance changes,
# and the protocol is selected again from the range's own type.


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def total(self, n: Int) -> Int:
        var s = 0
        for i in range(n):
            s += i
        return s

    def spans(self, lo: Int, hi: Int) -> Int:
        var s = 0
        for i in range(lo, hi):
            s += i
        for j in range(lo, hi, 2):
            s += j * 10
        return s

    def indexed(self) -> Int:
        var s = 0
        for i in range(len(self.items)):
            s += i + 1
        return s

    def squares(self, n: Int) -> List[Int]:
        return [i * i for i in range(n) if i != 1]


def main():
    var ints = Shelf[Int]()
    ints.items.append(1)
    var words = Shelf[String]()
    words.items.append("x")
    words.items.append("y")
    print(ints.total(4), words.total(5))
    print(ints.spans(1, 6), words.spans(0, 4))
    print(ints.indexed(), words.indexed())
    print(len(ints.squares(3)), len(words.squares(4)))
