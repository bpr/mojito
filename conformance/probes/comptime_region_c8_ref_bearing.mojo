# Compile-time region ownership probe c8 (docs/notes/comptime-region-ownership.md).
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
    var b = Thing(String("b"))
    ref r = a
    comptime if n > 0:
        print("then", r.s)
    else:
        print("else", b.s)
    print("after", r.s)
    print("end")

def main():
    f[1]()
    print("--")
    f[0]()
