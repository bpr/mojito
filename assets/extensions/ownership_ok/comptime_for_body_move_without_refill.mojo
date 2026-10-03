# Ledgered divergence (docs/roadmap.md 3.2): a `comptime for` over
# `range(1)` consumes `a` in its body without refilling it. The pin rejects
# the consume as a use on the back edge; Mojito unrolls the one iteration
# before the move analysis. Rule and probes:
# docs/notes/comptime-region-ownership.md.
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
    print("after")

def main():
    f[1]()
