# A read temporary is destroyed after the call that reads it, even when a
# later argument is a conditional expression whose branch separates the
# temporary's load from the call, as at Mojo: at top level, inside a `try`,
# round a loop, as a method receiver, and beside a raising arm.
struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct H(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def show(self, n: Int):
        print("H.show", self.v, n)

    def __deinit__(deinit self):
        print("del H", self.v)


def show_n(s: M, n: Int):
    print("show_n", s.v, n)


def may(c: Bool) raises -> Int:
    if not c:
        raise Error("no")
    return 5


def main() raises:
    var c = True
    show_n(M(2), 3 if c else 4)
    try:
        show_n(M(7), 3 if c else 4)
    except e:
        print(e)
    for i in range(2):
        show_n(M(10 + i), 1 if c else 0)
    H(9).show(3 if c else 4)
    show_n(M(8), may(c) if c else 0)
    print("end")
