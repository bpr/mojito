# `repr(x)` on a closed generic-struct instance writes through the instance's
# `write_repr_to` clone, so the text spells the instantiation. VM and Mojo
# only: native `repr` of a user struct is a recorded gap (docs/roadmap.md).
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


def main():
    var a = Box[Int](7)
    var b = Box(String("seven"))
    print(repr(a), repr(b))
    print(a, repr(a))
