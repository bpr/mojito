# A value read in one arm of a `comptime if` and again after the join lives
# to the later read; the value the other arm alone reads dies at that arm's
# entry when the first is taken. Output matches the pin
# (docs/notes/comptime-region-ownership.md, c9).
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
        look(a)
    else:
        look(b)
    look(a)
    print("end")

def main():
    f[1]()
    print("--")
    f[0]()
