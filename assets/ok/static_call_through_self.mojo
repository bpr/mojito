# A static method called through `Self` is called on the instance `Self`
# names: the enclosing struct applied to its own parameters.

struct Plain:
    var x: Int

    def __init__(out self):
        self.x = 3

    @staticmethod
    def scale() -> Int:
        return 10

    @staticmethod
    def add(a: Int, b: Int = 1) -> Int:
        return a + b

    def scaled(self) -> Int:
        return self.x * Self.scale() + Self.add(1) + Self.add(1, b=5)

struct Grid[n: Int, T: Copyable & Deinitable]:
    var cell: Self.T

    def __init__(out self, cell: Self.T):
        self.cell = cell.copy()

    @staticmethod
    def cells() -> Int:
        return Self.n * Self.n

    @staticmethod
    def wide() -> Bool:
        return Self.n > 2

    @staticmethod
    def offset(i: Int) -> Int:
        return Self.cells() + i

    def describe(self) -> Int:
        if Self.wide():
            return Self.offset(100)
        return Self.cells()

def main():
    print(Plain().scaled())
    print(Grid[2, Int](7).describe())
    print(Grid[3, Bool](True).describe())
    print(Grid[3, Int].offset(1))
