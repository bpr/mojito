# `Set(a, b, c)` selects the standard-library variadic constructor (its
# `__set_literal__` marker defaults, so an explicit call need not supply it) and
# infers the element type from the arguments. A zero-argument `Set[Int]()`
# prefers the empty constructor over the variadic one. Duplicate arguments
# collapse, matching set semantics.

from std.collections.set import Set

def main():
    var s = Set(1, 2, 3)
    print(len(s))
    var duplicates = Set(1, 2, 2, 3)
    print(len(duplicates))
    var empty = Set[Int]()
    print(len(empty))
