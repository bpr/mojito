# A `comptime for` body consumes `a`, and `a` is used after the loop: a
# use of an uninitialized value, as the pin says
# (docs/notes/comptime-region-ownership.md, l2).
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
        consume(a^)
    look(a)

def main():
    f[1]()
