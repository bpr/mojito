# expect: 'Pair' expects 1 argument(s), got 2
# An explicit construction of a variadic struct inside an untaken `comptime if`
# arm is matched against the template's constructor, as the pinned Mojo
# matches it: the fieldwise constructor takes the one `Tuple[*Self.Ts]` field,
# so two element arguments are refused before elaboration selects `n == 0`.
@fieldwise_init
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]


def f[n: Int]() -> Int:
    comptime if n == 0:
        return 1
    else:
        var p = Pair[Int, Bool](1, True)
        return len(p.storage)


def main():
    print(f[0]())
