# A compile-time call applies an initializer over a value pack, as at the
# pin: `StaticTuple[SIMDLength, 2].__init__[*mask]()` forwards a caller's
# pack, and `StaticTuple[Int, 2].__init__[4, 1]()` binds the pack from its
# elements.
from std.utils import StaticTuple


def first[t: StaticTuple[SIMDLength, 2]]() -> Int:
    return Int(t[0])


def second[t: StaticTuple[Int, 2]]() -> Int:
    return t[1]


def via[*mask: SIMDLength]() -> Int:
    comptime tup = StaticTuple[SIMDLength, 2].__init__[*mask]()
    return first[tup]()


def main():
    print(via[3, 0]())
    print(second[StaticTuple[Int, 2].__init__[4, 1]()]())
    comptime pair = StaticTuple[Int, 2].__init__[6, 7]()
    print(second[pair]())
