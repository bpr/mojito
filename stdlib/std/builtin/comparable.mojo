# Upstream's `std/builtin/comparable.mojo`: the `Equatable` and `Comparable`
# traits. `Equatable` defaults `__ne__` through `__eq__`, and `Comparable`
# defaults `__gt__`, `__le__`, and `__ge__` through `__lt__`. The prelude
# exports both, as upstream's does.
#
# Upstream's `__eq__` has a reflective default comparing every field; it
# stays a requirement here until its body can read each field through
# `field_ref` (R1481).


trait Equatable:
    def __eq__(self, other: Self, /) -> Bool:
        ...

    @inline(.always)
    def __ne__(self, other: Self, /) -> Bool:
        return not self == other


trait Comparable(Equatable):
    def __lt__(self, rhs: Self) -> Bool:
        ...

    @inline(.always)
    def __gt__(self, rhs: Self) -> Bool:
        return rhs < self

    @inline(.always)
    def __le__(self, rhs: Self) -> Bool:
        return not rhs < self

    @inline(.always)
    def __ge__(self, rhs: Self) -> Bool:
        return not self < rhs
