# Plain subscript assignment without `__setitem__` writes through the
# mutable-reference-returning `__getitem__` — upstream Array's semantics, and
# the same fallback any user struct with only a reference getter receives.
@fieldwise_init
struct Cell:
    var v: Int

@fieldwise_init
struct Grid:
    var cell: Cell
    def __getitem__(ref self, i: Int) -> ref[origin_of(self)] Cell:
        return self.cell

def main():
    var a = [1, 2, 3]
    a[0] = 5
    a[1] = a[2] + 10
    print(a)
    var g = Grid(Cell(1))
    g[0] = Cell(9)
    print(g[0].v)
