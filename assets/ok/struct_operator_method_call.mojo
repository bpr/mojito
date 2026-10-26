# A struct operator is its dunder's method call: `a - b` is `a.__sub__(b)`,
# `-x` is `x.__neg__()`, and `1 + m` is `m.__radd__(1)`, so a dunder owning
# its receiver runs as a call, a display or leading-dot right operand is
# built as the selected parameter's type, a generic dunder solves its
# parameters as a call does, also as a chained comparison's link, and the
# operands evaluate in source order.
struct N(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @always_inline("nodebug")
    def __sub__(deinit self, o: Int) -> Int:
        return self.v - o

    def __neg__(deinit self) -> Int:
        return -self.v


struct A:
    var t: Int

    def __init__(out self, t: Int):
        self.t = t


struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __radd__(self, o: Int) -> Int:
        return self.v + o


struct C(Equatable, ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @staticmethod
    def mk() -> C:
        return C(1)

    def __eq__(self, o: Self) -> Bool:
        return self.v == o.v


struct H(ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __lt__[T: Intable](self, o: T) -> Bool:
        return self.v < Int(o)


def bump(mut a: A) -> List[Int]:
    a.t += 100
    return [a.t]


def f() -> Int:
    print("f")
    return 1


def g() -> M:
    print("g")
    return M(10)


def main():
    print(N(3) - 1)
    var n = N(3)
    print(-(n^))
    var l: List[Int] = [1]
    print(l == [1], l != [2])
    print(len(l + [2]))
    var b = A(1)
    var s = bump(b) + [b.t]
    print(s[0], s[1])
    print(f() + g())
    var c = C(1)
    print(c == .mk())
    var h = H(1)
    print(h < 2, h < 2 < 3)
