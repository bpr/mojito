# expect: variadic struct 'Pair' requires explicit compile-time type arguments
@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]


def main():
    var p = Pair((1, True))
    print(p.storage[0])
