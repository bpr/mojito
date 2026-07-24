# expect: variadic struct 'Pair' requires explicit compile-time type arguments
@fieldwise_init
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]


def main():
    var p = Pair((1, True))
    print(p.storage[0])
