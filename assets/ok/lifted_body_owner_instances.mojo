# A function reads, through a body lifted from it, only the binders that
# body kept: `lifted[n, m]`, whose compile-time value reads only `n`, has
# one instance per `n`, and `Box[n].get[m]`, whose default reads only
# `Self.n`, one, however `m` varies, as on the pin. A free function's
# evaluated default is the call site's, so it reads none of the callee's
# parameters: `dflt[n, m]` returning `x` has one instance, and
# `scaled[n, m]`, defaulting `x` to `m * 2`, one per `n`, as on the pin.
# So is a positional-only one every earlier omitted default precedes:
# `ponly[n, m]` has one instance per `n`, and `pboth[n, m]` one. A method's
# default is the call's too: `Box[n].sum[m]`, defaulting `x` to `Self.n +
# m`, has one instance however `m` varies, and `pafter[n, m]`, whose unused
# `x` defaults to a call, one per `n`.
def dflt[n: Int, m: Int](x: Int = n * 2) -> Int:
    return x


def scaled[n: Int, m: Int](x: Int = m * 2) -> Int:
    return n + x


def ponly[n: Int, m: Int](x: Int, y: Int = m * 2, /) -> Int:
    return n + x + y


def pboth[n: Int, m: Int](x: Int = n + 1, y: Int = m * 2, /) -> Int:
    return x + y


def side() -> Int:
    return 7


def pafter[n: Int, m: Int](x: Int = side(), y: Int = m * 2, /) -> Int:
    return n + y


def lifted[n: Int, m: Int]() -> Int:
    comptime k = String("ab") * n
    return k.byte_length()


struct Box[n: Int]:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def get[m: Int](self, x: Int = Self.n + 1) -> Int:
        return self.v + x

    def sum[m: Int](self, x: Int = Self.n + m) -> Int:
        return self.v + x


def main():
    print(dflt[1, 3]())
    print(dflt[1, 4]())
    print(dflt[2, 4](7))
    print(scaled[1, 3]())
    print(scaled[1, 4]())
    print(scaled[2, 4](7))
    print(ponly[1, 3](0))
    print(ponly[1, 4](0))
    print(ponly[2, 4](0, 7))
    print(pboth[1, 3]())
    print(pboth[2, 4]())
    print(pboth[2, 4](1, 2))
    print(lifted[1, 3]())
    print(lifted[1, 4]())
    print(lifted[2, 4]())
    print(Box[2](1).get[5]())
    print(Box[2](1).get[6](7))
    print(Box[2](1).sum[5]())
    print(Box[2](1).sum[6]())
    print(pafter[1, 3]())
    print(pafter[1, 4]())
    print(pafter[2, 4](7))
