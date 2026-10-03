# A module-level compile-time evaluation beside a generic struct whose method
# holds a `comptime if` on `Self.T`: the evaluation's subprogram carries the
# method as its unspecialized stub, so the unbound `comptime if` does not fail
# the subprogram's check, and each instance's method still folds its own arm.
def doubled(n: Int) -> Int:
    return n * 2

comptime CAP = doubled(5)

struct Cell[T: Copyable & Deinitable & Writable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def label(self) -> String:
        comptime if Self.T == Int:
            return "int " + String(self.value)
        else:
            return "other " + String(self.value)

def main():
    print(Cell(9).label(), Cell(String("b")).label(), CAP)
