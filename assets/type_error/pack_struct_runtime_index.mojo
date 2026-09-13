# expect: expected a compile-time Int index
@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]


def main():
    var p = Pair[Int, Bool]((1, True))
    var i = 0
    print(p.storage[i])
