# A `comptime for` over `range(1)` consumes `a` in its body without refilling
# it. The body is decided as a loop body with its trip count unknown, as the
# pin decides it: the consume is a use on the back edge, rejected whatever
# the trip count (docs/notes/comptime-region-ownership.md, l3).
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
    print("after")

def main():
    f[1]()
