# Upstream's pack spellings inside a specialized variadic struct: the pack
# parameter is a `TypeList` in every compile-time position (`Self.Ts.length`,
# `Self.Ts[i]`, `Self.Ts.contains[T]()`), a `comptime T = Self.Ts[i]` alias
# binds the element type inside a `comptime for` body, type values compare
# with `==`, and conditional conformances and method availability spell
# `where Ts.all_conforms_to[Trait]()`. A struct's own conformance clauses
# name the parameter bare, because `Self` is not available there; everywhere
# inside a method it is `Self.Ts` (Mojito also accepts the bare name there —
# see the divergence ledger in `docs/roadmap.md`).
struct Bag[*Ts: Movable](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
    Equatable where Ts.all_conforms_to[Equatable](),
    Writable where Ts.all_conforms_to[Writable](),
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def __eq__(self, other: Self) -> Bool where Self.Ts.all_conforms_to[Equatable]():
        comptime for i in range(Self.Ts.length):
            if self.storage[i] != other.storage[i]:
                return False
        return True

    def has_int(self) -> Bool:
        return Self.Ts.contains[Int]()

    def write_to(self, mut writer: Some[Writer]) where Self.Ts.all_conforms_to[Writable]():
        writer.write("Bag[")
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            comptime if i > 0:
                writer.write(", ")
            comptime if T == Int:
                writer.write("int ")
            comptime if Self.Ts[i] == String:
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
