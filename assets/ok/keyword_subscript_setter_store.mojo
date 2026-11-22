# A store, an in-place update, a destructuring store, and a write-back
# through a value getter and a `__setitem__` bound by a keyword,
# positional-and-keyword, or keyword-slice bracket pass each keyword index
# to the setter by name, with the value after them; a variadic accessor
# collects its positional indices beside a keyword one.


@fieldwise_init
struct Counter(Copyable, ImplicitlyCopyable, Movable):
    var n: Int

    def bump(mut self):
        self.n += 1


struct Shelf:
    var items: List[Counter]
    var grid: List[Int]

    def __init__(out self):
        self.items = [Counter(10), Counter(20)]
        self.grid = [0, 0, 0, 0, 0, 0]

    def __getitem__(self, *, k: Int) -> Counter:
        return self.items[k]

    def __setitem__(mut self, *, k: Int, value: Counter):
        self.items[k] = value

    def __getitem__(self, i: Int, *, k: Int) -> Int:
        return self.grid[i * 3 + k]

    def __setitem__(mut self, i: Int, *, k: Int, value: Int):
        self.grid[i * 3 + k] = value


struct Cells:
    var x: List[Int]

    def __init__(out self):
        self.x = [2, 0, 0, 0]

    def __getitem__(self, *, k: Int) -> Int:
        return self.x[k]

    def __setitem__(mut self, *, k: Int, value: Int):
        self.x[k] = value + k

    def __getitem__(self, *, span: Slice) -> Int:
        var start = span.start.value() if span.start else 0
        return self.x[start]

    def __setitem__(mut self, *, span: Slice, value: Int):
        var start = span.start.value() if span.start else 0
        var end = span.end.value() if span.end else len(self.x)
        for i in range(start, end):
            self.x[i] = value


struct Names:
    var s: List[String]

    def __init__(out self):
        self.s = ["a", "b"]

    def __getitem__(self, *, at: Int) -> String:
        return self.s[at]

    def __setitem__(mut self, *, at: Int, value: String):
        self.s[at] = value


struct Grid:
    var v: Int

    def __init__(out self):
        self.v = 0

    def __getitem__(self, *indices: Int, k: Int) -> Int:
        return self.v + k

    def __setitem__(mut self, *indices: Int, k: Int, value: Int):
        var total = 0
        for i in indices:
            total += i
        self.v = total + k + value


def index() -> Int:
    print("index")
    return 1


def main():
    var c = Cells()
    c[k=1] = 5
    print(c.x[1])
    c[k=1] += 2
    print(c[k=1])
    c[k=0], c[k=3] = 4, 9
    print(c.x[0], c.x[3])
    c[span=1:3] = 6
    c[span=1:3] += 1
    print(c.x[1], c.x[2])

    var shelf = Shelf()
    shelf[k=1].bump()
    print(shelf.items[1].n)
    shelf[k=0] = Counter(5)
    print(shelf.items[0].n)
    shelf[1, k=2] = 7
    shelf[1, k=2] += 3
    print(shelf[1, k=2])
    shelf[k=index()] = Counter(1)
    print(shelf.items[1].n)

    var names = Names()
    names[at=0] = "x"
    names[at=1] += "y"
    var kept = String("zz")
    names[at=0] = kept
    print(names[at=0], names[at=1], kept)

    var grid = Grid()
    grid[1, 2, k=3] = 4
    grid[1, k=1] += 1
    print(grid.v, grid[k=2])
