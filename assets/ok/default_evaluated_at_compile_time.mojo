# A default that is neither a literal nor a conversion of one is evaluated
# once per instance at compile time, and each call leaving its slot out
# materializes the value: `side`, `sidep 1`, `sidep 2`, `init 5`, `init 6`,
# and `Q init 12` are printed while the program elaborates, never by the
# program. A call materializes a fresh value (`noisy_var` prints `7` each
# time). An unused default whose value's destruction runs code is still
# evaluated, and destroyed after each call (`del 5`); an unused one whose
# destruction runs no code is not evaluated at all (`Q(11)`).
def side() -> Int:
    print("side")
    return 7


def fd2(x: Int = side()) -> Int:
    return x


def sidep[n: Int]() -> Int:
    print("sidep", n)
    return n


def fp[n: Int](x: Int = sidep[n]()) -> Int:
    return x


struct Noisy(Movable):
    var v: Int

    def __init__(out self, v: Int):
        print("init", v)
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def noisy(a: Int, n: Noisy = Noisy(5)) -> Int:
    return a


def noisy_var(var n: Noisy = Noisy(6)) -> Int:
    n.v += 1
    return n.v


struct Q(Movable):
    var v: Int

    def __init__(out self, v: Int):
        print("Q init", v)
        self.v = v


def g(a: Int, q: Q = Q(11)) -> Int:
    return a


def h(a: Int, q: Q = Q(12)) -> Int:
    return a + q.v


def main():
    print("main", fd2(), fd2(), fd2(3))
    print(fp[1](), fp[2](), fp[1]())
    print(noisy(4))
    print(noisy(5))
    print(noisy_var())
    print(noisy_var())
    print(g(1), h(1))
