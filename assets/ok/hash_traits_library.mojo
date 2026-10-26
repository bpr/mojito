# `Hashable` and `Hasher` are upstream's library traits in the bundled
# `std/hash`: the prelude exports `Hashable`, `Hasher` needs its import, and
# each conformance records the witness a call through the bound reaches.
from std.hash import Hasher, default_comp_time_hasher


@fieldwise_init
struct Point(Copyable, Equatable, Hashable):
    var x: Int
    var y: Int


# A generic witness of the `Some[Hasher]` requirement.
struct Tag(Hashable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __hash__[H: Hasher](self, mut hasher: H):
        (self.id * 3).__hash__(hasher)


# A hasher bound constructs, feeds, and finishes its hasher, as `hash` does.
def digest[H: Hasher & Defaultable & Deinitable, T: Hashable](value: T) -> UInt64:
    var hasher = H()
    value.__hash__(hasher)
    return hasher^.finish()


def same[T: Hashable](a: T, b: T) -> Bool:
    return hash(a) == hash(b)


def main():
    comptime Fnv = default_comp_time_hasher
    print(digest[Fnv](Point(2, 5)) == hash[HasherType=Fnv](Point(2, 5)))
    print(digest[Fnv](Tag(4)) == hash[HasherType=Fnv](12))
    print(digest[Fnv](7), hash[HasherType=Fnv](Tag(1)))
    print(same(Point(1, 2), Point(1, 2)), same(Point(1, 2), Point(2, 1)))
    print(same(Tag(9), Tag(9)), same(3, 4))
    var d = Dict[Point, String]()
    d[Point(1, 1)] = "one"
    print(Point(1, 1) in d, Point(0, 1) in d)
