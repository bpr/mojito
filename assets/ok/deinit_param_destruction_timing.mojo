# Pinned against Mojo 1.1.0.dev2026082605 (2026-09-08): a consuming receiver
# (`deinit self`, `var self`) is an owned local of the callee, destroyed at its
# last use — an unused one's fields die before the body's first statement, on
# the normal and the raising path alike — and a struct without `__deinit__`
# destroys its fields in declaration order while simultaneously dying locals
# and owned parameters die in reverse declaration order.
# Expected:
#   del part 9 / commit start / commit end / done 1
#   del part 8 / commit start / caught / done 2
#   peek 7 / del part 7 / del tx / consume start / consume end / done 3
#   1 / del inner 1 / del inner 2 / del outer / done 4
#   del inner 10 / del inner 20 / locals / del inner 1 / del inner 2 / plain
#   2 1 / del inner 1 / del inner 2 / 10 20 / del inner 20 / del inner 10
#   take 1 2 / del inner 2 / del inner 1 / done 5
struct Part:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del part", self.n)


struct Tx:
    var part: Part

    def __init__(out self, n: Int):
        self.part = Part(n)

    def commit(deinit self):
        print("commit start")
        print("commit end")

    def commit_raising(deinit self) raises:
        print("commit start")
        raise Error("commit failed")


struct Owned:
    var part: Part

    def __init__(out self, n: Int):
        self.part = Part(n)

    def __deinit__(deinit self):
        print("del tx")

    def consume(var self):
        print("consume start")
        print("consume end")

    def peek(self):
        print("peek", self.part.n)


struct Inner:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("del inner", self.id)


struct Outer:
    var a: Inner
    var b: Inner

    def __init__(out self):
        self.a = Inner(1)
        self.b = Inner(2)

    def __deinit__(deinit self):
        print("del outer")


struct Plain:
    var a: Inner
    var b: Inner

    def __init__(out self):
        self.a = Inner(1)
        self.b = Inner(2)


def take(var a: Inner, var b: Inner):
    print("take", a.id, b.id)


def main():
    var tx = Tx(9)
    tx^.commit()
    print("done 1")
    var raising = Tx(8)
    try:
        raising^.commit_raising()
    except:
        print("caught")
    print("done 2")
    var owned = Owned(7)
    owned.peek()
    owned^.consume()
    print("done 3")
    var o = Outer()
    print(o.a.id)
    print("done 4")
    var x = Inner(10)
    var y = Inner(20)
    print("locals")
    var p = Plain()
    print("plain")
    var q = Plain()
    print(q.b.id, q.a.id)
    var m = Inner(10)
    var n = Inner(20)
    print(m.id, n.id)
    take(Inner(1), Inner(2))
    print("done 5")
