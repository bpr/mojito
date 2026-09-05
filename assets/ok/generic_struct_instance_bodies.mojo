# requires: discovery
# A closed application of an ordinary generic struct (`Box[Int]`,
# `Dict[String, Int]`) mints one clone per method on the template, checked
# with `self` bound to the instance: `_unqualified_type_name[Self]` spells the
# instantiation and a `comptime if` on `Self.T` folds per instance, while the
# template keeps Mojo's generic pre-check and the runtime name stays `Box`.
from std.reflection.type_info import _unqualified_type_name


struct Box[T: Copyable & Deinitable](Copyable, Movable, Writable where conforms_to(T, Writable)):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def type_name(self) -> String:
        return String(_unqualified_type_name[Self]())

    def kind(self) -> String:
        comptime if Self.T == Int:
            return String("int box")
        else:
            return String("other box")

    def write_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write("Box(", self.value, ")")

    def write_repr_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write(_unqualified_type_name[Self](), "(", repr(self.value), ")")


def main() raises:
    var a = Box[Int](7)
    var b = Box(String("seven"))
    print(a.type_name(), b.type_name())
    print(a.kind(), b.kind())
    print(a, b)
    var d: Dict[String, Int] = {"one": 1, "two": 2}
    print(d["one"] + d["two"], len(d))
    var opt = Optional[Int](5)
    print(opt.value(), opt.or_else(0))
