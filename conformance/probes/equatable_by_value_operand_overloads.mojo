# PROBE: an `Equatable` conformer whose own `__eq__` and `__ne__` take their
# operand by value.
#
# **Differs.** The pin runs it and prints `False True`: neither method
# matches `Equatable`'s `other: Self, /` requirement, so the trait's own
# defaults (its reflective `__eq__` and `__ne__`) fill both, and the
# conformer's methods are overloads beside them. Mojito rejects it, "struct
# 'Coin' method '__ne__' does not match the signature required by trait
# 'Equatable'": a same-named method keeps the default out. Filed in
# `docs/roadmap.md` (R1616). When Mojito runs it, promote this file to
# `assets/ok/`.
#
# Observed 2026-10-28 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run equatable_by_value_operand_overloads.mojo


@fieldwise_init
struct Coin(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var value: Int

    def __eq__(self, var other: Coin) -> Bool:
        return self.value == other.value

    def __ne__(self, var other: Coin) -> Bool:
        return self.value != other.value

struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def same(self) -> Bool:
        return self.first == self.second

def main():
    var a = Pair[Int](1, 2)
    var b = Pair[Coin](Coin(3), Coin(3))
    print(a.same(), b.same())
