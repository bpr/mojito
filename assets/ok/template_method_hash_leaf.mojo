# A per-instantiation method clone inherits its checked template's facts when
# the body hands a hasher a multi-lane vector no other body hashed
# (`docs/notes/instantiation-from-template.md`, class MethodBody). The leaf is
# a closed type, the same under every instance, so the template keeps it and
# each instance records it again: a tag vector and a seed pair, one in each
# arm of an `if`, derive for an `Int`, a `String`, and a user-struct instance.
from std.hashlib import Hasher


@fieldwise_init
struct Point(Copyable, Deinitable, Hashable, Movable):
    var x: Int
    var y: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)


struct Lanes[T: Copyable & Deinitable & Hashable](Hashable, Movable):
    var value: Self.T
    var tagged: Bool

    def __init__(out self, var value: Self.T, tagged: Bool):
        self.value = value^
        self.tagged = tagged

    def __hash__[H: Hasher](self, mut hasher: H):
        if self.tagged:
            hasher._update_with_simd(SIMD[DType.uint8, 4](1, 2, 3, 4))
        else:
            hasher._update_with_simd(SIMD[DType.uint16, 2](5, 6))
        self.value.__hash__(hasher)


def main():
    var ints = Lanes[Int](1, True)
    var other_ints = Lanes[Int](2, True)
    var untagged_ints = Lanes[Int](1, False)
    var strings = Lanes[String](String("a"), True)
    var untagged_strings = Lanes[String](String("a"), False)
    var points = Lanes[Point](Point(1, 2), True)
    var other_points = Lanes[Point](Point(2, 1), True)
    print(hash(ints) == hash(ints), hash(ints) == hash(other_ints))
    print(hash(ints) == hash(untagged_ints))
    print(hash(strings) == hash(strings), hash(strings) == hash(untagged_strings))
    print(hash(points) == hash(points), hash(points) == hash(other_points))
