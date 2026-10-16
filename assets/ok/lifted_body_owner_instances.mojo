# A function reads, through a body lifted from it, only the binders that
# body kept: `lifted[n, m]`, whose compile-time value reads only `n`, has
# one instance per `n`, and `Box[n].get[m]`, whose default reads only
# `Self.n`, one, however `m` varies, as on the pin. A free function's
# evaluated default is the call site's, so it reads none of the callee's
# parameters: `dflt[n, m]` returning `x` has one instance, and
# `scaled[n, m]`, defaulting `x` to `m * 2`, one per `n`, as on the pin.
def dflt[n: Int, m: Int](x: Int = n * 2) -> Int:
    return x


def scaled[n: Int, m: Int](x: Int = m * 2) -> Int:
    return n + x


def lifted[n: Int, m: Int]() -> Int:
    comptime k = String("ab") * n
    return k.byte_length()


struct Box[n: Int]:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def get[m: Int](self, x: Int = Self.n + 1) -> Int:
        return self.v + x


def main():
    print(dflt[1, 3]())
    print(dflt[1, 4]())
    print(dflt[2, 4](7))
    print(scaled[1, 3]())
    print(scaled[1, 4]())
    print(scaled[2, 4](7))
    print(lifted[1, 3]())
    print(lifted[1, 4]())
    print(lifted[2, 4]())
    print(Box[2](1).get[5]())
    print(Box[2](1).get[6](7))
