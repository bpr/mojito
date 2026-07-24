# expect: expected a compile-time Int index
@fieldwise_init
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]


def main():
    var p = Pair[Int, Bool]((1, True))
    var i = 0
    print(p.storage[i])
