# A pointer to a `Tuple` element, `Pointer(to=t[0])`, writes the element in
# place as current Mojo does: on a local, on a field through a method's
# `self`, and over each index of a variadic struct's own `comptime for`.
struct Pair(Movable):
    var items: Tuple[Int, Int]

    def __init__(out self):
        self.items = Tuple(1, 2)

    def fill(mut self):
        Pointer(to=self.items[1]).unsafe_write(9)


struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def reset(mut self) where conforms_to(Self.Ts.values, Defaultable):
        comptime for i in range(len(Self.Ts)):
            Pointer(to=self.storage[i]).unsafe_write({})


def main():
    var t = Tuple[Int, Int](1, 2)
    Pointer(to=t[0]).unsafe_write(7)
    print(t[0], t[1])
    var p = Pair()
    p.fill()
    print(p.items[0], p.items[1])
    var bag = Bag(3, True, 2.5)
    bag.reset()
    print(bag.storage[0], bag.storage[1], bag.storage[2])
