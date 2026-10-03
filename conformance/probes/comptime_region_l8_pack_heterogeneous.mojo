# Compile-time region ownership probe l8 (docs/notes/comptime-region-ownership.md).
# Pin verdict, the runtime-shaped twin, and the `--comptime-regions keep`
# experiment are tabulated there; the production path selects the arm and
# unrolls the loop before the move analysis runs.
struct A(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del A", self.s)

struct B(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __del__(deinit self):
        print("del B", self.n)

def f[*Ts: Movable](var *args: *Ts):
    print("in f")
    comptime for i in range(len(Ts)):
        print("iter", i)
    print("end f")

def main():
    f(A(String("x")), B(7), A(String("y")))
    print("--")
    f()
