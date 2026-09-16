# A pack element keeps its dependent type `Self.Ts[i]` inside a folded
# `comptime if Self.Ts[i] == T` arm; `rebind[T](...)` is the explicit retyping
# to the method's own `T`, as upstream spells it: a reference-returning
# accessor rebinds the element it returns, an equality compares the rebound
# element. The accessor's reference is into `self.storage`, so that is the
# origin it declares.
from std.os import abort

struct Bag[*Ts: Movable](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def get[T: AnyType](ref self) -> ref[self.storage] T:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                return rebind[T](self.storage[i])
        abort("Bag.get: no such element type")

    def count_matching[T: Equatable](self, value: T) -> Int:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                if rebind[T](self.storage[i]) == value:
                    return 1
        return 0

def main():
    var b = Bag[Int, String](7, "x")
    print(b.get[String]())
    print(b.get[Int]() + 1)
    print(b.count_matching(7))
    print(b.count_matching("y"))
