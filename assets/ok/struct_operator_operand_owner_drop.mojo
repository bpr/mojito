# A struct operator is its dunder call: the operands' owners die when the
# dunder returns, before the result's consumer runs.
struct S(Equatable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __add__(self, o: Int) -> Int:
        return self.v + o

    def __sub__(self, o: S) -> Int:
        return self.v - o.v

    def __eq__(self, o: S) -> Bool:
        return self.v == o.v

    def __radd__(self, o: Int) -> Int:
        return self.v + o

    def __deinit__(deinit self):
        print("del", self.v)


struct G[T: Copyable](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __mul__(self, o: Int) -> Int:
        return self.v * o

    def __deinit__(deinit self):
        print("gdel", self.v)


def main():
    var a = S(1)
    print(a + 1)
    print("--")
    var b = S(2)
    var c = S(3)
    print(b - c)
    print("--")
    var d = S(4)
    var e = S(5)
    print(d != e)
    print("--")
    var f = S(6)
    print(1 + f)
    print("--")
    var g = G[Int](7)
    print(g * 2)
    print("--")
    var h = S(8)
    var x = h + 1
    print(x)
    print("end")
