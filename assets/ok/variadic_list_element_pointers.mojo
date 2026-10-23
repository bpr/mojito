# A homogeneous collector is a `VariadicList` over pointers to its
# elements, which the call builds: a `read` collector's point at the
# caller's own values, fields included, and an owned collector's at slots
# the list then owns, so its destructor alone destroys them, on a raise
# out of the callee too.
@fieldwise_init
struct D(Copyable, Writable):
    var n: Int

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut w: Some[Writer]):
        w.write("D", self.n)


@fieldwise_init
struct P(Copyable):
    var a: D
    var b: D


def show[T: Writable](*xs: T):
    for x in xs:
        print(x, end=" ")
    print()


def take(var *xs: D) raises:
    print("in take", len(xs))
    raise Error("boom")


def look(*xs: D) raises:
    print("in look", xs[0].n)
    raise Error("bang")


def keep(var *xs: D):
    print("keep", xs[len(xs) - 1].n)


def main():
    var p = P(D(1), D(2))
    show(p.a, p.b)
    show(p.b, D(9))
    for i in range(2):
        show(i, i + 1)
    try:
        take(D(3), D(4))
    except e:
        print("caught", e)
    try:
        look(p.a, D(5))
    except e:
        print("caught", e)
    try:
        take()
    except e:
        print("caught", e)
    keep(D(6), D(7), p.b.copy())
    show[String]()
    print("end", p.a.n)
