# A multi-argument subscript element dispatches its in-place dunder through the
# same value-getter/setter path.
@fieldwise_init
struct Counter(Copyable, Movable):
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

@fieldwise_init
struct Grid:
    var cell: Counter

    def __getitem__(self, i: Int, j: Int) -> Counter:
        return self.cell

    def __setitem__(mut self, i: Int, j: Int, v: Counter):
        self.cell = v

def main():
    var g = Grid(Counter(1))
    g[0, 1] += 40
    print(g.cell.value)
