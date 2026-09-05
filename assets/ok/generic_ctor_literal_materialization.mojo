# A literal argument to a generic constructor materializes to the solved
# parameter type on every constructor path — the explicit application, the
# inferred one, a fieldwise constructor, and an overloaded one — so a
# `Float64` field stores `2.5`, never the exact literal; an integer literal
# against an explicitly bound `Float64` parameter widens the same way.
struct Box[T: Copyable & Deinitable](Copyable, Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var left: Self.T
    var right: Self.T


struct Many[T: Copyable & Deinitable](Copyable, Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def __init__(out self, var value: Self.T, twice: Bool):
        self.value = value^


def more():
    var p = Pair(1.5, 2.5)
    print(p.left, p.right)
    var q = Pair[Float64](0.5, 1)
    print(q.left, q.right)
    var m = Many(7.5, True)
    print(m.value)
    var n = Many[Float64](8.5)
    print(n.value)


def main():
    var a = Box[Float64](2.5)
    print(a.value)
    var b = Box(2.5)
    print(b.value)
    var c = Box[Int](3)
    print(c.value)
    var d = Box(4)
    print(d.value)
    more()
