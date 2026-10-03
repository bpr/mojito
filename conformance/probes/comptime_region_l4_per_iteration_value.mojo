# Compile-time region ownership probe l4 (docs/notes/comptime-region-ownership.md).
# Pin verdict, the runtime-shaped twin, and the `--comptime-regions keep`
# experiment are tabulated there; the production path selects the arm and
# unrolls the loop before the move analysis runs.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del", self.s)

def consume(var t: Thing):
    print("consume", t.s)

def look(t: Thing):
    print("look", t.s)

def f[n: Int]():
    var a = Thing(String("a"))
    comptime for i in range(n):
        var t = Thing(String("t") + String(i))
        look(t)
        look(a)
    print("after")

def main():
    f[0]()
    print("--")
    f[1]()
    print("--")
    f[3]()
