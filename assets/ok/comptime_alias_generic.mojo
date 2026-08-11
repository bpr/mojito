# Generic top-level comptime aliases: declared once, lowered to a symbolic
# template, and expanded per application during type resolution — directly,
# through another alias, as a generic argument, and with a per-application
# value-parameter constraint.
comptime Pair[T: Copyable & Movable]: AnyType = Tuple[T, T]
comptime Second[T: Copyable & Movable]: AnyType = Pair[T]
comptime Guard[n: Int]: AnyType where (n > 0, "positive only") = Int


def first_of(pair: Pair[Int]) -> Int:
    return pair[0]


def main():
    var pair: Pair[Int] = (1, 2)
    print(first_of(pair))
    var nested: Second[Int] = (4, 5)
    print(nested[1])
    var wrapped: List[Pair[Int]] = [(6, 7)]
    print(len(wrapped))
    var guarded: Guard[3] = 8
    print(guarded)
