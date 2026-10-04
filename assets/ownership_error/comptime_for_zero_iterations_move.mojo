# A `comptime for` over `range(0)` consumes `a` in its body, and `a` is used
# after the loop. The body is decided as a loop body with its trip count
# unknown, as the pin decides it: the template keeps the loop through the
# move analysis, and the use after it is rejected whatever the trip count
# (docs/notes/comptime-region-ownership.md, l1).
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
    comptime for i in range(n):
        consume(a^)
    look(a)

def main():
    f[0]()
