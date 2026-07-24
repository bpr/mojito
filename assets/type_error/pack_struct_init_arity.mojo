# expect: no constructor overload matches
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)


def main():
    var p = Pair[Int, String](1)
    print(p.storage[0])
