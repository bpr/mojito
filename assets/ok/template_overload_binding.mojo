# A call inside a generic body is bound once, while the body is checked with
# `T` symbolic: `pick(x)` can only select the generic overload, and every
# instance inherits that choice. The pinned Mojo prints 2 for each line below;
# re-checking the `Int` instance used to rank the set again and pick
# `pick(x: Int)`. `pick(5)` outside any generic body still selects it.
def pick(x: Int) -> Int:
    return 1


def pick[T: Copyable](x: T) -> Int:
    return 2


def outer[T: Copyable](x: T) -> Int:
    return pick(x)


def main():
    print(outer(3))
    print(outer(True))
    print(outer[Int](4))
    print(pick(5))
