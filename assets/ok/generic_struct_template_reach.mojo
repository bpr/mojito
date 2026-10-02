# A method of an ordinary generic struct with no compile-time construct in
# its body mints no clone per instance: the elaborator instantiates the
# template's MIR. What such a body reaches at an instance is still found:
# a member that keeps its clones (a `comptime if` on `Self.T`, a type name)
# called by name, through `print`, or on an instance only the template body
# builds (`Box[List[Self.T]]`), runs as that instance's clone.
from std.reflection.type_info import _unqualified_type_name


struct Box[T: Copyable & Deinitable & Writable](Copyable, Movable, Writable):
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def kind(self) -> String:
        comptime if Self.T == Int:
            return String("int")
        else:
            return String("other")

    def name(self) -> String:
        return String(_unqualified_type_name[Self]())

    def write_to(self, mut writer: Some[Writer]):
        writer.write(_unqualified_type_name[Self](), "(", self.value, ")")

    def plain(self) -> Self.T:
        return self.value.copy()


struct Outer[T: Copyable & Deinitable & Writable](Movable):
    var inner: Box[Self.T]
    var count: Int

    def __init__(out self, var value: Self.T):
        self.inner = Box[Self.T](value^)
        self.count = 0

    def kind(self) -> String:
        return self.inner.kind()

    def describe(self) -> String:
        return self.inner.name()

    def show(self):
        print(self.inner)

    def fresh(self) -> String:
        var made = Box[List[Self.T]](List[Self.T]())
        return made.kind() + made.name()

    def value(self) -> Self.T:
        return self.inner.plain()


def main():
    var a = Outer[Int](7)
    var b = Outer[String]("s")
    print(a.kind(), b.kind())
    print(a.describe(), b.describe())
    a.show()
    b.show()
    print(a.fresh(), b.fresh())
    print(a.value(), b.value())
