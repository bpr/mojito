# Probe: may a compile-time evaluation call a generic struct's method that
# holds a `comptime if`?
#
# The pin (2026-09-21) prints `1 6`. Mojito stops with "Cell.m: unspecialized
# type-keyed method": the evaluation's subprogram carries the method as its
# template stub and mints no per-instantiation clone. `docs/roadmap.md` 3.122.
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

comptime CAP = np(5)

def main():
    print(Cell(9).m(), CAP)
