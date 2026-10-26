# A compile-time evaluation calls per-instance code: a generic struct's
# method holding a `comptime if`, a value-keyed `def` recursing under a
# `comptime if`, and the variadic `write_to` members a t-string and a
# `Tuple` write through.
comptime FLAG = 3


struct Cell[T: Copyable & Deinitable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def m(self) -> Int:
        comptime if FLAG > 2:
            return 1
        else:
            return 2


def np(n: Int) -> Int:
    return Cell[Int](n).m() + n


def rep[n: Int]() -> Int:
    comptime if n == 0:
        return 0
    else:
        return 1 + rep[n - 1]()


def label(n: Int) -> String:
    return String(t"n={n}")


def pair(n: Int) -> String:
    return String((n, 2))


comptime CAP = np(5)
comptime R = rep[3]()


def main():
    print(Cell(9).m(), CAP)
    print(R)
    comptime s = label(3)
    comptime p = pair(3)
    print(s)
    print(p)
