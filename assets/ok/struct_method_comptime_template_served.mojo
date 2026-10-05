# A generic struct's method whose `comptime if` or `comptime for` reads the
# struct's own parameters (`Self.T`, `Self.n`) is served by its template, as
# a generic `def` is: the elaborator decides each branch and unrolls each
# loop per instance, beside a binder of the method's own. No clone is minted
# per instance of the struct.
struct Cell[T: Copyable & Deinitable & Writable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def label(self) -> String:
        comptime if Self.T == Int:
            return "int cell " + String(self.value)
        else:
            return "cell " + String(self.value)


struct Rep[n: Int]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def total(self) -> Int:
        var s = 0
        comptime for i in range(Self.n):
            comptime if i == 1:
                s += 100
            s += self.x
        return s

    def first[k: Int](self) -> Int:
        comptime if k == Self.n:
            return 1
        return 0


def main():
    print(Cell(9).label(), Cell(String("b")).label(), Cell(1.5).label())
    print(Rep[3](2).total(), Rep[1](5).total())
    print(Rep[2](0).first[2](), Rep[2](0).first[3]())
