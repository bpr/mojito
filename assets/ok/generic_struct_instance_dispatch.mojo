# requires: discovery
# Calls on a closed generic-struct instance dispatch that instance's
# per-instantiation method clones through every call shape: `print`,
# `String(x)`, and `Writer.write` (the clone's `write_to` spells the
# instantiation), the `==`/`!=` operator dunders, `len(x)` (`__len__` folds
# its `comptime if Self.T == Int` per instance), subscripts and iteration
# over `List[Int]`/`Dict[String, Int]`, and `Bool(opt)` on `Optional[Int]`.
from std.reflection.type_info import _unqualified_type_name


struct Box[T: Copyable & Deinitable](
    Copyable,
    Equatable where conforms_to(T, Equatable),
    Movable,
    Sized,
    Writable where conforms_to(T, Writable),
):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def __eq__(self, other: Self) -> Bool where conforms_to(Self.T, Equatable):
        return self.value == other.value

    def __ne__(self, other: Self) -> Bool where conforms_to(Self.T, Equatable):
        return not (self.value == other.value)

    def __len__(self) -> Int:
        comptime if Self.T == Int:
            return 1
        else:
            return 2

    def write_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write(_unqualified_type_name[Self](), "(", self.value, ")")

    def write_repr_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write(_unqualified_type_name[Self](), "(", repr(self.value), ")")


def main() raises:
    var a = Box[Int](7)
    var b = Box(String("seven"))
    print(a, b)
    print(String(a), String(b))
    print(a == Box[Int](7), a != Box[Int](8), b == Box(String("seven")))
    print(len(a), len(b))
    var xs: List[Int] = [1, 2, 3]
    xs[0] = 10
    xs.append(4)
    var total = 0
    for x in xs:
        total += x
    print(xs[0], len(xs), total, 4 in xs, 9 in xs)
    var d: Dict[String, Int] = {"one": 1}
    d["two"] = 2
    var keys = String("")
    for key in d.keys():
        keys += key
    print(d["two"], len(d), keys)
    var opt = Optional[Int](5)
    print(opt.value(), Bool(opt), Bool(Optional[Int]()))
