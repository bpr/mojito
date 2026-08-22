# User-struct subscripts: a reference-yielding `__getitem__` composing with
# in-place updates (`xs[i] += k` routes the returned place pointer through
# the write), and a `mut self` `__setitem__` written back to the receiver.
@fieldwise_init
struct Grid:
    var a: Int
    var b: Int

    def __getitem__(ref self, i: Int) -> ref[origin_of(self)] Int:
        if i == 0:
            return self.a
        return self.b

    def __setitem__(mut self, i: Int, value: Int):
        if i == 0:
            self.a = value * 10
        else:
            self.b = value * 10

def main():
    var g = Grid(1, 2)
    g[0] += 5
    print(g.a, g.b)
    g[1] = 7
    print(g.a, g.b)
    print(g[0], g[1])
