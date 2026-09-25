# A per-instantiation method clone inherits its checked template's facts when
# the body constructs a closed `SIMD` or scalar-alias value
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `simd_constructions`). Inference records a construction's dtype and width
# only when both are closed, so the template's entry is every instance's: a
# presence tag handed to `_update_with_simd`, a seed handed to it through a
# local bound to a construction, and a construction passed by value to a
# sibling method all derive for an `Int`, a `String`, and a user-struct
# instance.
from std.hashlib import Hasher
from std.hashlib.hasher import default_hasher


@fieldwise_init
struct Point(Copyable, Deinitable, Hashable, Movable, Writable):
    var x: Int
    var y: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P", self.x, "/", self.y)


struct Tagged[T: Copyable & Deinitable & Hashable & Writable](Hashable, Movable):
    var value: Self.T
    var present: Bool

    def __init__(out self, var value: Self.T, present: Bool):
        self.value = value^
        self.present = present

    def __hash__[H: Hasher](self, mut hasher: H):
        if self.present:
            hasher._update_with_simd(UInt8(1))
            self.value.__hash__(hasher)
        else:
            hasher._update_with_simd(UInt8(0))

    def lanes[H: Hasher](self, mut hasher: H):
        var seed = UInt64(7)
        hasher._update_with_simd(seed)
        self.value.__hash__(hasher)

    def code(self, tag: UInt8) -> Int:
        return Int(tag)

    def tagged_code(self) -> Int:
        return self.code(UInt8(42))


def lanes_hash[T: Copyable & Deinitable & Hashable & Writable](value: Tagged[T]) -> UInt64:
    var hasher = default_hasher()
    value.lanes(hasher)
    return hasher^.finish()


def main():
    var some_int = Tagged[Int](3, True)
    var none_int = Tagged[Int](3, False)
    var some_str = Tagged[String](String("x"), True)
    var none_str = Tagged[String](String("x"), False)
    var some_point = Tagged[Point](Point(1, 2), True)
    var other_point = Tagged[Point](Point(2, 1), True)
    print(hash(some_int) == hash(some_int), hash(some_int) == hash(none_int))
    print(hash(some_str) == hash(some_str), hash(some_str) == hash(none_str))
    print(hash(some_point) == hash(other_point))
    print(hash(none_int) == hash(none_str))
    print(lanes_hash(some_int) == lanes_hash(none_int))
    print(lanes_hash(some_str) == lanes_hash(some_point))
    print(some_int.tagged_code(), some_str.tagged_code(), some_point.tagged_code())
