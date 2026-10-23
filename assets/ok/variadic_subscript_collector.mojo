# A variadic subscript passes the pack its call builds: `g[1, 2, 3]` and
# `g[1, 2] = v` collect their indices into the accessor's `*idx` as a call
# of `__getitem__`/`__setitem__` would.
struct Grid:
    var last: Int

    def __init__(out self):
        self.last = 0

    def __getitem__(self, *idx: Int) -> Int:
        var s = 0
        for i in idx:
            s += i
        return s

    def __setitem__(mut self, *idx: Int, value: Int):
        var s = 0
        for i in idx:
            s += i
        self.last = s * value


def main():
    var g = Grid()
    print(g[1, 2, 3])
    print(g[5])
    g[1, 2] = 10
    print(g.last)
