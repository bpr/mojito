# A local `comptime` binding of a field of a temporary (`HC(mk(3)).c`) is the
# field of the evaluated application, as upstream's `load_from_mem` reads it:
# no copy or move initializer runs on the way out, the temporary ends without
# its destructor, and every read materializes the parameter value.
struct C(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n + 100

    def __deinit__(deinit self):
        print("del C", self.n)


struct HC(Copyable, Movable):
    var c: C

    def __init__(out self, var c: C):
        self.c = c^

    def __deinit__(deinit self):
        print("del HC", self.c.n)


struct M(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, deinit move: Self):
        self.n = move.n + 1000


struct H(Movable):
    var m: M

    def __init__(out self, var m: M):
        self.m = m^


struct HH(Copyable, Movable):
    var h: HC

    def __init__(out self, var h: HC):
        self.h = h^


def mk(n: Int) -> C:
    return C(n)


def mkm(n: Int) -> M:
    return M(n)


comptime MS = HC(mk(7)).c


def gen[k: Int]():
    comptime s = HC(mk(k)).c
    print(s.n)
    comptime t = H(mkm(k)).m
    print(materialize[t]().n)


def main():
    comptime s = HC(mk(3)).c
    print(s.n)
    comptime n = HC(mk(4)).c.n
    print(n)
    comptime d = HH(HC(mk(5))).h.c
    print(d.n)
    comptime t = H(mkm(3)).m
    print(materialize[t]().n)
    print(MS.n)
    gen[6]()
    comptime if s.n == 3:
        print("ct if")
    var v = materialize[s]()
    print(v.n)
