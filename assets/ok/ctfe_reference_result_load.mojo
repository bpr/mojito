# A compile-time initializer's reference result is upstream's `load_from_mem`
# of the referent: the element of a temporary display is loaded at any
# `Movable` type, with no copy initializer and no move initializer run on the
# way out, and the display's temporaries end without their destructors.
struct NC(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n


struct C(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n + 100


struct D(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)


def mk(n: Int) -> NC:
    return NC(n)


def mkc(n: Int) -> C:
    return C(n)


def mkd(n: Int) -> D:
    return D(n)


comptime MR = [mk(3), mk(4)][1]


def gen[k: Int]():
    comptime r = [mk(k), mk(2)][0]
    print(r.n)


def main():
    comptime r = [mk(1), mk(2)][0]
    print(r.n)
    print(MR.n)
    gen[5]()
    comptime t = [mk(6), mk(7)][1].n
    print(t)
    comptime c = [mkc(8), mkc(9)][0]
    print(c.n, materialize[c]().n)
    comptime d = [mkd(10), mkd(11)][1]
    print(d.n)
