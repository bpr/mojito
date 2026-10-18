# A `^` transfer an owning (`var *xs`) collector gathers hands over the
# source's storage, as at Mojo, so no move initializer runs, and the pack
# destroys its elements last to first when it dies. A loop or comprehension
# over a collector, read or owning, borrows each element in place rather than
# copying the pack or moving its elements out. One line differs from the pin,
# which moves a *temporary* argument into the pack through the move
# initializer (`i 1009 1`, `del 1009` where Mojito prints `i 9 1`, `del 9`):
# conformance/assets-mojo-output-diffs.tsv, roadmap R1131.
struct M(Copyable, Movable, Writable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
    def __deinit__(deinit self):
        print("del", self.v)
    def write_to(self, mut writer: Some[Writer]):
        writer.write("M", self.v)


def many(var *xs: M):
    for x in xs:
        print("x", x.v)


def readmany(*xs: M):
    for x in xs:
        print("r", x.v)


def idx(var *xs: M):
    print("i", xs[0].v, len(xs))


def take[*Ts: Writable & Movable](var *a: *Ts):
    comptime for i in range(len(Ts)):
        print("t", a[i])


def comprehension(*xs: M):
    var vs = [x.v for x in xs]
    print("c", vs[0], vs[1])


def bump(var *xs: M):
    for ref x in xs:
        x.v += 10
    for x in xs:
        print("u", x.v)


def total(*ns: Int) -> Int:
    var t = 0
    for var n in ns:
        n += 1
        t += n
    return t


def main():
    var a = M(1)
    var b = M(2)
    many(a^, b^)
    var c = M(3)
    var d = M(4)
    readmany(c, d)
    comprehension(c, d)
    idx(c^, d^)
    var e = M(5)
    var f = M(6)
    take(e^, f^)
    var g = M(7)
    var h = M(8)
    bump(g^, h^)
    var k = M(9)
    idx(M(copy=k))
    print(total(1, 2, 3))
