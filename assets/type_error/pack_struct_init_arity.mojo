# expect: no constructor overload matches
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)


def main():
    var p = Pair[Int, String](1)
    print(p.storage[0])
