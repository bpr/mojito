# expect: no constructor overload matches
# An explicit construction of a variadic struct inside an untaken `comptime if`
# arm is matched against the template's constructor, as the pinned Mojo
# matches it: the `*args: *Self.Ts` collector must take exactly the elements
# `Bag[Int, String]` binds, so one argument is refused before elaboration
# selects `n == 0`.
struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)


def f[n: Int]() -> Int:
    comptime if n == 0:
        return 1
    else:
        var b = Bag[Int, String](1)
        return len(b.storage)


def main():
    print(f[0]())
