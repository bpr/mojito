# A folded `comptime if Self.Ts[i] == T` guard does not narrow a variadic pack
# element to the enclosing method's own type parameter: the element read
# `self.storage[i]` keeps its dependent type `Self.Ts[i]`, so returning it as a
# `T` and comparing it with a `T` argument are both type errors.
#
# Both compilers reject this file from the template (`cases.tsv` row
# `pack-element-type-narrowing`): the pinned Mojo names the element
# `Ts.values[...]`, Mojito `Ts[i]`. `rebind[T](...)` is the explicit retyping,
# and `assets/ok/pack_element_rebind.mojo` is the spelling both accept. The
# pin also rejects `get`'s `ref[origin_of(self)]` into `self.storage`; that
# spelling is not what this case claims.
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
