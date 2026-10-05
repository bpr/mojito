# A compile-time `continue` and `break` in a `comptime for` are the loop's:
# `t1` is destroyed at the `continue`, `t3` at the entry of the breaking arm,
# and `a`, consumed only in that arm, at the loop's exit when no iteration
# takes it. Output matches the pin (docs/notes/comptime-region-ownership.md,
# l6).
# requires: stdlib
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __deinit__(deinit self):
        print("del", self.s)

def consume(var t: Thing):
    print("consume", t.s)

def look(t: Thing):
    print("look", t.s)

def f[n: Int]():
    var a = Thing(String("a"))
    comptime for i in range(n):
        var t = Thing(String("t") + String(i))
        comptime if i == 1:
            continue
        comptime if i == 3:
            consume(a^)
            break
        look(t)
    print("after")

def main():
    f[5]()
    print("--")
    f[2]()
