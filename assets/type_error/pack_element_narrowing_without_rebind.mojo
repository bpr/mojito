# expect: operator '==' is not defined for Ts[i] and T
# A folded `comptime if Self.Ts[i] == T` guard does not narrow the element: it
# keeps its dependent type `Self.Ts[i]`, and `rebind[T](...)` is the explicit
# retyping (`assets/ok/pack_element_rebind.mojo`).
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def count_matching[T: Equatable](self, value: T) -> Int:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                if self.storage[i] == value:
                    return 1
        return 0


def main():
    var b = Bag[Int, String](7, "x")
    print(b.count_matching(7))
