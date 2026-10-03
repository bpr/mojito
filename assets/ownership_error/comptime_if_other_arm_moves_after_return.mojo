# The taken arm returns, the other arm consumes `b`, and `b` is used after
# the join: the join reads the arm that falls through, so `b` is
# uninitialized there, as the pin says (docs/notes/comptime-region-ownership.md, c6).
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
    comptime if n > 0:
        print("then", a.s)
        return
    else:
        consume(b^)
    print("after", a.s, b.s)

def main():
    f[1]()
    print("--")
    f[0]()
