# expect: value passed to a mutable argument must be mutable
# A subscript whose `__getitem__` returns a value and whose type declares no
# `__setitem__` is no place, so it cannot bind a `mut` parameter.


struct G(Movable):
    var items: List[Int]

    def __init__(out self):
        self.items = [0, 0]

    def __getitem__(self, i: Int) -> Int:
        return self.items[i]


def bump(mut x: Int):
    x += 1


def main():
    var g = G()
    bump(g[1])
    print(g.items[1])
