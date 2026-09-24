# A per-instantiation method clone inherits its checked template's facts when
# the body puts an operator over two places of the struct's parameter type
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `operator_dispatch`). The template proves the operator through the bound
# and records nothing at it; an instance decides the same operator on its own
# type: a scalar operates natively, and a struct dispatches the dunder its
# type declares, whose selection the operand types alone make. An instance
# that serves `!=` through `__eq__`, and one whose operator is an arithmetic
# one whose result is the operand's own type, are covered here too.
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


# A type that declares `__eq__` and no `__ne__` takes `Equatable`'s default
# `!=`: the instance dispatches `__eq__` and negates the result, an adjustment
# the template's `!=` over two symbolic places never recorded.
@fieldwise_init
struct Tag(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var id: Int

    def __eq__(self, other: Tag) -> Bool:
        return self.id == other.id


# An operator whose operands are built over the parameter rather than being it:
# the field's own `__add__` answers, and the result is a temporary of that type
# rather than a `Bool`.
struct Bag[T: Copyable & Deinitable & Movable](Copyable, Deinitable, ImplicitlyCopyable, Movable):
    var count: Int

    def __init__(out self, count: Int):
        self.count = count

    def __add__(self, other: Self) -> Self:
        return Bag[Self.T](self.count + other.count)


struct Totals[T: Copyable & Deinitable & Movable](Movable):
    var left: Bag[Self.T]
    var right: Bag[Self.T]

    def __init__(out self, var left: Bag[Self.T], var right: Bag[Self.T]):
        self.left = left^
        self.right = right^

    def merged(self) -> Bag[Self.T]:
        return self.left + self.right


def main():
    var a = Pair[Int](1, 2)
    var b = Pair[String](String("x"), String("x"))
    print(a.same(), b.same(), a.differs(1), b.differs(String("y")))
    print(a.matches(2), b.matches(String("x")))
    var s = Sorted[Int](1, 5)
    var t = Sorted[String](String("a"), String("c"))
    print(s.ordered(), t.ordered(), s.bounds(3), t.bounds(String("d")))
    var u = Pair[Tag](Tag(1), Tag(2))
    print(u.same(), u.differs(Tag(1)), u.matches(Tag(2)))
    var m = Totals[Int](Bag[Int](1), Bag[Int](2))
    var n = Totals[String](Bag[String](3), Bag[String](4))
    print(m.merged().count, n.merged().count)
