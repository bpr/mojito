# PROBE (divergence): `==` through an `Equatable` bound on a struct whose own
# `__eq__` takes another type.
#
# `Money` declares `__eq__(self, other: Cents)` and conforms to `Equatable`.
# The pin takes `Equatable`'s default, the fieldwise comparison, as the
# witness, so `Pair[Money].same()` prints True; a direct `Money == Money`
# converts the operand and calls the declared `__eq__`. Mojito has no such
# default: the template's `==` names the declared `__eq__`, whose parameter
# is `Cents`, and elaboration stops with "argument 0 of 'Money.__eq__' has
# type Money, declared Cents". Filed in `docs/roadmap.md` §3 ("`==` through
# a bound reaches an `__eq__` over another type"). When Mojito prints
# True, promote this file to `assets/ok/`.
#
# Run:    mojo run equatable_witness_of_another_type.mojo
#         cargo run -- run conformance/probes/equatable_witness_of_another_type.mojo
@fieldwise_init
struct Money(Copyable, Deinitable, Equatable, ImplicitlyCopyable, Movable):
    var cents: Int

    def __eq__(self, other: Cents) -> Bool:
        return False


struct Cents(Copyable, Deinitable, ImplicitlyCopyable, Movable):
    var amount: Int

    def __init__(out self, amount: Int):
        self.amount = amount

    @implicit
    def __init__(out self, m: Money):
        self.amount = m.cents


struct Pair[T: Copyable & Equatable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = first^
        self.second = second^

    def same(self) -> Bool:
        return self.first == self.second


def main():
    var b = Pair[Money](Money(4), Money(4))
    print(b.same())
