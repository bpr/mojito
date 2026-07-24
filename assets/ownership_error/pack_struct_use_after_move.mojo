# expect: after it was transferred
@fieldwise_init
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]


def main():
    var p = Pair[Int, Bool]((1, True))
    var moved = p^
    print(p.storage[0])
