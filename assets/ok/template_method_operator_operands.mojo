# A per-instantiation method clone inherits its checked template's facts when
# an operator's operand is a literal, a call's result, or another operator
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `operator_dispatch`). A temporary operand records nothing in either check:
# the dunder moves it in, or reads it and drops it after. A literal stands
# beside a struct built over the parameter, whose dunder the template
# dispatched too, converting the literal where the parameter type asks.
struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def first_value(self) -> Self.T:
        return self.first.copy()

    def starts(self, value: Self.T) -> Bool:
        return self.first_value() == value


struct Meter[T: Copyable & Deinitable & Movable](Copyable, Deinitable, ImplicitlyCopyable, Movable):
    var count: Int

    @implicit
    def __init__(out self, count: Int):
        self.count = count

    def __add__(self, var other: Self) -> Self:
        return Meter[Self.T](self.count + other.count)

    def __sub__(self, other: Int) -> Self:
        return Meter[Self.T](self.count - other)

    def __eq__(self, other: Self) -> Bool:
        return self.count == other.count

    def __lt__(self, other: Float64) -> Bool:
        return Float64(self.count) < other


struct Gauge[T: Copyable & Deinitable & Movable](Movable):
    var low: Meter[Self.T]
    var high: Meter[Self.T]

    def __init__(out self, var low: Meter[Self.T], var high: Meter[Self.T]):
        self.low = low^
        self.high = high^

    # A literal the dunder's `Int` parameter takes as it stands, and one
    # an `@implicit` constructor converts into `Meter[Self.T]`.
    def lowered(self) -> Meter[Self.T]:
        return self.high - 1

    def bumped(self) -> Meter[Self.T]:
        return self.low + 5

    def is_two(self) -> Bool:
        return self.high == 2

    def below(self) -> Bool:
        return self.low < 1.5

    # A consuming dunder copies a place operand and moves a temporary one.
    def spanned(self) -> Meter[Self.T]:
        return self.low + self.high

    def sum(self) -> Meter[Self.T]:
        return self.low + self.high

    def chained(self) -> Meter[Self.T]:
        return self.low + (self.high + self.low)

    def balanced(self) -> Bool:
        return self.low + self.low == self.high + self.high

    def matches_sum(self) -> Bool:
        return self.sum() == self.low + self.high


def main():
    var p = Pair[Int](1, 2)
    var q = Pair[String](String("a"), String("b"))
    print(p.starts(1), p.starts(2), q.starts(String("a")), q.starts(String("b")))
    var g = Gauge[Int](Meter[Int](1), Meter[Int](2))
    var h = Gauge[String](Meter[String](3), Meter[String](6))
    print(g.lowered().count, h.lowered().count, g.bumped().count, h.bumped().count)
    print(g.is_two(), h.is_two(), g.below(), h.below())
    print(g.spanned().count, h.spanned().count, g.chained().count, h.chained().count)
    print(g.balanced(), h.balanced(), g.matches_sum(), h.matches_sum())
