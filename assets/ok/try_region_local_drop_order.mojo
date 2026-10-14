# A value only a `try`'s handler, `else`, or `finally` defines is scoped to
# that region: it dies at its last use there, not round an enclosing loop or
# after the `try`, and nothing the `try` ended is destroyed again.
struct Res(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del res", self.n)


def fail(n: Int) raises:
    if n > 0:
        raise Error(String("b") + String(n))
    print("ok", n)


def main():
    var outer = Res(0)
    for i in range(3):
        try:
            fail(i)
            print("body", outer.n)
        except e:
            var local = Res(10 + i)
            print("caught", e, local.n)
        print("after", i)
    try:
        fail(5)
    except e:
        pass
    print("after pass")
    try:
        fail(0)
    except e:
        print("never")
    else:
        var r = Res(7)
        print("else", r.n)
    finally:
        var g = Res(8)
        print("finally", g.n)
    print("end")
