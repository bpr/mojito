# A static or initializer of a parametric struct read as a value is a
# function value of the struct instance it is spelled through, as upstream's
# reference names the struct's specialization and the method together:
# stored, passed, selected by an expected type, specialized by a bracket,
# read through `Self`, and read over a type or value parameter.
struct P[T: Copyable]:
    @staticmethod
    def st(x: Int) -> Int:
        return x + 1

    @staticmethod
    def tw(x: Self.T) -> Self.T:
        return x.copy()

    def __init__(out self):
        pass

    def m(self) -> Int:
        var f = Self.st
        return f(10)


struct W[n: Int]:
    @staticmethod
    def mul(x: Int) -> Int:
        return x * Self.n

    @staticmethod
    def ov(x: Int) -> Int:
        return x + Self.n

    @staticmethod
    def ov(x: String) -> String:
        return x + "!"

    @staticmethod
    def gst[k: Int](x: Int) -> Int:
        return x * k + Self.n

    def __init__(out self):
        pass

    def run(self) -> Int:
        var g = Self.gst[2]
        return g(7)


struct Q[n: Int]:
    var v: Int

    def __init__(out self, k: Int):
        self.v = k * Self.n


def apply(f: def(Int) thin -> Int, v: Int) -> Int:
    return f(v)


def main():
    var f = P[Int].st
    print(f(3))
    var h = P[String].tw
    print(h("hi"))
    print(P[Int]().m())
    var g = W[4].mul
    print(g(5))
    print(apply(W[3].mul, 2))
    var o: def(Int) thin -> Int = W[10].ov
    print(o(1))
    var s: def(String) thin -> String = W[10].ov
    print(s("a"))
    var k = W[1].gst[3]
    print(k(5))
    print(W[5]().run())
    var c: def(Int) thin -> Q[3] = Q[3].__init__
    print(c(5).v)
