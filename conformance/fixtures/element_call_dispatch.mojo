# Bare element-call dispatch: `objs[0](3)`, member-base `h.items[0](5)`, and
# multi-index `g[1, 1](10)` all subscript the runtime value and call the
# element, printing the same results as the parenthesized spelling.
@fieldwise_init
struct Doubler(def(Int) -> Int, Copyable, ImplicitlyCopyable):
    var gain: Int

    def __call__(self, x: Int) -> Int:
        return x * self.gain

@fieldwise_init
struct Holder(Copyable):
    var items: List[Doubler]

struct Grid(Copyable):
    var cells: List[Doubler]

    def __init__(out self, var cells: List[Doubler]):
        self.cells = cells^

    def __getitem__(self, row: Int, column: Int) -> Doubler:
        return self.cells[row * 2 + column]

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[0](3))
    print((objs[0])(3))
    var h: Holder = Holder([Doubler(3)])
    print(h.items[0](5))
    var g: Grid = Grid([Doubler(1), Doubler(2), Doubler(3), Doubler(4)])
    print(g[1, 1](10))
