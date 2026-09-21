# expect: type 'Ts[i]' has no method 'nonexistent'
# A variadic struct's method is checked once, from the template, with the
# element at a `comptime for` index opaque: a member its pack's bound does not
# declare is rejected inside an untaken `comptime if` arm, whatever the
# instances in the program hold.
struct Bag[*Ts: Movable](
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def poke(self):
        comptime for i in range(Self.Ts.length):
            comptime if i > 100:
                self.storage[i].nonexistent()


def main():
    var b = Bag[Int, String](1, "two")
    b.poke()
