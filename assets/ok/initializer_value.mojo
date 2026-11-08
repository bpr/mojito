# An initializer read through its struct is a function value returning the
# struct its `out self` initializes. The struct's synthesized move
# initializer is always another candidate, so the expected type or a
# bracket's operands select it: stored, passed, specialized by a bracket,
# read through `Self`, and synthesized by `@fieldwise_init`.
struct Q:
    var a: Int

    def __init__(out self, k: Int):
        self.a = k


struct R:
    var a: Int

    def __init__[k: Int](out self):
        self.a = k


struct N:
    var s: String

    def __init__(out self, s: String):
        self.s = s + "!"

    def __init__(out self, n: Int):
        self.s = String(n)


struct M:
    var a: Int

    def __init__(out self, mut k: Int):
        k += 1
        self.a = k


struct Z:
    def __init__(out self):
        pass

    def hi(self):
        print("z")


@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int

    def swapped(self) -> Self:
        var f: def(Int, Int) thin -> Self = Self.__init__
        return f(self.y, self.x)


def apply(f: def(Int) thin -> Q, n: Int) -> Q:
    return f(n)


def run(f: def() thin -> R) -> Int:
    return f().a


def main():
    var g: def(Int) thin -> Q = Q.__init__
    print(g(3).a)
    print(apply(Q.__init__, 5).a)
    var f = R.__init__[7]
    print(f().a)
    print(run(R.__init__[9]))
    var h: def(String) thin -> N = N.__init__
    print(h("hey").s)
    var i: def(Int) thin -> N = N.__init__
    print(i(42).s)
    var m: def(mut k: Int) thin -> M = M.__init__
    var n = 4
    print(m(n).a, n)
    var z: def() thin -> Z = Z.__init__
    z().hi()
    var p: def(Int, Int) thin -> P = P.__init__
    var q = p(1, 2).swapped()
    print(q.x, q.y)
    var b: def() thin -> Bool = Bool.__init__
    print(b())
