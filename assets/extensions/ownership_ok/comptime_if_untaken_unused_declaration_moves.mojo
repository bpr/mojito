# Ledgered divergence (docs/roadmap.md 3.2): the untaken arm declares
# `var t = a^` and never uses `t`; `a` is used after the join. The pin
# rejects the use after the join; Mojito selects the arm before the move
# analysis. Rule and probes: docs/notes/comptime-region-ownership.md.
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
    comptime if n > 0:
        print("then")
    else:
        var t = a^
    look(a)

def main():
    f[1]()
