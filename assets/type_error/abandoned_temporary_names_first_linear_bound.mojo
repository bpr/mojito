# An abandoned temporary typed by a type parameter names the first trait of
# its bound's composition, ordered by symbol path, as upstream's
# `getMessageIfTraitIsLinear` does: `std.builtin.comparable.Comparable` sorts
# before `std.traits.copyable.Copyable`. The `<` reads `mk(a)` without
# owning it, so nothing destroys it.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Comparable'
def mk[T: Comparable & Copyable](a: T) -> T:
    return a.copy()


def g[T: Comparable & Copyable](a: T, b: T) -> Bool:
    return mk(a) < b


def main():
    print(g(1, 2))
