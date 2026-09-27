# A one-element tuple `(7,)` — declared, printed, measured, indexed,
# compared, hashed, searched — and a variadic struct whose
# `Tuple[*Self.Ts]` storage holds a single element.
struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable, Sized):
    var storage: Tuple[*Self.Ts]
    var count: Int

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)
        self.count = 0

    def __len__(self) -> Int:
        return len(self.storage)


def main():
    var one = (7,)
    var other = (8,)
    print(one, len(one), one[0])
    print(one == other, one != other, one < other, one == (7,))
    print(hash(one) == hash((7,)))
    var copied = one
    print(copied[0] + one[0])
    print(7 in one, 8 in one)
    var b = Bag[Bool](True)
    print(len(b), b.count)
