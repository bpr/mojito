# A variadic struct's type pack is inferred from its constructor, as current
# Mojo infers it: a fieldwise `Tuple[*Self.Ts]` field solves `*Ts` from the
# tuple display it stores, and a pack constructor `var *args: *Self.Ts` solves
# it from the collected arguments. The inferred instance is the same concrete
# struct an explicit `Pair[Int, Bool]` names, inside a generic body too.
@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def size(self) -> Int:
        return len(self.storage)


struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def size(self) -> Int:
        return len(self.storage)


def first_int(p: Pair[Int, Bool]) -> Int:
    return p.storage[0]


def wrap[T: Copyable & Movable & Deinitable](var x: T) -> Pair[T, Int]:
    return Pair((x^, 1))


def main():
    var p = Pair((1, True))
    var q: Pair[Int, Bool] = Pair((2, False))
    print(p.storage[0] + q.storage[0])
    print(first_int(p))
    print(p.storage[1], q.storage[1])
    print(p.size())
    var b = Bag(7, "x", False)
    print(b.size())
    print(b.storage[0])
    print(b.storage[1])
    print(b.storage[2])
    var w = wrap(3)
    print(w.storage[0] + w.storage[1])
    var s = wrap(String("s"))
    print(s.storage[0])
    print(s.size())
