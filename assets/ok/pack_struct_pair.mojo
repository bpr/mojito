# Variadic-generic struct: `struct Pair[*Ts]` with pack-determined heterogeneous
# storage. Compile-time elaboration specializes each instantiation into a
# concrete struct; `p.storage[i]` has the exact per-index element type.
@fieldwise_init
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def size(self) -> Int:
        return len(self.storage)


def first_int(p: Pair[Int, Bool]) -> Int:
    return p.storage[0]


def main():
    var p: Pair[Int, Bool] = Pair[Int, Bool]((1, True))
    var q = p.copy()
    var single = Pair[String](("solo",))
    var n: Int = p.storage[0] + 41
    print(n)
    print(first_int(q))
    print(q.storage[1])
    print(p.size())
    print(single.storage[0])
    var moved = p^
    print(moved.storage[0])
