# The untaken arm of a `comptime if` consumes `a`, and `a` is used after the
# join. The region is decided as an `if` with its condition opaque, as the
# pin decides it: the template keeps both arms through the move analysis,
# and the use after the join is rejected whichever arm an instance takes.
# Rule and probes: docs/notes/comptime-region-ownership.md.
# expect: use of uninitialized value 'a'
# requires: stdlib
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
