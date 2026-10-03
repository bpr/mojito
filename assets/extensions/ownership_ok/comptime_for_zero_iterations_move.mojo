# Ledgered divergence (docs/roadmap.md R21): a `comptime for` over
# `range(0)` consumes `a` in its body, and `a` is used after the loop. The
# pin decides the body as a loop body with its trip count unknown and
# rejects the use; Mojito unrolls zero iterations before the move analysis.
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
    comptime for i in range(n):
        consume(a^)
    look(a)

def main():
    f[0]()
