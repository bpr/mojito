# A composition's traits are ordered by symbol path, not by spelling:
# `std.builtin.comparable.Comparable` sorts before `std.hash.hash.Hashable`
# whichever the bound names first.
# expect: 'x' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Comparable'
def f[T: Hashable & Comparable](var x: T):
    pass


def main():
    pass
