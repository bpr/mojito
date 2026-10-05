# A value declared in a `comptime for` body is one binding per iteration,
# destroyed after its read in that iteration; a value read in the body lives
# to its last read, in the last iteration. Output matches the pin for zero,
# one, and three trips (docs/notes/comptime-region-ownership.md, l4).
# requires: stdlib
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __deinit__(deinit self):
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
