struct M(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", self.v)

    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
        print("move", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def two(var a: M, n: Int):
    print("two", a.v, n)


def boom(n: Int) raises -> Int:
    raise Error("boom")


def main():
    var c = M(3)
    try:
        two(M(copy=c), boom(8))
    except:
        print("caught")
    var x = M(5)
    try:
        two(x^, boom(1))
    except:
        print("caught2")
