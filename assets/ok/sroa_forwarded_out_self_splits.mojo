# An `@always_inline` initializer's `out self` is stored whole into the
# caller's variable, which `sroa` splits; each field moves out of the
# forwarded value once, a nested struct's field included.


struct In(Movable):
    var n: Int
    var t: String

    def __init__(out self, n: Int):
        self.n = n
        self.t = String("i") + String(n)


struct W(Movable):
    var n: Int
    var t: String

    @always_inline
    def __init__(out self, n: Int):
        self.n = n
        self.t = String("x") + String(n)
        self.show()

    def show(self):
        print(self.n)


struct Out(Movable):
    var k: Int
    var inner: In
    var u: String

    @always_inline
    def __init__(out self, k: Int):
        self.k = k
        self.inner = In(k)
        self.u = String("u") + String(k)
        self.show()

    def show(self):
        print(self.k)


def main():
    var w = W(3)
    w.n += 1
    print(w.t, w.n)
    var o = Out(7)
    o.k += 1
    o.inner.n += 2
    print(o.inner.t, o.u, o.k, o.inner.n)
