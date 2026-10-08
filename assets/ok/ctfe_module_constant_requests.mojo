# A module constant whose initializer applies a callable is evaluated on
# demand: a function body's read of it is a request the elaborator serves
# from its worklist — through a generic struct's method that holds a
# `comptime if`, or a value-keyed `def` that recurses under one — and a type
# that reads it forces it once.
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


def through_cell(n: Int) -> Int:
    return Cell[Int](n).m() + n


def depth[n: Int]() -> Int:
    comptime if n == 0:
        return 0
    else:
        return 1 + depth[n - 1]()


def unused() -> Int:
    return 99


comptime CAP = through_cell(5)
comptime DEPTH = depth[3]()
comptime NEXT = DEPTH + 1
comptime WIDTH = depth[2]() * 2
comptime NEVER = unused()


def read_next() -> Int:
    return NEXT


def main():
    var v = SIMD[DType.int32, WIDTH](2)
    print(Cell(9).m(), CAP, DEPTH, read_next(), v)
