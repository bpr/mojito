# A per-instantiation method clone inherits its checked template's facts when
# the body calls a method through the struct parameter's bound with arguments
# (`docs/notes/instantiation-from-template.md`, class MethodBody, features
# `bound_dispatch`, `bound_builtins`, `bound_binders`). The template records
# the abstract dispatch (`item.__hash__(hasher)`), the inverted write
# (`item.write_to(writer)`), or the checker builtin (`writer.write(item)`),
# and an instance re-selects each on its own type: a built-in leaf has no
# callee, `String` and a user struct select their own method. The method's
# own `[H: Hasher]` binder stays symbolic in every clone.
from std.hashlib import Hasher


@fieldwise_init
struct Point(Copyable, Deinitable, Equatable, Hashable, Movable, Writable):
    var x: Int
    var y: Int

    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P", self.x, "/", self.y)


struct Pair[T: Copyable & Deinitable & Hashable & Writable](Hashable, Movable, Writable):
    var a: Self.T
    var b: Self.T

    def __init__(out self, var a: Self.T, var b: Self.T):
        self.a = a^
        self.b = b^

    def __hash__[H: Hasher](self, mut hasher: H):
        self.a.__hash__(hasher)
        self.b.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(", self.a, ", ")
        self.b.write_to(writer)
        writer.write(")")

    def write_second(self, mut writer: Some[Writer]):
        writer.write(self.b)


def main():
    var ints = Pair[Int](1, 2)
    var same_ints = Pair[Int](1, 2)
    var other_ints = Pair[Int](2, 1)
    var strings = Pair[String](String("a"), String("b"))
    var same_strings = Pair[String](String("a"), String("b"))
    var points = Pair[Point](Point(1, 2), Point(3, 4))
    var other_points = Pair[Point](Point(1, 2), Point(4, 3))
    print(ints, strings, points)
    print(hash(ints) == hash(same_ints), hash(ints) == hash(other_ints))
    print(hash(strings) == hash(same_strings), hash(points) == hash(other_points))
    var text = String()
    ints.write_second(text)
    strings.write_second(text)
    points.write_second(text)
    print(text)
