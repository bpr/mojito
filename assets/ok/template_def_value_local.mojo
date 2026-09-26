# A surviving trait-bound `def` whose body holds a whole value of a parameter
# type derives its instances from the checked template
# (`docs/notes/instantiation-from-template.md`, class FunctionBody): a local
# copied from a parameter, the `^` transfer of that local into another or
# into the result, the local handed by value to a direct call, and a runtime
# `for` over a parameter. A call inside a generic body is bound once, while the body is
# checked with `T` symbolic, so every instance inherits `pick[T]`: the pinned
# Mojo prints 2 for each `outer` line below, and re-checking the `Int` instance
# used to rank the set again and pick `pick(x: Int)`.
def pick(x: Int) -> Int:
    return 1


def pick[T: Copyable](x: T) -> Int:
    return 2


def tally[T: Copyable & Movable](items: List[T]) -> Int:
    var seen = 0
    for _ in items:
        seen += 1
    return seen


def outer[T: ImplicitlyCopyable & Deinitable](x: T) -> Int:
    var kept = x
    var moved = kept^
    return pick(moved)


def keep[T: ImplicitlyCopyable & Deinitable](x: T) -> T:
    var kept = x
    return kept^


def main():
    print(outer(3))
    print(outer(True))
    print(outer(String("s")))
    var numbers: List[Int] = [1, 2, 3]
    var words: List[String] = [String("a"), String("b")]
    print(tally[Int](numbers))
    print(tally[String](words))
    print(keep(5))
    print(keep(String("k")))
