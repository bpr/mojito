# `a` is consumed before the `comptime if`, and the untaken arm reads it.
# Every arm is checked with the condition opaque, as the pin checks it, so
# the read is rejected whichever arm an instance takes.
# Rule and probes: docs/notes/comptime-region-ownership.md.
# expect: use of uninitialized value 'a'
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
