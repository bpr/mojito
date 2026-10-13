# A copy construction a read receiver takes at its source's last use lends
# the source's storage to the method, as at Mojo: no copy or move initializer
# runs, and the source is destroyed when the call returns. A source read
# afterwards still copies, a raising argument evaluated after the receiver
# makes it a move, and a `ref self` receiver, a field read, and an argument
# that branches keep the move, as at the pin.
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

    def show(self):
        print("show", self.v)

    def show_n(self, n: Int):
        print("show_n", self.v, n)

    def show_r(self) raises:
        print("show_r", self.v)
        raise Error("r")

    def peek(ref self) -> Int:
        print("peek", self.v)
        return self.v


def boom(x: Int) raises -> Int:
    if x > 0:
        raise Error("boom")
    return x


def pure(x: Int) -> Int:
    print("pure", x)
    return x


def param(var p: M):
    M(copy=p).show()


def lends():
    var a = M(1)
    M(copy=a).show()
    var b = M(2)
    M(copy=b).show()
    print("b", b.v)
    var c = M(3)
    M(copy=c).show_n(pure(30))
    M(copy=M(4)).show()
    param(M(5))
    for i in range(10, 12):
        var l = M(i)
        M(copy=l).show()
    var k = M(12)
    if k.v > 0:
        M(copy=k).show()
    print("lends end")


def moves():
    var c = True
    var a = M(20)
    _ = M(copy=a).peek()
    var b = M(21)
    print(M(copy=b).v)
    var d = M(22)
    M(copy=d).show_n(3 if c else 4)
    print("moves end")


def raising():
    var a = M(30)
    try:
        M(copy=a).show_n(boom(1))
    except e:
        print("caught", e)
    var b = M(31)
    try:
        M(copy=b).show_r()
    except e:
        print("caught", e)
    try:
        var c = M(32)
        M(copy=c).show_r()
    except e:
        print("caught", e)
    print("raising end")


def main():
    lends()
    moves()
    raising()
