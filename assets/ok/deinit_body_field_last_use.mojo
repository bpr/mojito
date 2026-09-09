# Pinned against Mojo 1.1.0.dev2026082605 (2026-09-08): inside a `__deinit__`
# or any `deinit self` body, each direct field of `self` is destroyed at that
# field's own last use — an unused field before the body's first statement, a
# field read in an `if` condition right after the condition, a field read in
# one arm inside that arm and at the other arm's entry, a field read only
# through a call on bare `self` right after that call — while a nested read
# (`self.p.b.id`, `peek(self.q.b)`) keeps the whole direct field alive, and
# a field transferred out (`self.b^`) is the new owner's. Every value the body
# never reads dies at entry, so a destructor that never touches `self` still
# destroys its fields first, in declaration order.
# Expected:
#   del inner 1 / del outer start / uses 2 / del inner 2 / del outer end / mid
#   take_b start / take_b mid 1 / del inner 1 / take_b end / got 2 / del inner 2
#   start / del inner 1 / branch a 2 / del inner 2 / after 3 / del inner 3 / end
#   start / use 12 / del inner 11 / del inner 12 / after use / peek inner 22
#   / del inner 21 / del inner 22 / take 3 / del inner 3 / end
#   start / dump 1 2 3 / del inner 1 / after dump / took 2 / del inner 2
#   / uses 3 / del inner 3 / end / done
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
        print("del outer start")
        print("uses", self.b.id)
        print("del outer end")

    def take_b(deinit self) -> Inner:
        print("take_b start")
        var b = self.b^
        print("take_b mid", self.a.id)
        print("take_b end")
        return b^


struct Branching:
    var a: Inner
    var b: Inner
    var c: Inner

    def __init__(out self):
        self.a = Inner(1)
        self.b = Inner(2)
        self.c = Inner(3)

    def __deinit__(deinit self):
        print("start")
        if self.a.id == 1:
            print("branch a", self.b.id)
        else:
            print("branch b")
        print("after", self.c.id)
        print("end")


struct Plain:
    var a: Inner
    var b: Inner

    def __init__(out self, base: Int):
        self.a = Inner(base + 1)
        self.b = Inner(base + 2)


def peek_inner(x: Inner):
    print("peek inner", x.id)


def take(var x: Inner):
    print("take", x.id)


struct Nested:
    var p: Plain
    var q: Plain
    var r: Inner

    def __init__(out self):
        self.p = Plain(10)
        self.q = Plain(20)
        self.r = Inner(3)

    def __deinit__(deinit self):
        print("start")
        print("use", self.p.b.id)
        print("after use")
        peek_inner(self.q.b)
        take(self.r^)
        print("end")


struct BareUse:
    var a: Inner
    var b: Inner
    var c: Inner

    def __init__(out self):
        self.a = Inner(1)
        self.b = Inner(2)
        self.c = Inner(3)

    def dump(self):
        print("dump", self.a.id, self.b.id, self.c.id)

    def __deinit__(deinit self):
        print("start")
        self.dump()
        print("after dump")
        var b = self.b^
        print("took", b.id)
        print("uses", self.c.id)
        print("end")


def main():
    var o = Outer()
    print("mid")
    var o2 = Outer()
    var b = o2^.take_b()
    print("got", b.id)
    var br = Branching()
    var n = Nested()
    var bu = BareUse()
    print("done")
