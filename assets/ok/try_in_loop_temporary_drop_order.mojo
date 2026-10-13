# A raising call's argument temporaries inside a `try` in a loop die when the
# call returns, before its result's consumer, whether the call raises or not.
struct S(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct P(Movable):
    var total: Int

    def __init__(out self, x: S, y: S, n: Int) raises:
        if n == 1:
            raise Error("init")
        self.total = x.v + y.v


struct Q(Movable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def m(self, x: S, y: S, n: Int) raises -> Int:
        if n == 1:
            raise Error("method")
        return self.base + x.v + y.v


def mk(x: S, y: S, n: Int) raises -> Int:
    if n == 1:
        raise Error("free")
    return x.v + y.v


def main():
    var q = Q(100)
    for j in range(2):
        try:
            print(mk(S(5), S(6), j))
        except e:
            print("caught", e)
        try:
            print(P(S(7), S(8), j).total)
        except e:
            print("caught", e)
        try:
            print(q.m(S(9), S(10), j))
        except e:
            print("caught", e)
    var k = 0
    while k < 2:
        try:
            print(mk(S(11), S(12), k))
        except e:
            print("caught", e)
        k += 1
