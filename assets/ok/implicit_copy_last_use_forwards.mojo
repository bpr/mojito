# An implicit copy whose source dies right there is no copy, as upstream's
# lifetime check turns a copy followed by the source's destruction into the
# source's own transfer: an owned parameter, an owned receiver, or an owned
# operator operand takes the dying source's storage with no move
# initializer, even past a raising later argument; a binding, an owning
# collector's element, and a chained comparison's first middle operand move
# it in; a source read again later is still copied.
struct C(ImplicitlyCopyable, Movable, Equatable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v

    def __init__(out self, *, deinit move: Self):
        print("move", move.v)
        self.v = move.v

    def __deinit__(deinit self):
        print("del", self.v)

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def consume(var self) -> Int:
        return self.v

    def __add__(var self, var other: Self) -> Self:
        return C(self.v + other.v)


def take(var c: C):
    print("take", c.v)


def take_after(var c: C, n: Int):
    print("take_after", c.v, n)


def take_keyword(*, var c: C):
    print("take_keyword", c.v)


def take_all(var *cs: C):
    print("take_all", len(cs))


def checked(n: Int) raises -> Int:
    if n < 0:
        raise Error("negative")
    return n


def forward_parameter(var c: C):
    take(c)


def main() raises:
    print("-- owned argument")
    var a = C(1)
    take(a)
    print("-- keyword argument")
    var b = C(2)
    take_keyword(c=b)
    print("-- raising later argument")
    var d = C(3)
    take_after(d, checked(30))
    print("-- owned receiver")
    var e = C(4)
    print(e.consume())
    print("-- owned operator operands")
    var f = C(5)
    var g = C(6)
    var h = f + g
    print(h.v)
    print("-- var parameter")
    forward_parameter(C(7))
    print("-- last use in one arm")
    var i = C(8)
    if h.v > 0:
        take(i)
    else:
        print(i.v)
    print("-- chained comparison middle")
    var j = C(9)
    var k = C(9)
    var l = C(9)
    print(j == k == l)
    print("-- binding")
    var m = C(10)
    var n = m
    print(n.v)
    print("-- owning collector")
    var o = C(11)
    take_all(o)
    print("-- read again later")
    var p = C(12)
    take(p)
    print(p.v)
    print("end")
