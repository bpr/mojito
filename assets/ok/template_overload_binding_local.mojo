# A call inside a generic body is bound once, while the body is checked with
# `T` symbolic, and a scalar local does not change that: `outer` derives from
# its checked template, so every instance inherits `pick[T]`. The pinned Mojo
# prints 2 for each line below; re-checking the `Int` instance used to rank
# the set again and pick `pick(x: Int)`.
def pick(x: Int) -> Int:
    return 1


def pick[T: Copyable](x: T) -> Int:
    return 2


def outer[T: Copyable](x: T) -> Int:
    var selected = pick(x)
    return selected


def main():
    print(outer(3))
    print(outer(True))
