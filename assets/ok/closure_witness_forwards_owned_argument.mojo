# A call through a capturing closure's `__call__` witness hands an owned
# argument on without a move, as at the pin: `apply(h)` prints no move.
struct M(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1
        print("move", self.v)
    def __deinit__(deinit self):
        print("del", self.v)

def apply[F: def(var x: M) -> None](f: F):
    f(M(10))

def main():
    var n = 0
    def h(var x: M) {mut n}:
        n += 1
        print("got", x.v)
    h(M(1))
    var m = M(5)
    h(m^)
    apply(h)
    print(n)
