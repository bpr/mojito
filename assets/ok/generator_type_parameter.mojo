# A `__generator_type[...] T` parameter takes a generator value (a
# parameterized alias, a struct member, or a forwarded generator parameter)
# and applies it in its body, as upstream's `ParameterList.tabulate` and
# `reduce` take theirs.
comptime Twice[x: Int] = x * 2
comptime Sq[x: Int] = x * x
comptime One[a: Int] = 1
comptime Both[x: Int] = x > 2
comptime IsInt[T: AnyType] = T == Int
comptime Copies[T: AnyType] = conforms_to(T, Copyable)
comptime Gen = __generator_type[Idx: Int] Int


struct S:
    comptime k[i: Int]: Int = i + 100


def apply[G: Gen]() -> Int:
    return G[21]


def at_three[G: __generator_type[Idx: Int] Int]() -> Int:
    return G[3]


def twice[G: __generator_type[Idx: Int] Int]() -> Int:
    return at_three[G]() * 2


def test[P: __generator_type[Idx: Int] Bool]() -> Bool:
    return P[3]


def is_int[P: __generator_type[T: AnyType] Bool]() -> Bool:
    return P[Int]


struct Unique(Movable):
    pass


def holds_for_unique[P: __generator_type[T: AnyType] Bool]() -> Bool:
    return P[Unique]


def main():
    print(apply[Twice]())
    print(twice[Sq]())
    print(at_three[Sq]() + 1)
    print(at_three[One]())
    print(test[Both]())
    print(at_three[S.k]())
    comptime g = S.k
    print(at_three[g]())
    print(is_int[IsInt]())
    print(is_int[Copies](), holds_for_unique[Copies](), holds_for_unique[IsInt]())
