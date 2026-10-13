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

    def with_arg(var self, n: Int):
        print("with_arg", self.v, n)


def boom(n: Int) raises -> Int:
    raise Error("boom")


def main():
    var c = M(3)
    try:
        M(copy=c).with_arg(boom(8))
    except:
        print("caught")
