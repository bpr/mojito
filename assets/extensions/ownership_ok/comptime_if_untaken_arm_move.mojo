# Ledgered divergence (docs/roadmap.md 3.2): the untaken arm of a
# `comptime if` consumes `a`, and `a` is used after the join. The pin
# decides the region as an `if` with its condition opaque and rejects the
# use; Mojito selects the arm before the move analysis and runs it.
# Rule and probes: docs/notes/comptime-region-ownership.md.
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
        consume(a^)
    else:
        look(a)
    look(a)

def main():
    f[0]()
