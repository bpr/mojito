# Compile-time region ownership probe c7 (docs/notes/comptime-region-ownership.md).
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

def f[n: Int]() raises:
    var a = Thing(String("a"))
    var b = Thing(String("b"))
    comptime if n > 0:
        consume(a^)
        raise Error("boom")
    else:
        look(a)
    print("after", a.s, b.s)

def main():
    try:
        f[1]()
    except e:
        print("caught", e)
    print("--")
    try:
        f[0]()
    except e:
        print("caught", e)
