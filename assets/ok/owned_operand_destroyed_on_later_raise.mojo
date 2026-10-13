# An owned call operand is consumed at the call: a raise in a later operand
# destroys every owned operand evaluated before it, before the handler runs,
# and an operand may still read a value a `^` operand transfers.


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


def two(var a: M, n: Int):
    print("two", a.v, n)


def rd(a: M, n: Int):
    print("rd", a.v, n)


def three(var a: M, var b: M, n: Int):
    print("three", a.v, b.v, n)


def owe(n: Int, var a: M):
    print("owe", a.v, n)


def kw(n: Int, *, var a: M):
    print("kw", a.v, n)


def many(var *xs: M, n: Int):
    print("many", n)


def look(a: M) -> Int:
    return a.v


def boom(n: Int) raises -> Int:
    raise Error("boom")


def maybe(n: Int) raises -> Int:
    if n < 0:
        raise Error("neg")
    return n


def param_in_try(var y: M):
    try:
        two(y^, boom(2))
    except:
        print("caught-param")


def conditional_operand(c: Bool):
    var y = M(60)
    var z = M(61)
    try:
        two(y^ if c else z^, boom(1))
    except:
        print("caught-conditional")


def nested_try():
    var x = M(80)
    try:
        try:
            two(x^, boom(1))
        finally:
            print("finally")
    except:
        print("caught-nested")


def main():
    var c = M(3)
    try:
        two(M(copy=c), boom(8))
    except:
        print("caught-copy")
    var x = M(5)
    try:
        two(x^, boom(1))
    except:
        print("caught-transfer")
    var r = M(4)
    try:
        M(copy=r).with_arg(boom(8))
    except:
        print("caught-receiver")
    try:
        two(M(7), boom(1))
    except:
        print("caught-rvalue")
    try:
        rd(M(8), boom(1))
    except:
        print("caught-read")
    var a = M(10)
    var b = M(11)
    try:
        three(a^, b^, boom(1))
    except:
        print("caught-two-owned")
    var e = M(12)
    try:
        three(e^, M(13), boom(1))
    except:
        print("caught-mixed")
    try:
        var y = M(20)
        two(y^, boom(1))
    except:
        print("caught-local")
    param_in_try(M(14))
    try:
        kw(boom(1), a=M(51))
    except:
        print("caught-kw-after")
    try:
        kw(a=M(52), n=boom(1))
    except:
        print("caught-kw-first")
    var f = M(70)
    try:
        many(f^, M(71), n=boom(1))
    except:
        print("caught-collector")
    conditional_operand(False)
    nested_try()
    var m = M(2)
    try:
        _ = maybe(-1)
        var d = M(copy=m)
        print("d", d.v)
    except:
        print("caught-outer-value")
    var q = M(22)
    try:
        two(q^, maybe(0))
    except:
        print("caught-q")
    var g = M(30)
    two(g^, look(g))
    var h = M(40)
    two(h^, h.v)
    var k = M(41)
    owe(k.v, k^)
    var s = M(1)
    two(M(copy=s), 5)
    var t = M(2)
    M(copy=t).with_arg(7)
    print("end")
