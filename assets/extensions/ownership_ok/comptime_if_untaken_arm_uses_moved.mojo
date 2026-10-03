# Ledgered divergence (docs/roadmap.md 3.2): `a` is consumed before
# the `comptime if`, and the untaken arm reads it. The pin checks every arm
# and rejects the read; Mojito selects the arm before the move analysis.
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
    consume(a^)
    comptime if n > 0:
        print("then")
    else:
        look(a)

def main():
    f[1]()
