# A field's replaced value dies at its own last use, not at the store that
# replaces it, as the pinned Mojo's destructor insertion places it: after the
# read that last uses it, at the entry of the `if` arm that does not move it
# out, and, for a `mut` parameter whose field the body overwrites before
# reading it, at the function's entry.
# Expected: del 10 / in / stored / param 12 11 / del 12 / del 11 / read 1 /
# del 1 / between / stored 3 2 / del 3 / del 2 / took 21 / del 21 / after if /
# q 20 22 / del 20 / del 22 / del 31 / else arm r / after if r / r 30 32 /
# del 30 / del 32
@fieldwise_init
struct Inner(Movable):
    var id: Int

    def __deinit__(deinit self):
        print("del", self.id)


@fieldwise_init
struct Pair(Movable):
    var a: Inner
    var b: Inner


def take(var x: Inner):
    print("took", x.id)


def overwrite_param(mut p: Pair, id: Int):
    print("in")
    p.a = Inner(id)
    print("stored")


def locals(c: Bool):
    var p = Pair(Inner(1), Inner(2))
    print("read", p.a.id)
    print("between")
    p.a = Inner(3)
    print("stored", p.a.id, p.b.id)
    var q = Pair(Inner(20), Inner(21))
    if c:
        take(q.b^)
    else:
        print("else arm")
    print("after if")
    q.b = Inner(22)
    print("q", q.a.id, q.b.id)
    var r = Pair(Inner(30), Inner(31))
    if not c:
        take(r.b^)
    else:
        print("else arm r")
    print("after if r")
    r.b = Inner(32)
    print("r", r.a.id, r.b.id)


def main():
    var p = Pair(Inner(10), Inner(11))
    overwrite_param(p, 12)
    print("param", p.a.id, p.b.id)
    locals(String("x").byte_length() == 1)
