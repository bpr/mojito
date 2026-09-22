# A per-instantiation method clone inherits its checked template's facts when
# the body compares two places of the struct's parameter type
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `operator_dispatch`). The template proves the operator through the bound
# and records nothing at it; an instance decides the same operator on its own
# type: a scalar compares natively, and a struct dispatches the dunder its
# type declares, whose selection the operand types alone make.
struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def same(self) -> Bool:
        return self.first == self.second

    def differs(self, value: Self.T) -> Bool:
        return self.first != value

    def matches(self, value: Self.T) -> Int:
        var hits = 0
        if self.first == value:
            hits += 1
        if self.second == value:
            hits += 1
        return hits


struct Sorted[T: Copyable & Comparable & Deinitable](Movable):
    var low: Self.T
    var high: Self.T

    def __init__(out self, var low: Self.T, var high: Self.T):
        self.low = low^
        self.high = high^

    def ordered(self) -> Bool:
        return self.low < self.high

    def bounds(self, value: Self.T) -> Bool:
        return self.low <= value and value <= self.high


def main():
    var a = Pair[Int](1, 2)
    var b = Pair[String](String("x"), String("x"))
    print(a.same(), b.same(), a.differs(1), b.differs(String("y")))
    print(a.matches(2), b.matches(String("x")))
    var s = Sorted[Int](1, 5)
    var t = Sorted[String](String("a"), String("c"))
    print(s.ordered(), t.ordered(), s.bounds(3), t.bounds(String("d")))
