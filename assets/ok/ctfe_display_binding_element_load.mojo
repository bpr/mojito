# An element read off a compile-time display binding (`comptime r = L[0]`,
# `comptime MR = ML[1]`, `comptime q = ML[i]` in a `comptime for`) is upstream's `load_from_mem` of the binding's
# element: the binding is a parameter attribute nothing destroys, so the
# element is loaded at any `Movable` type, with no copy initializer run, and
# neither the binding's elements nor the loaded one end in a destructor.
struct NC(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("deinit", self.n)


struct C(Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n + 100


def mk(n: Int) -> NC:
    return NC(n)


def mkc(n: Int) -> C:
    return C(n)


comptime ML = [mk(1), mk(2)]
comptime MR = ML[1]
comptime CL = [mkc(5), mkc(6)]
comptime CR = CL[0]


def generic[k: Int]():
    comptime L = [mk(k), mk(k + 1)]
    comptime r = L[1]
    print(r.n)


def main():
    comptime L = [mk(3), mk(4)]
    comptime r = L[0]
    print(r.n)
    print(MR.n)
    comptime cl = [mkc(7), mkc(8)]
    comptime c = cl[1]
    print(c.n, CR.n)
    generic[10]()
    comptime for i in range(2):
        comptime e = L[i]
        comptime q = ML[i]
        comptime k = CL[i]
        print(e.n, q.n, k.n)
