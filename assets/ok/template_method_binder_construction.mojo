# A per-instantiation method clone inherits its checked template's facts when
# the body constructs a value of the method's own trait-bounded binder
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `bound_binders`). Every clone keeps `[H: Hasher]` symbolic, so each records
# the template's construction (`H()`) and consumes the fresh hasher through
# the same `finish` builtin (`inner^.finish()`), whatever the struct's own
# parameter binds.
from std.hashlib import Hasher


@fieldwise_init
struct Point(Copyable, Deinitable, Equatable, Hashable, Movable):
    var x: Int
    var y: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)


struct Sealed[T: Copyable & Deinitable & Hashable](Hashable, Movable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def __hash__[H: Hasher](self, mut hasher: H):
        var inner = H()
        self.value.__hash__(inner)
        var sealed = inner^.finish()
        hasher._update_with_simd(sealed)


def main():
    var ints = Sealed[Int](1)
    var same_ints = Sealed[Int](1)
    var other_ints = Sealed[Int](2)
    var strings = Sealed[String](String("a"))
    var same_strings = Sealed[String](String("a"))
    var other_strings = Sealed[String](String("b"))
    var points = Sealed[Point](Point(1, 2))
    var same_points = Sealed[Point](Point(1, 2))
    var other_points = Sealed[Point](Point(2, 1))
    print(hash(ints) == hash(same_ints), hash(ints) == hash(other_ints))
    print(hash(strings) == hash(same_strings), hash(strings) == hash(other_strings))
    print(hash(points) == hash(same_points), hash(points) == hash(other_points))
