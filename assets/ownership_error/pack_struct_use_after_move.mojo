# expect: use of uninitialized value
@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]


def main():
    var p = Pair[Int, Bool]((1, True))
    var moved = p^
    print(p.storage[0])
