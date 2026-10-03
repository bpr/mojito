# Compile-time region ownership probe l9 (docs/notes/comptime-region-ownership.md).
# Pin verdict, the runtime-shaped twin, and the `--comptime-regions keep`
# experiment are tabulated there; the production path selects the arm and
# unrolls the loop before the move analysis runs.
struct A(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del A", self.s)

def take(var a: A):
    print("take", a.s)

def f[*Ts: Movable](var *args: *Ts):
    comptime for i in range(len(Ts)):
        comptime if i == 1:
            print("skip", i)
        else:
            print("iter", i)
    print("end f")

def main():
    f(A(String("x")), A(String("y")), A(String("z")))
