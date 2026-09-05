# Upstream's pack spellings inside a specialized variadic struct: the pack
# parameter is a `TypeList` in every compile-time position (`Ts.length`,
# `Self.Ts.length`, `Ts[i]`, `Self.Ts[i]`, `Ts.contains[T]()`), a
# `comptime T = Self.Ts[i]` alias binds the element type inside a
# `comptime for` body, type values compare with `==`, and conditional
# conformances and method availability spell
# `where Ts.all_conforms_to[Trait]()`.
from std.collections.tuple import Tuple

struct Bag[*Ts: Copyable & Movable](
    Copyable,
    Movable,
    Equatable where Ts.all_conforms_to[Equatable](),
    Writable where Self.Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple[*Ts](*args^)

    def __eq__(self, other: Self) -> Bool where Self.Ts.all_conforms_to[Equatable]():
        comptime for i in range(Ts.length):
            if self.storage[i] != other.storage[i]:
                return False
        return True

    def has_int(self) -> Bool:
        return Ts.contains[Int]()

    def write_to(self, mut writer: Some[Writer]) where Ts.all_conforms_to[Writable]():
        writer.write("Bag[")
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            comptime if i > 0:
                writer.write(", ")
            comptime if T == Int:
                writer.write("int ")
            comptime if Ts[i] == String:
                writer.write("str ")
            writer.write(self.storage[i])
        writer.write("]")

def main():
    var b = Bag[Int, String, Bool](7, "x", True)
    print(b)
    print(b.has_int())
    print(Bag[String]("only").has_int())
    print(b == Bag[Int, String, Bool](7, "x", True))
    print(b == Bag[Int, String, Bool](8, "x", True))
