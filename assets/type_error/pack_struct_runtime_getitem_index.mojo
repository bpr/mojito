# expect: expected a compile-time Int index
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Ts[i]:
        return self.storage[i]


def main():
    var p = Pair[Int, Bool](1, True)
    var i = 0
    print(p[i])
