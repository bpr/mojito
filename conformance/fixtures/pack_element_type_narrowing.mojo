# Mojito narrows a variadic pack element to the enclosing method's own type
# parameter inside a folded `comptime if Self.Ts[i] == T` branch: the element
# read `self.storage[i]` is already a `T` there, so a `ref[origin_of(self)] T`
# accessor returns it and an `==` against a `T` argument type-checks.
#
# The pinned Mojo keeps the element at its dependent pack type
# (`Ts.values[...]`) and demands an explicit `rebind[T](...)`, which Mojito
# does not implement; it also rejects returning a reference into
# `self.storage` under `origin_of(self)`. This file therefore runs on Mojito
# and is rejected upstream (`cases.tsv` row `pack-element-type-narrowing`).
from std.os import abort

struct Bag[*Ts: Movable](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def get[T: AnyType](ref self) -> ref[origin_of(self)] T:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                return self.storage[i]
        abort("Bag.get: no such element type")

    def count_matching[T: AnyType](self, value: T) -> Int:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                if self.storage[i] == value:
                    return 1
        return 0

def main():
    var b = Bag[Int, String](7, "x")
    print(b.get[String]())
    print(b.get[Int]() + 1)
    print(b.count_matching(7))
    print(b.count_matching("y"))
