# A per-instantiation method clone inherits its checked template's facts when
# an operator's left operand is a literal or a closed scalar
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `operator_dispatch`). The left operand has no dunder for the pair, so the
# operator dispatches the right operand's reflected dunder, which an
# instance names again at its own types.
struct Meter[T: Copyable & Deinitable & Movable](Copyable, Deinitable, ImplicitlyCopyable, Movable):
    var count: Int

    def __init__(out self, count: Int):
        self.count = count

    def __add__(self, other: Self) -> Self:
        return Meter[Self.T](self.count + other.count)

    def __radd__(self, other: Int) -> Self:
        return Meter[Self.T](other + self.count)

    def __rmul__(self, other: Float64) -> Float64:
        return other * Float64(self.count)

    # An overloaded reflected dunder: the operator names the overload key.
    def __rsub__(self, other: Int) -> Int:
        return other - self.count

    def __rsub__(self, other: Float64) -> Float64:
        return other - Float64(self.count)


struct Gauge[T: Copyable & Deinitable & Movable](Movable):
    var low: Meter[Self.T]
    var base: Int

    def __init__(out self, var low: Meter[Self.T]):
        self.low = low^
        self.base = 4

    def raised(self) -> Meter[Self.T]:
        return 1 + self.low

    def scaled(self) -> Float64:
        return 2.5 * self.low

    def margin(self) -> Int:
        return 10 - self.low

    def fraction(self) -> Float64:
        return 0.5 - self.low

    # A closed scalar parameter or field as the left operand.
    def shifted(self, n: Int) -> Meter[Self.T]:
        return n + self.low

    def based(self) -> Int:
        return self.base - self.low

    # A reflected operator's value is the left operand of a forward one.
    def doubled(self) -> Meter[Self.T]:
        return (1 + self.low) + self.low


def main():
    var g = Gauge[Int](Meter[Int](1))
    var h = Gauge[String](Meter[String](3))
    print(g.raised().count, h.raised().count, g.scaled(), h.scaled())
    print(g.margin(), h.margin(), g.fraction(), h.fraction())
    print(g.doubled().count, h.doubled().count)
    print(g.shifted(2).count, h.shifted(2).count, g.based(), h.based())
