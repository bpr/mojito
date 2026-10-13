# A copy construction an owned (`var` or `deinit`) receiver takes at its
# source's last use forwards the source's storage to the method, as at Mojo:
# no copy or move initializer runs. A raising argument evaluated after the
# receiver makes it a move instead, and a source read afterwards still copies.
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

    def consume(var self):
        print("consume", self.v)

    def finish(deinit self):
        print("finish", self.v)

    def with_arg(var self, n: Int):
        print("with_arg", self.v, n)

    def maybe(var self) raises:
        print("maybe", self.v)

    def ret(var self) -> Int:
        return self.v

    def show(self):
        print("show", self.v)


def might(n: Int) raises -> Int:
    return n


def param(var p: M):
    M(copy=p).consume()


def main() raises:
    var a = M(1)
    M(copy=a).finish()
    var b = M(2)
    M(copy=b).with_arg(7)
    var c = M(3)
    M(copy=c).with_arg(might(8))
    var d = M(4)
    M(copy=d).maybe()
    var e = M(5)
    print(M(copy=e).ret())
    param(M(6))
    for i in range(10, 12):
        var l = M(i)
        M(copy=l).consume()
    var g = M(20)
    if g.v > 0:
        M(copy=g).consume()
    var k = M(40)
    try:
        M(copy=k).maybe()
    except:
        pass
    var s = M(50)
    M(copy=s).consume()
    s.show()
