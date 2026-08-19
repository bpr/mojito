# The bare element-call spelling dispatches as subscript-then-indirect-call,
# matching current Mojo: identifier bases (`objs[0](3)`), member bases
# (`h.items[0](5)`), and multi-index brackets (`g[1, 1](10)`) all read the
# element through the selected `__getitem__` contract and call it, with output
# equal to the parenthesized `(objs[0])(3)` spelling. A raising getter keeps
# its own effect: the subscript raise is catchable around the bare call.
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

struct Bank(Copyable):
    var items: List[Doubler]

    def __init__(out self, var items: List[Doubler]):
        self.items = items^

    def __getitem__(self, index: Int) raises -> Doubler:
        if index >= len(self.items):
            raise Error("bank index out of range")
        return self.items[index]

def main():
    var objs: List[Doubler] = [Doubler(2)]
    print(objs[0](3))
    print((objs[0])(3))
    var h: Holder = Holder([Doubler(3)])
    print(h.items[0](5))
    var g: Grid = Grid([Doubler(1), Doubler(2), Doubler(3), Doubler(4)])
    print(g[1, 1](10))
    var bank: Bank = Bank([Doubler(2)])
    try:
        print(bank[7](3))
    except e:
        print("caught: bank getter raised")
    try:
        print(bank[0](4))
    except e:
        print("unreachable")
