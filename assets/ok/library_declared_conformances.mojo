# A bundled library type conforms to the traits upstream's library declares
# for it, conditions included: `List[T]` is `Equatable` where `T` is and
# always `Defaultable`, `String` iterates through `__iter__` without being
# `Iterable`, and a `Comparable` declaration is an `Equatable` one.
from std.collections import Set
from std.hash import Hasher
from std.pathlib import Path


@fieldwise_init
struct Rank(Comparable, Copyable, Hashable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v

    def __le__(self, other: Self) -> Bool:
        return self.v <= other.v

    def __gt__(self, other: Self) -> Bool:
        return self.v > other.v

    def __ge__(self, other: Self) -> Bool:
        return self.v >= other.v

    def __hash__(self, mut hasher: Some[Hasher]):
        hasher._update_with_simd(Int64(self.v))


def traits_of[T: AnyType]() -> String:
    var s = String("")
    comptime if conforms_to(T, Equatable):
        s += "Equatable "
    comptime if conforms_to(T, Comparable):
        s += "Comparable "
    comptime if conforms_to(T, Defaultable):
        s += "Defaultable "
    comptime if conforms_to(T, Iterable):
        s += "Iterable "
    comptime if conforms_to(T, IterableOwned):
        s += "IterableOwned "
    comptime if conforms_to(T, IntableRaising & FloatableRaising):
        s += "Parses "
    return s


def traits_of_value[T: AnyType](value: T) -> String:
    return traits_of[T]()


def same[T: Equatable & Copyable](a: T) -> Bool:
    return a == a


def make[T: Defaultable]() -> T:
    return T()


def main() raises:
    print(traits_of[List[Int]]())
    print(traits_of[String]())
    print(traits_of_value(StringSpan("12")))
    print(traits_of[Set[Int]]())
    print(traits_of[Path]())
    print(traits_of[Rank]())

    var text = String("héllo")
    for grapheme in text:
        print(grapheme, end="|")
    print()

    print(same(Rank(9)), (Rank(2), 1).__lt__((Rank(3), 1)))
    var ranks = Dict[Rank, Int]()
    ranks[Rank(1)] = 5
    print(len(ranks), len(make[List[Int]]()), len(make[Dict[Int, Int]]()))
    print(Int(StringSpan("12")), Float64(StringSpan("2.5")))
    var empty = make[StringSpan[ImmStaticOrigin]]()
    print(empty.byte_length(), Bool(empty))
