# A value consumed and refilled in every iteration of a `comptime for` is
# live after the loop, for zero and for several iterations. Output matches
# the pin (docs/notes/comptime-region-ownership.md, l5).
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

def rebuild(var t: Thing, i: Int) -> Thing:
    print("rebuild", t.s)
    return Thing(t.s + String(i))

def f[n: Int]():
    var a = Thing(String("a"))
    comptime for i in range(n):
        a = rebuild(a^, i)
    look(a)
    print("after")

def main():
    f[0]()
    print("--")
    f[3]()
